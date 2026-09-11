# syntax=docker/dockerfile:1
#
# One image, two processes: the site and cloudflared, under supervisor. No
# Python, no browser, no sidecar -- the site this grew out of carried a hosted
# screening model and the headless browser that kept it signed in, and both are
# gone. What is left is a Rust binary, its static assets, and the tunnel.
#
# Nothing in here is a secret and nothing in here becomes one: every credential
# arrives as a runtime environment variable from the CI deploy step (see
# .github/workflows/ci.yml), never as a build argument or an ENV. The audit
# (scripts/audit-secrets.sh --image) exists to keep that a fact.
#
# Base images are pinned by digest as well as by tag, so a rebuild months from
# now produces the same layers. Re-pin deliberately (scripts/build-check.sh
# checks that a pinned digest still resolves).

# ---- build ------------------------------------------------------------------
FROM rust:1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS builder

RUN rustup target add wasm32-unknown-unknown \
    && cargo install cargo-leptos --locked

# Tailwind's standalone CLI, so the image builds without Node. Pinned by
# version and checksum: it is an executable downloaded from GitHub.
ARG TAILWIND_VERSION=3.4.19
ARG TAILWIND_SHA256=4af3198c015616ea7d6617974ec3d70d987ecc00c1ca8463b0a30fd65cc7c06e
RUN curl -fsSL -o /usr/local/bin/tailwindcss \
      "https://github.com/tailwindlabs/tailwindcss/releases/download/v${TAILWIND_VERSION}/tailwindcss-linux-x64" \
    && echo "${TAILWIND_SHA256}  /usr/local/bin/tailwindcss" | sha256sum -c - \
    && chmod +x /usr/local/bin/tailwindcss \
    && tailwindcss --help | head -1

WORKDIR /build

# Dependencies first, against stub sources, so a source-only change does not
# recompile the world. Both targets, since the wasm build has its own tree.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/components src/storage \
    && echo 'fn main() {}' > src/main.rs \
    && echo '' > src/lib.rs \
    && cargo build --release --no-default-features --features ssr \
    && cargo build --release --target wasm32-unknown-unknown --no-default-features --features hydrate --lib \
    && rm -rf src

COPY src ./src
COPY style ./style
COPY public ./public
COPY tailwind.config.js ./
# The stubs above have newer mtimes than the real sources cargo is about to
# see; touching the roots makes it rebuild the crate rather than trust them.
RUN touch src/main.rs src/lib.rs
RUN cargo leptos build --release

# ---- run --------------------------------------------------------------------
FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl supervisor xz-utils unzip \
    && rm -rf /var/lib/apt/lists/*

# The link importer's three helpers, each a single static binary pinned by
# hash like cloudflared below. They are only ever run as subprocesses by
# `src/fetch.rs`, and only for links on the platforms it allowlists.
#
# - yt-dlp: the one tool that keeps up with YouTube, TikTok, Reddit and the
#   rest. The `_linux` build bundles its own Python, so the image stays free
#   of a system interpreter.
# - deno: the JavaScript runtime yt-dlp needs to solve YouTube's player
#   challenges. Without it most YouTube formats are refused.
# - ffmpeg/ffprobe: merges the separate video and audio streams the big sites
#   serve into one MP4, and draws the poster frame for an imported clip (the
#   browser is not in the loop to do it, as it is for an upload).
ARG YTDLP_VERSION=2026.08.19
ARG YTDLP_SHA256=58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a
RUN curl -fsSL -o /usr/local/bin/yt-dlp \
      "https://github.com/yt-dlp/yt-dlp/releases/download/${YTDLP_VERSION}/yt-dlp_linux" \
    && echo "${YTDLP_SHA256}  /usr/local/bin/yt-dlp" | sha256sum -c - \
    && chmod +x /usr/local/bin/yt-dlp \
    && yt-dlp --version

ARG DENO_VERSION=2.9.6
ARG DENO_SHA256=394f07f4da2bebe6ce6f1e7ce0fa16429b29b08c35e3fac3fe25972676dff4b2
RUN curl -fsSL -o /tmp/deno.zip \
      "https://github.com/denoland/deno/releases/download/v${DENO_VERSION}/deno-x86_64-unknown-linux-gnu.zip" \
    && echo "${DENO_SHA256}  /tmp/deno.zip" | sha256sum -c - \
    && unzip -q /tmp/deno.zip -d /usr/local/bin && chmod +x /usr/local/bin/deno && rm /tmp/deno.zip \
    && deno --version | head -1

# johnvansickle's static build. The URL is not versioned (the archive inside
# is: ffmpeg-7.0.2), so the hash is the pin; when upstream moves the file the
# build fails here, loudly, and the two ARGs get updated together.
ARG FFMPEG_SHA256=abda8d77ce8309141f83ab8edf0596834087c52467f6badf376a6a2a4c87cf67
RUN curl -fsSL -o /tmp/ffmpeg.tar.xz \
      "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz" \
    && echo "${FFMPEG_SHA256}  /tmp/ffmpeg.tar.xz" | sha256sum -c - \
    && tar -xJf /tmp/ffmpeg.tar.xz -C /tmp \
    && install -m 0755 /tmp/ffmpeg-*-amd64-static/ffmpeg /tmp/ffmpeg-*-amd64-static/ffprobe /usr/local/bin/ \
    && rm -rf /tmp/ffmpeg* \
    && ffmpeg -version | head -1

# cloudflared, pinned like Tailwind above.
ARG CLOUDFLARED_VERSION=2026.7.3
ARG CLOUDFLARED_SHA256=9d71c677db00134c1bd4144b7783486b654ad281b1ea62b4972098d19f770f17
RUN curl -fsSL -o /usr/local/bin/cloudflared \
      "https://github.com/cloudflare/cloudflared/releases/download/${CLOUDFLARED_VERSION}/cloudflared-linux-amd64" \
    && echo "${CLOUDFLARED_SHA256}  /usr/local/bin/cloudflared" | sha256sum -c - \
    && chmod +x /usr/local/bin/cloudflared \
    && cloudflared --version

RUN useradd --system --uid 10001 --create-home --home-dir /app app
WORKDIR /app

COPY --from=builder /build/target/release/geekgallery ./geekgallery
COPY --from=builder /build/target/site ./site
COPY --from=builder /build/target/release/hash.txt ./hash.txt
COPY deploy/supervisord.conf /etc/supervisor/supervisord.conf

RUN chown -R app:app /app
USER app

# Public configuration only. Every secret is runtime environment from compose.
ENV LEPTOS_SITE_ADDR=0.0.0.0:3100 \
    LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_HASH_FILES=true

EXPOSE 3100

# A TCP connect to the app's port and nothing more: the deploy script gates the
# rollout on this, and "the site answers" is the only claim worth rolling back
# over.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD bash -c 'echo > /dev/tcp/127.0.0.1/3100' || exit 1

ENTRYPOINT ["supervisord", "-c", "/etc/supervisor/supervisord.conf"]
