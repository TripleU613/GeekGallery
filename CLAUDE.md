# Traps

Things that have already cost real time in this codebase. Not documentation of
how the code works -- a list of ways it has been broken. Short because each
entry was paid for.

## Flavors

- **The flavor is read from `flavor::get()` on both sides, never from `env`
  in a component.** The server installs it from the environment in `main`,
  the wasm reads it back out of `<script id="gg-flavor">` in `hydrate()`, and
  the two are byte-identical -- which is what lets copy like "No pic here"
  be server-rendered and hydrated without a mismatch. A `std::env::var` in a
  component is `None` in wasm and a guaranteed mismatch.
- **Item pages live at `/{noun}/:slug`, and the noun comes from the flavor.**
  Never write `"/item/"` as a literal: use `Flavor::item_path` /
  `item_prefix`. The one place the noun appears as a route string is the
  download route in `main.rs`, written with a `{noun}` placeholder on purpose
  so `tests/router_links.rs` still parses it.
- **`GALLERY_ENV` is unpacked with `set_var` before anything reads env.**
  Anything that caches an environment value in a `OnceLock` (the storage
  backend, the site origin) must therefore be constructed after that block in
  `main`, or it caches the empty default.
- **Every colour is `rgb(var(--c-x) / <alpha>)`.** A hex literal in a
  component or in `tailwind.config.js` is a colour the flavor cannot change;
  the embed card's CSS is built at runtime from the theme for the same
  reason.

## Leptos / hydration

- **Never write a signal during render.** A signal write in a component body or
  a view closure can leave the server HTML and the client's first hydration
  pass disagreeing, and that does not degrade -- it kills the whole wasm module.
  Writes go in `Effect`s (client-only by construction) or event handlers. The
  one exception is *creating* a signal and providing context, which is fine.
- **Nothing that differs between server and client may seed initial state:**
  no `window.innerWidth`, no `localStorage`, no `navigator` probes, no clocks.
  Seed from the URL or from constants, then flip in an `Effect`.
- **`<Link>` from leptos_meta is not reactive.** Its `href` is an `Oco<str>`,
  not a closure. `<Title>` and `<Meta>` accept closures; `<Link>` does not.
- **`leptos_meta` resolves duplicate tags first-set-wins.** A site-wide default
  `og:image` in `App` silently beats every page's own. `SitePreview` is opt-in
  per page for exactly this reason.
- **Every `<a>` to an Axum route needs `rel="external"` or `download`,** or
  leptos_router intercepts the click and renders the 404 page. Sign-in, the
  download button, and logout have all been broken this way. `tests/
  router_links.rs` parses `main.rs` and checks every anchor in `src/`.
- **`ItemDetail`'s view! needs `#![recursion_limit = "512"]`** in both lib.rs
  and main.rs -- release-mode monomorphization hits the default query depth,
  and debug builds never show it.
- **`Suspense` fallbacks are `ViewFn`s, not tracking closures.** A `.get()`
  inside one subscribes to nothing and warns at runtime. Use `get_untracked`.
- **`NodeRef` is `Copy`.** `[NodeRef::new(); N]` hands the same ref to every
  slot. Write each `NodeRef::new()` out.
- **A captured `String` in a view closure makes it `FnOnce`.** Use
  `StoredValue` for ids read from inside reactive click handlers.
- **Media events fire before hydration attaches listeners.** A server-rendered
  `<video preload="metadata">` has usually fired `loadedmetadata` by the time
  the wasm is up, so `on:loadedmetadata` alone leaves state stale. The
  player's mount `Effect` reads duration/buffered/time off the element too.
- **A `muted` attribute set client-side does not mute.** Only the property
  does, and unmuted autoplay is refused. `player.rs` calls `set_muted(true)`
  then `play()` in an `Effect`; do not go back to `<video autoplay muted>`.
- **iPhone Safari decodes nothing for a paused `<video>` until it has played.**
  The upload preview must `autoplay muted playsinline` (the cover pick pauses
  it afterwards); a canvas draw of a never-played clip is solid black, and
  five of the first eight clips shipped with that as their cover. The client
  refuses a flat frame, the server replaces one with ffmpeg's, and `repair`
  redraws any already stored.
- **A block-level box inside the detail `<figure>` needs an explicit width.**
  The figure is a flex item sized from its content; an `<img>` brings a
  width, a `<div>` does not, and `w-full` alone collapsed it to its padding.

- **Turnstile tokens are single-use and the server spends them.** The upload
  page resets the widget after every answer from `/api/upload` or
  `/api/import`, success or not; a second submit with the same token is
  refused as `timeout-or-duplicate`. The script is appended from an `Effect`
  only when the server reports a site key, so a dev box never loads it.

## Tailwind

- **The scanner reads `.rs` files as raw text, comments included.** Classes
  built with `format!` from fragments are never generated; whole literal
  strings always are -- even in a comment, which means a class name quoted in a
  comment keeps a dead rule alive.
- **Never layer a utility on a primitive that already sets that property**
  (`.btn`, `.icon-btn`, `.card`, `.field`...). Two rules at equal specificity
  are decided by stylesheet order. Write the whole class string per state.
- **`LEPTOS_HASH_FILES` is read at build time by cargo-leptos.** `watch` with it
  on serves stale CSS forever; the Dockerfile turns it on for release.

## D1

- **There is no cross-request transaction.** A `batch` is atomic but fixed in
  advance. `toggle_like` is written the way it is (INSERT ... RETURNING, then
  recompute from COUNT) because of this, not by preference.
