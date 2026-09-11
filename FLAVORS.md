# Flavors

One build of this repository runs any number of galleries. Everything that
makes a deployment *a particular site* -- its name, what one item is called,
its colours, its like button, its logo, its domain, its credentials, its
mirror -- is a **flavor**: a single `KEY=VALUE` file, held as one secret, read
by the app at startup. Nothing about a site is in the code, the image or the
repository.

This file is the complete list of keys. `src/flavor.rs` (`Flavor::from_env`,
`Theme::from_env`) is what reads them; if the two disagree, the code is right
and this file has a bug.

## How a flavor reaches the app

```
FLAVOR_ENV_<NAME>  (Cloudflare Worker secret, deploy/env-broker/)
      │  POST /flavor/<name>, with GitHub's OIDC token for a push to main
      ▼
GitHub Actions deploy job (one per flavor, from the broker's /flavors list)
      │  ssh, as `export GALLERY_ENV=<the whole file>` on stdin
      ▼
/opt/geekgallery/<name>/docker-compose.yml  →  container env GALLERY_ENV
      │  main.rs: flavor::parse_blob, set_var for each key not already set
      ▼
std::env, then Flavor::from_env() once, then <script id="gg-flavor"> in every page
```

Locally there is no blob: put the same keys in `.env` (see `.env.example`)
and `cargo leptos watch` reads them the ordinary way. Every key has a default,
so an empty `.env` runs a grey gallery called "geekgallery" whose items are
"items".

Four keys are read by the **deploy job**, not the app, and are stripped from
what reaches the container: `DEPLOY_HOST`, `DEPLOY_HOST_KEY`, `TS_CI_AUTHKEY`,
`LEGACY_COMPOSE`.
One is lifted out for `cloudflared`, which is a separate process: `CF_TUNNEL_TOKEN`
becomes `TUNNEL_TOKEN`.

## Identity (`SITE_*`)

| key | default | what it is |
|---|---|---|
| `SITE_ORIGIN` | `http://127.0.0.1:3100` | `https://example.com`, no trailing slash. Every absolute URL the site emits (canonicals, sitemap, og:image, oEmbed) starts with it. |
| `SITE_NAME` | `geekgallery` | The site's name, exactly as written everywhere: `<title>`, `og:site_name`, the logo's alt text, the embed card's brand line, JSON-LD, the provenance written into stored images. Case is kept. |
| `SITE_NOUN` | `item` | What one thing in the gallery is called, lower case: `pic`, `frog`. Also the URL segment of every item page, `/<noun>/<slug>`, so an existing site's links keep working. Letters only. |
| `SITE_NOUN_PLURAL` | `<noun>s` | Its plural, when adding an `s` is wrong. Also an API alias: `/api/v1/<plural>` beside the canonical `/api/v1/items`. |
| `SITE_SUBJECT` | *(empty)* | What the items are *of*, for copy like "a Frog clip" and "memes of Frog". Empty drops the phrase ("a clip"). |
| `SITE_TAGLINE` | `A gallery for one joke, kept properly.` | One line under the name: the about page's subtitle, the home `<title>` after the name, the first words of `llms.txt`. |
| `SITE_DESCRIPTION` | *(a generic sentence)* | The home page's `<meta name="description">`, the web-app manifest's description, and part of the About/FAQ text. One or two sentences; search engines show ~155 characters. |
| `SITE_ABOUT` | *(empty)* | The About page's opening paragraphs, in the site's own voice. Paragraphs are separated by a literal `\n` (backslash, n) because the file is one line per key. Empty gets one honest paragraph built from the name and noun. |
| `SITE_ALTERNATE_NAMES` | *(empty)* | Comma-separated other spellings people search for (`frog memes, the frog gallery`), for JSON-LD `alternateName` and `llms.txt`. |
| `SITE_CONTACT_EMAIL` | *(empty)* | The DMCA designated agent shown on `/dmca`. Empty makes the page point at the report flag instead of printing an address. |
| `SITE_REPO_URL` | this repository | The header's GitHub button. Empty hides the button. |
| `SITE_SISTER_SITES` | *(empty)* | Footer links to other galleries: `name=url, name=url`. |

## The like button (`SITE_LIKE_*`)

Every gallery has a favourite action; only its glyph and its words differ.
`{noun}` and `{nouns}` in the words are filled in.

