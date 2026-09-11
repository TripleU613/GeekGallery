# Security

## Reporting a vulnerability

Open a [GitHub issue](https://github.com/TripleU613/GeekGallery/issues) for
anything that isn't sensitive. For anything you'd rather not post publicly (a
real exploit against the live site, a credential leak, an auth bypass), email
tripleutech@gmail.com instead of filing an issue.

## What runs where

- **No secret is ever committed.** `.env` and `secrets.env` are gitignored;
  `.env.example` holds no real values. If you find one in the history anyway,
  treat it as compromised and say so in the report — it doesn't matter whether
  it's still "valid," it needs rotating.
- **No secret is baked into a Docker image layer.** Every credential the app
  needs (D1, R2, the tunnel token, the session key) arrives as a runtime
  environment variable injected by the CI deploy step (one blob per flavor,
  from the deploy-env broker -- see FLAVORS.md), never an `ENV`/`ARG` in the
  `Dockerfile` and never a file on the deploy host's disk. `docker history`
  on the published image should show no secret material — if it ever does,
  that's a real finding, not a style nit. `scripts/audit-secrets.sh --image`
  checks.
- **The app's Cloudflare tokens are scoped.** One token can only touch D1, the
  other only R2 (`scripts/cloudflare-provision.sh` mints them that way). The
  account-wide key that created them never leaves the operator's machine.
- **The server holds no application data.** The container runs `read_only`
  with no volumes. Items live in D1, media lives in R2. A compromised container
  has nothing local worth taking beyond whatever environment it was handed for
  that run.
- **User uploads are served from a separate origin** (each flavor's media host)
  and the same-origin download proxy sets `X-Content-Type-Options: nosniff`
  globally, so an mp4 container carrying HTML can never become same-origin
  script.
- **Uploaded files are validated by magic bytes, never by name or declared
  type.** Stills are re-encoded from decoded pixels (stripping EXIF and
  anything appended after the image data). GIFs and videos are stored as
  uploaded, which is a deliberate trade: re-encoding either without ffmpeg is
  impossible, and ffmpeg is a larger attack surface than serving bytes with the
  right `Content-Type` and `nosniff`.
- **Base images are pinned by digest**, not a movable tag — see the README's
  Deploy section for why and how to re-pin.
- **Moderation is after the fact.** Nothing screens an upload before it
  appears; the three-report auto-hide and the admin queue are the mechanism.
