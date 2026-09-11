#!/usr/bin/env bash
# Prove that no secret leaves this machine: not in git history, not in the built
# binary or the wasm every visitor downloads, not in the image's layers or metadata.
#
# Written because "the env is injected at build time, could it be reversed out?" is
# a question that deserves a measurement rather than an argument. The answer depends
# on facts that can change with one careless line -- an `env!()` instead of
# `std::env::var`, a `COPY . .`, a `--build-arg` -- so it is checked, not remembered.
#
#   scripts/audit-secrets.sh              # history + tracked files + built artifacts
#   scripts/audit-secrets.sh --image REF  # also every layer and the config of an image
#
# What it compares against: every value in .env longer than 12 characters, plus a
# set of credential shapes (Google OAuth secrets, Gemini session cookies, tunnel
# tokens, AWS-style keys) that would be a leak even if they are not in this .env.
#
# No secret value is ever printed, including on failure -- output is key names and
# locations only, so the log of a failing run is itself safe to paste.
set -uo pipefail
cd "$(dirname "$0")/.."

IMAGE=""
if [ "${1:-}" = "--image" ]; then
  IMAGE="${2:?--image needs an image reference}"
fi

if [ ! -f .env ]; then
  echo "audit-secrets: no .env here, so there are no known values to search for." >&2
  echo "audit-secrets: shape patterns will still run." >&2
fi

IMAGE="$IMAGE" python3 - <<'PY'
import json, os, pathlib, re, subprocess, sys, tarfile, tempfile

FAIL = []

def load_env():
    vals = {}
    p = pathlib.Path(".env")
    if not p.exists():
        return vals
    for line in p.read_text(errors="replace").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        v = v.strip().strip('"').strip("'")
        # Short values match everywhere and prove nothing; a bind address or a
        # bucket name is not a credential.
        if len(v) >= 12:
            vals[k.strip()] = v
    return vals

# Values that are public by design. They live in .env only because the app reads
# them from the environment, and finding them in an artifact is correct, not a leak.
PUBLIC = {"SITE_ORIGIN", "R2_PUBLIC_BASE", "R2_BUCKET", "LEPTOS_SITE_ADDR",
          "GEMINI_URL", "KEEPER_URL", "SIDECAR_URL", "RUST_LOG", "GOOGLE_CLIENT_ID",
          "TURNSTILE_SITE_KEY", "MIRROR_SITEMAP", "MIRROR_MATCH"}

# Credential shapes, so a secret that is not in this .env is still caught. Kept
# narrow on purpose: a pattern with false positives gets ignored, and an ignored
# check is worse than no check.
SHAPES = {
    "google-oauth-secret": re.compile(rb"GOCSPX-[A-Za-z0-9_\-]{20,}"),
    "gemini-1psid-cookie": re.compile(rb"g\.a000[A-Za-z0-9_\-]{40,}"),
    "gemini-1psidts-cookie": re.compile(rb"sidts-[A-Za-z0-9_\-]{30,}"),
    "cloudflare-tunnel-token": re.compile(rb"eyJhIjoi[A-Za-z0-9+/=]{40,}"),
    "aws-style-secret-key": re.compile(rb"(?i)(secret[_-]?access[_-]?key\"?\s*[:=]\s*\"?)[A-Za-z0-9/+]{40}"),
    "private-key-block": re.compile(rb"-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----"),
}

def scan(name, data, vals, note_public=False):
    """Report which known values or credential shapes appear in `data`."""
    hits = []
    for k, v in vals.items():
        if v.encode() in data:
            if k in PUBLIC:
                if note_public:
                    hits.append(("public", k))
            else:
                hits.append(("SECRET", k))
    for shape, pat in SHAPES.items():
        if pat.search(data):
            hits.append(("SECRET", f"shape:{shape}"))
    for level, key in hits:
        if level == "SECRET":
            FAIL.append(f"{name}: {key}")
    return hits

vals = load_env()
print(f"audit-secrets: {len(vals)} known values from .env, {len(SHAPES)} credential shapes")
print()