- **Verify every non-trivial SQL shape against live D1 before relying on it.**
  CTEs inside scalar subqueries, FTS5 `'delete'` commands, trigram tokeniser
  minimums -- each was checked, not assumed.
- **`ADMIN_EMAILS` and `PSEUDONYMS` are re-applied at every login.** A manual
  `UPDATE users` is reverted the next time that person signs in.
- **Apply migrations with wrangler, not the raw HTTP API.** Triggers contain
  semicolons; a naive split sends half a trigger.

## Media

- **Uploaded videos are never decoded on the server.** Width, height, duration
  and the poster frame come from the browser, which already has the clip open.
  ffmpeg *is* in the image now, for the link importer (it merges yt-dlp's
  streams and draws the poster for a clip no browser ever saw); do not route
  uploads through it -- the browser's frame is free, the server's is not.
- **`fetch.rs` is the only code allowed to make outbound requests to a
  visitor-supplied URL,** and only through `guarded_get`, which resolves the
  host, refuses anything not publicly routable, and pins the connection to
  the checked addresses. yt-dlp is pointed only at hosts in `PLATFORMS`.
- **The original's object key carries its extension** (`orig/<id>.mp4`), the
  thumbnail is always `thumb/<id>.jpg`. `storage::all_keys` lists every key an
  item could have, so a delete never needs a lookup. Changing a key format
  orphans every object already in R2.
- **GIFs are stored byte-for-byte.** Re-encoding keeps one frame.
- **A JPEG source stays a JPEG (`orig/<id>.jpg`); PNG and WebP become PNG.**
  Only the PNG originals carry the LSB watermark -- it cannot survive JPEG.
  `Item::mime()` reads the kind off the URL's extension, so a new stored
  extension means a new arm there *and* a new entry in `storage::all_keys`.
- **MP4s are reordered on intake (`faststart`), never re-encoded.** `relocate`
  returning `None` means "store as uploaded" and is not an error; do not turn
  it into one.

## Testing in the sandbox

- **The Playwright Chromium here has no H.264.** An MP4 clip loads with
  `videoWidth == 0` and never plays; that is the codec, not the player. Test
  with a WebM (record one from a canvas with `MediaRecorder`, remux with the
  bundled ffmpeg so it has a duration and cues) served with `Range` support.
- **The bundled ffmpeg is WebM-only as well.** `/opt/pw-browsers/ffmpeg-*/
  ffmpeg-linux` is Playwright's screen-recording build: `-demuxers` lists
  Matroska and nothing else. Point `FFMPEG_DIR` at it, import an mp4, and the
  poster step fails with "Invalid data found when processing input" and the
  clip falls back to the placeholder cover -- which looks exactly like the
  iOS black-cover bug and is not it. A poster drawn from an mp4 cannot be
  tested in this sandbox at all; the release image installs a full static
  build, and `repair` redraws anything that landed on the placeholder.
- **A flavor's media host is unreachable from the sandbox's browser.** Route
  it in Playwright to local files; `curl` (which honours the proxy) can fetch
  the real poster for the route to serve.

## Deploy

- **Pinned image digests must be fetched with the right `Accept` headers** or
  you get a real digest for the wrong manifest and a build that fails with a
  bare "not found". The pre-push hook checks every pinned digest resolves.
- **The `/tmp` tmpfs needs `exec`.** Docker mounts a tmpfs `noexec` by
  default; yt-dlp is a PyInstaller binary that extracts its `.so` files under
  `/tmp` and `dlopen`s them, and `noexec` turns that into "failed to map
  segment from shared object". Every link import through yt-dlp failed that
  way until the compose file said `/tmp:size=384m,exec`.
- **`--remove-orphans` on `compose up` is load-bearing:** a renamed service
  leaves the old one running, and two `cloudflared` on one tunnel token means
  Cloudflare load-balances between two origins. The same failure waits when a
  flavor replaces an older stack on the same host: `LEGACY_COMPOSE` in the
  flavor exists so the old stack is taken down before the new one starts.
- **`docker compose -f <old file> down` fails when the file's `${IMAGE}`
  variable is not in the shell** -- compose refuses a service with no image
  -- and `|| true` hides it. That once retired a legacy compose file and left
  the old container running beside the new one on the same tunnel.
  `remote-deploy.sh` removes legacy containers by their compose-project
  label; never trust `down` alone.
- **An error response must never carry a long `Cache-Control`.** The cache
  middleware once stamped `immutable` on a 404 for a hashed stylesheet (an
  old container answering for a new file), and the edge served that 404 for
  the CSS long after the origin was fixed. Errors get `no-store`; a zone
  purge is the only way out once it has happened.
- **Deleting an object from R2 does not delete it from the edge.** Media is
  served `immutable, max-age=31536000`; after an admin delete every edge that
  had the thumbnail kept answering `HIT 200` with it, and would have for a
  year. `admin_delete_item` purges the item's URLs through `edge_cache.rs`
  when `CF_ZONE_ID`/`CF_CACHE_PURGE_TOKEN` are in the flavor; without them the
  only remedy is a purge by hand.
- **Cache-Control on HTML is `private, no-cache`** because the served body
  contains per-visitor state (account menu, which items you have liked). Do
  not relax it for edge caching without moving that state out of the SSR'd
  body first.
- **The zone's plan allows exactly one rate-limit rule.** It covers both
  `POST /api/upload` and `POST /api/import` in one expression; adding a second
  rule fails with "2 out of 1", so widen the expression instead.
- **Bot Fight Mode and Hotlink Protection stay off** at Cloudflare. Unfurlers
  are bots and unfurling is the product; embeds are hotlinks by design.