| key | default | what it is |
|---|---|---|
| `SITE_LIKE_ICON` | `heart` | One of `heart`, `mic`, `droplet`, `star`, `thumbs_up`, `flame`, `zap`, `laugh`. Anything else is a heart. |
| `SITE_LIKE_VERB` | `Like this {noun}` | The button's label when not yet liked. Also, lower-cased, the sign-in prompt ("Sign in: give this pic a mic"). |
| `SITE_LIKE_UNDO` | `Unlike` | The label when already liked. |
| `SITE_LIKE_SORT_LABEL` | `Most liked` | The gallery's sort option for the like count. |

Two worked examples: `mic` / `Give this {noun} a mic` / `Take the mic back` /
`Most mics`; or `droplet` / `Cry over this {noun}` / `Un-cry over this {noun}` /
`Most cried over`.

## Colours (`THEME_*`)

`#rrggbb` or `#rgb`. Every token becomes a CSS variable on `:root`, and every
Tailwind colour utility reads the variable, so the whole UI recolours with no
rebuild. A value that does not parse is ignored and the default stands.

| key | default | what it is |
|---|---|---|
| `THEME_BG` | `#0a0b0f` | The page. |
| `THEME_SURFACE` | `#10121a` | Cards, fields. |
| `THEME_SURFACE_RAISED` | `#171a25` | Menus, chips, skeletons. |
| `THEME_SURFACE_HOVER` | `#1e2230` | Hover fill. |
| `THEME_LINE` | `#2b3042` | Hairlines. |
| `THEME_LINE_STRONG` | `#3d4358` | Hovered hairlines. |
| `THEME_INK` | `#f2f4f8` | Text. |
| `THEME_INK_2` | `#aab0c0` | Secondary text. Keep 4.5:1 on the surfaces. |
| `THEME_INK_3` | `#7f8698` | Bylines, timestamps. Keep 4.5:1 on the surfaces. |
| `THEME_ACCENT` | `#9aa4ff` | The brand colour. The hairline, wash and border tints are fixed alphas of it. |
| `THEME_ACCENT_HOVER` | derived | Lighter accent. Derived from `THEME_ACCENT` when unset. |
| `THEME_ACCENT_ACTIVE` | derived | Darker accent. Derived when unset. |
| `THEME_ACCENT_MUTED` | derived | Accent as body text. Derived when unset. |
| `THEME_ACCENT_INK` | `#0a0b0f` | Text on a filled accent button. |
| `THEME_DANGER` | `#ff6b7a` | |
| `THEME_OK` | `#4ee3a0` | |

The default is a cool near-black palette with a periwinkle accent. A warm
one, for contrast:
`THEME_BG=#0c0b08 THEME_SURFACE=#13120d THEME_SURFACE_RAISED=#1b1912
THEME_SURFACE_HOVER=#232017 THEME_LINE=#383429 THEME_LINE_STRONG=#4e4839
THEME_INK=#f7f6f3 THEME_INK_2=#b8b4a8 THEME_INK_3=#908a7a THEME_ACCENT=#ffcc33
THEME_ACCENT_INK=#0a0a0b`.

## Brand art (`SITE_ASSETS_*`, `SITE_LOGO_*`)

Six files at fixed names, wherever `SITE_ASSETS_BASE` points:
`logo.png` (the header), `logo-large.png` (1200x630, the link preview),
`favicon-32.png`, `favicon-192.png`, `favicon-512.png`, `apple-touch-icon.png`.

| key | default | what it is |
|---|---|---|
| `SITE_ASSETS_BASE` | `/brand` | URL prefix of the six files, no trailing slash. The default is this repository's placeholder art under `public/brand/`; a real site uploads its own to a folder on its media bucket (`https://media.example.com/brand`). |
| `SITE_ASSETS_VERSION` | *(empty)* | Appended as `?v=` to every art URL. Bump it when the files change under the same names; they are cached for a day. |
| `SITE_LOGO_WIDTH` / `SITE_LOGO_HEIGHT` | `96` / `96` | `logo.png`'s intrinsic size, so the header does not reflow while it loads. It is shown 28-32px tall. |
| `SITE_LOGO_HAS_NAME` | `false` | `true` when `logo.png` is a wordmark that already spells the name; the header then does not print the name in text beside it. |

No art yet? `cargo run --example brand_art --features ssr -- --accent '#hex' --bg '#hex' --out some/dir`
draws the six files from two colours.

## Infrastructure