# ---- 1. git history -------------------------------------------------------
# Every object that has ever existed, because making a repo public publishes all
# of them -- a secret removed in a later commit is still there in the earlier one.
print("[1/4] git history (every object, every commit message)")
oids = [l.split()[0] for l in subprocess.run(
    ["git", "rev-list", "--objects", "--all"],
    capture_output=True, text=True).stdout.splitlines() if l.strip()]
blob = bytearray()
for i in range(0, len(oids), 400):
    p = subprocess.run(["git", "cat-file", "--batch"], input="\n".join(oids[i:i+400]),
                       capture_output=True, text=True, errors="replace")
    blob += p.stdout.encode("utf-8", "replace")
blob += subprocess.run(["git", "log", "--all", "--format=%B"],
                       capture_output=True, text=True).stdout.encode("utf-8", "replace")
hits = scan("git-history", bytes(blob), vals, note_public=True)
pub = sorted(k for lvl, k in hits if lvl == "public")
print(f"      {len(oids)} objects scanned")
if pub:
    print(f"      public config present, as expected: {', '.join(pub)}")

# ---- 2. tracked working tree ----------------------------------------------
print("[2/4] tracked files in the working tree")
tracked = subprocess.run(["git", "ls-files"], capture_output=True, text=True).stdout.split()
for f in tracked:
    try:
        scan(f"tracked:{f}", pathlib.Path(f).read_bytes(), vals)
    except OSError:
        pass

# ---- 3. built artifacts ---------------------------------------------------
# The binary is what "could be reversed"; the wasm and JS are what every visitor
# downloads without being asked.
print("[3/4] built artifacts (binary, wasm, js, css)")
built = [pathlib.Path("target/release/geekgallery")]
built += [p for p in pathlib.Path("target/site").rglob("*") if p.is_file()] \
         if pathlib.Path("target/site").is_dir() else []
built = [p for p in built if p.exists()]
if not built:
    print("      none present -- run `cargo leptos build --release` to include them")
for p in built:
    scan(f"artifact:{p}", p.read_bytes(), vals)

# ---- 4. the image ---------------------------------------------------------
# Layers *and* config: a --build-arg leaks into the layer history, an ENV leaks
# into the config, and a stray COPY leaks into a layer. All three are read by
# anyone who can pull the image.
image = os.environ.get("IMAGE") or ""
if not image:
    print("[4/4] image: skipped (pass --image REF to include it)")
else:
    print(f"[4/4] image {image}")
    meta = subprocess.run(["docker", "image", "inspect", image],
                          capture_output=True, text=True)
    if meta.returncode != 0:
        print("      cannot inspect it here; run this where the image is")
        FAIL.append("image: not inspectable")
    else:
        scan("image-config", meta.stdout.encode(), vals)
        hist = subprocess.run(["docker", "history", "--no-trunc", "--format", "{{.CreatedBy}}", image],
                              capture_output=True, text=True).stdout
        scan("image-history", hist.encode(), vals)
        with tempfile.TemporaryDirectory() as td:
            tar = os.path.join(td, "img.tar")
            if subprocess.run(["docker", "save", "-o", tar, image]).returncode != 0:
                FAIL.append("image: docker save failed")
            else:
                # Only layers this Dockerfile created can carry our secrets, but
                # scanning all of them costs nothing beyond time and removes the
                # need to be right about which is which.
                with tarfile.open(tar) as t:
                    for m in t.getmembers():
                        if not m.isfile():
                            continue
                        f = t.extractfile(m)
                        if f is None:
                            continue
                        if m.name.endswith((".tar", ".tar.gz", ".tgz")):
                            try:
                                with tarfile.open(fileobj=f) as inner:
                                    for im in inner.getmembers():
                                        if not im.isfile() or im.size > 80_000_000:
                                            continue
                                        g = inner.extractfile(im)
                                        if g is not None:
                                            scan(f"image-layer:{im.name}", g.read(), vals)
                            except tarfile.TarError:
                                pass
                        elif m.size < 4_000_000:
                            scan(f"image-meta:{m.name}", f.read(), vals)

print()
if FAIL:
    print("audit-secrets: FAILED -- a credential is reachable:")
    for f in sorted(set(FAIL)):
        print(f"  {f}")
    sys.exit(1)
print("audit-secrets: clean. No credential in history, tracked files, artifacts"
      + (", or image layers." if image else "."))
PY