| key | required | what it is |
|---|---|---|
| `CF_ACCOUNT_ID` | yes | Cloudflare account. |
| `CF_D1_DATABASE_ID` | yes | The flavor's D1 database. One per flavor. |
| `CF_D1_API_TOKEN` | yes | A token that can only touch D1. |
| `R2_ACCESS_KEY_ID` / `R2_SECRET_ACCESS_KEY` | production | S3 credentials for R2. Unset, uploads go to `./uploads` on disk (dev only). |
| `R2_BUCKET` | production | The flavor's bucket. One per flavor. |
| `R2_PUBLIC_BASE` | production | `https://media.example.com`, the bucket's public domain. |
| `SESSION_SECRET` | yes | ≥32 bytes; `openssl rand -base64 48`. Encrypts session cookies. |
| `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` | no | Google sign-in. The client must list `<SITE_ORIGIN>/auth/google/callback`. Absent, the sign-in link explains itself. |
| `ADMIN_EMAILS` | no | Comma-separated. Re-applied at every login. |
| `PSEUDONYMS` | no | `email=Name` or `email=Name\|https://avatar`, comma-separated: publish someone under a different name. |
| `CF_ANALYTICS_TOKEN` | no | Cloudflare Web Analytics beacon. Absent, no script. |
| `INDEXNOW_KEY` | no | 8-128 alphanumerics. Pings IndexNow on every publish. |
| `TURNSTILE_SITE_KEY` / `TURNSTILE_SECRET` | no | The captcha in front of uploads and imports. Both or neither. |
| `CF_TUNNEL_TOKEN` | production | cloudflared's. Lifted out of the blob by CI as `TUNNEL_TOKEN`. |
| `RUST_LOG` | no | Set by compose to `info`. |
| `FFMPEG_DIR`, `YTDLP_BIN`, `IMPORT_COOKIES_FILE`, `INSTAGRAM_SESSIONID` | no | The link importer's tools and credentials; see `.env.example`. The image has the tools on PATH. |
| `REPAIR_COVERS` | no | Set to `1` to redraw covers stored black on the next start. |

## The mirror (`MIRROR_*`)

An hourly pull from another gallery, entirely inside the blob, so one flavor
can be kept in step with a site that does not run this code and another can
have no mirror at all.

| key | default | what it is |
|---|---|---|
| `MIRROR_SITEMAP` | *(off)* | The other site's sitemap. A leaf (listing pages) or an index (listing sitemaps; up to 20 children are followed). Set, the mirror is on. |
| `MIRROR_MATCH` | *(any)* | A substring a link must contain to count, e.g. `/en/gallery/`. |
| `MIRROR_EVERY_MINS` | `60` | Floor 15. |
| `MIRROR_PER_RUN` | `40` | How many new links one pass takes on. |

## Deploy (read by CI, never reaches the container)

| key | required | what it is |
|---|---|---|
| `DEPLOY_HOST` | yes | The Tailscale hostname of the machine this flavor runs on. Flavors may share one or each have their own. |
| `DEPLOY_HOST_KEY` | yes | That machine's ssh host key, e.g. `ssh-ed25519 AAAA...` (`ssh-keyscan -t ed25519 <host>`). Pinned in `known_hosts` by the deploy job, so the connection is never trust-on-first-use. |
| `TS_CI_AUTHKEY` | yes | A reusable Tailscale auth key for `tag:ci`, whose SSH policy lets the runner in as root on `DEPLOY_HOST`. |
| `LEGACY_COMPOSE` | no | The compose file of a stack this flavor replaces (an earlier single-site deploy of the same site). Its containers are removed and the file renamed `.retired` before the first deploy, so two `cloudflared` never share one tunnel. Harmless once it no longer exists. |

## Adding a site

1. `CF_ZONE=example.com FLAVOR=example scripts/cloudflare-provision.sh > example.env`
   makes the D1 database, the bucket and its media domain, two scoped tokens,
   the tunnel and its DNS, and prints the infrastructure keys above.
2. `scripts/d1-migrate.sh example` applies the schema.
3. Add the `SITE_*`, `THEME_*` and `SITE_LIKE_*` lines, `DEPLOY_HOST`,
   `DEPLOY_HOST_KEY` and `TS_CI_AUTHKEY`; draw or upload the art; create the
   Google client and the Turnstile widget if wanted.
4. `cd deploy/env-broker && npx wrangler secret put FLAVOR_ENV_EXAMPLE < example.env`
5. Set the `BROKER_URL` repository variable to the broker's URL (once per
   repository), then push to `main` or run the `ci` workflow from the Actions
   tab. The deploy matrix now has one more row.
