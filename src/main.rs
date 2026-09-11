//! Server entry point.

// See the matching attribute + comment in lib.rs: ItemDetail's view! hits
// rustc's default query depth limit in release mode, and this binary crate
// hits the same wall independently of the lib crate.
#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use axum::routing::{get, post};
    use axum::Router;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use tower_http::services::ServeDir;

    use geekgallery::app::{shell, App};

    let _ = dotenvy::dotenv();
    // The deploy hands the whole configuration over as one KEY=VALUE blob in
    // GALLERY_ENV rather than as forty separate variables (see FLAVORS.md).
    // Unpacked into the process environment before anything reads it, and
    // without overriding a variable that is already set, so a value exported
    // beside the blob still wins.
    if let Ok(blob) = std::env::var("GALLERY_ENV") {
        let mut n = 0;
        for (k, v) in geekgallery::flavor::parse_blob(&blob) {
            if std::env::var_os(&k).is_none() {
                std::env::set_var(&k, &v);
                n += 1;
            }
        }
        tracing::debug!("GALLERY_ENV: {n} variable(s) unpacked");
    }
    // The flavor, once, before anything renders or logs the site's name.
    geekgallery::flavor::set(geekgallery::flavor::Flavor::from_env());
    let flavor = geekgallery::flavor::get();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // D1 only — no local database. Local dev talks to the same D1 database as
    // production rather than falling back to a divergent local store, so there
    // is exactly one code path to trust (see db.rs).
    let d1 = geekgallery::d1::D1::from_env()
        .map_err(|e| anyhow::anyhow!("D1 is not configured: {e}"))?;
    geekgallery::db::set_client(d1);
    let existing = geekgallery::db::count_public().await?;
    tracing::info!(
        "{} ({}) -- database ready (D1): {existing} {} collected",
        flavor.name,
        flavor.origin,
        flavor.nouns
    );

    // R2 when configured, local disk otherwise. Logged either way so it is never
    // a mystery which one served a given upload.
    let storage = geekgallery::storage::backend_from_env().await;
    tracing::info!("storage backend: {}", storage.name());
    geekgallery::storage::set_backend(storage);
    // Covers stored black by an earlier upload page get redrawn once the site
    // is up (see `repair`). Off the request path, and a no-op when clean.
    tokio::spawn(geekgallery::repair::covers());
    // An hourly pull from another gallery, when `MIRROR_SITEMAP` names one.
    // Logs which, or that it is off, so it is never a mystery either.
    geekgallery::mirror::spawn();

    if geekgallery::auth::google_configured() {
        tracing::info!("Google sign-in: configured");
    } else {
        tracing::info!("Google sign-in: not configured (GOOGLE_CLIENT_ID/SECRET unset) — /auth/google/login will redirect back with an error");
    }

    // cargo-leptos supplies site-addr and site-root through Cargo.toml metadata.
    let conf = get_configuration(None)?;
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;
    let routes = generate_route_list(App);

    let noun = flavor.noun.as_str();
    let mut app = Router::new();
    // IndexNow ownership proof: `/<INDEXNOW_KEY>.txt`, registered as an exact
    // path from the environment. Not `/{key}.txt`: axum allows one parameter
    // per segment and nothing beside it, and a bare `/{key}` would shadow the
    // styled 404 for every unknown single-segment path.
    if let Some(key) = geekgallery::indexnow::key() {
        tracing::info!("indexnow: key file served at /{key}.txt");
        app = app.route(&format!("/{key}.txt"), get(geekgallery::indexnow::key_file));
    }
    let app = app
        // Server functions need their own mount; `.leptos_routes` only registers
        // page routes. Without this, SSR still works (it calls the fns directly
        // in-process) but every client-side call 404s.
        //
        // Declared before the wildcard so the static path wins the match.
        // axum caps request bodies at 2MB by default, and the Multipart
        // extractor hits that ceiling before any of this app's own limits are
        // consulted, with an error that names no size.
        //
        // Raised to MAX_UPLOAD_BYTES (the video limit) plus two megabytes of
        // slack, because the body carries multipart boundaries, the title, and
        // for a video the JPEG poster the browser drew, on top of the file
        // itself. `storage::decode` still enforces the real per-kind limits and
        // still produces the message a visitor sees; this only stops the
        // extractor refusing the request before that code can run.
        .route(
            "/api/upload",
            post(geekgallery::upload_route::upload).layer(axum::extract::DefaultBodyLimit::max(
                geekgallery::storage::MAX_UPLOAD_BYTES + 2 * 1024 * 1024,
            )),
        )
        // A item from a pasted link. Small JSON body; the fetch happens in the
        // background and reports through the same job registry as an upload.
        .route("/api/import", post(geekgallery::import_route::import))
        .route(
            "/api/upload/status/{id}",
            get(geekgallery::upload_route::status),
        )
        .route("/auth/google/login", get(geekgallery::oauth_route::login))
        .route(
            "/auth/google/callback",
            get(geekgallery::oauth_route::callback),
        )
        .route("/auth/logout", post(geekgallery::oauth_route::logout))
        // A stable, documented public API -- unlike the server fns below,
        // whose paths are hashed and change on every rebuild.
        .route("/api/v1/items", get(geekgallery::public_route::list_items))
        .route(
            "/api/v1/items/{id}",
            get(geekgallery::public_route::get_item),
        )
        .route("/oembed", get(geekgallery::public_route::oembed))
        // A framable card for one item. A plain route, not a Leptos one:
        // `generate_route_list(App)` knows nothing about `/embed`, so without
        // this line it falls through to `file_and_error_handler` and 404s.
        .route("/embed/{id}", get(geekgallery::public_route::embed))
        // `/{noun}/{id}/download`: the download sits under the item's own
        // page path, whatever the flavor calls one. Written with the noun
        // as a format placeholder so `tests/router_links.rs`, which reads
        // this file as text, still sees a route pattern here.
        .route(
            &format!("/{noun}/{{id}}/download"),
            get(geekgallery::public_route::download),
        )
        // The per-flavor manifest and the API alias under the flavor's own
        // plural (`/api/v1/<nouns>`), because that is what someone guessing the
        // API will type. Both skipped when the noun would collide with a
        // path that already exists.
        .route(
            "/manifest.webmanifest",
            get(geekgallery::public_route::manifest),
        )
        // Bulk link import, admin only. A plain route rather than a Leptos
        // page (see the module for why), so `generate_route_list` knows
        // nothing about it and it has to be declared here or it 404s -- the
        // same arrangement `/embed/:id` needs.
        .route(
            "/admin/import",
            get(geekgallery::admin_import::page).post(geekgallery::admin_import::submit),
        )
        .route("/robots.txt", get(geekgallery::seo_route::robots_txt))
        .route("/sitemap.xml", get(geekgallery::seo_route::sitemap_xml))
        .route("/llms.txt", get(geekgallery::seo_route::llms_txt))
        .route(
            "/api/{*fn_name}",
            axum::routing::any(leptos_axum::handle_server_fns),
        );
    let app = if flavor.nouns != "items" {
        app.route(
            &format!("/api/v1/{}", flavor.nouns),
            get(geekgallery::public_route::list_items),
        )
        .route(
            &format!("/api/v1/{}/{{id}}", flavor.nouns),
            get(geekgallery::public_route::get_item),
        )
    } else {
        app
    };
    let app = app
        // Uploaded media is served straight off disk in local dev; R2 serves its
        // own origin in production and this directory stays empty.
        .nest_service("/uploads", ServeDir::new(geekgallery::storage::UPLOAD_ROOT))
        .leptos_routes(&leptos_options, routes, {
            let opts = leptos_options.clone();
            move || shell(opts.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        // After `.fallback`, before `.with_state`. `Router::layer` wraps every
        // route registered *so far*, fallback included -- and the fallback is
        // what serves `/pkg/` and everything in `public/`, i.e. exactly the
        // responses this layer exists to put a `Cache-Control` on.
        // `.route_layer` would skip the fallback and silently cache nothing.
        //
        // The hashing flag comes from `leptos_options`, never from a second
        // `std::env::var("LEPTOS_HASH_FILES")` read. One value decides both
        // which filenames the HTML asks for and whether those filenames are
        // safe to cache forever; splitting it is the two-switch trap in
        // CLAUDE.md, and getting it wrong in this direction is unrecoverable
        // from the origin, because an `immutable` client never revalidates.
        .layer(axum::middleware::from_fn_with_state(
            leptos_options.hash_files,
            cache::headers,
        ))
        .with_state(leptos_options);

    tracing::info!("{} listening on http://{addr}", flavor.name);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

/// `cargo-leptos` also builds the lib for wasm, where there is no main to run.
#[cfg(not(feature = "ssr"))]
fn main() {}

/// One `Cache-Control` decision per route class, in one place.
///
/// Before this existed, `curl -sI /` and `curl -sI /pkg/geekgallery.css`
/// both came back with no `Cache-Control` at all -- every response was at the
/// mercy of whatever the CDN in front chose to do, which is how a four-hour
/// edge TTL on `/pkg/` once served yesterday's CSS against today's HTML.
///
/// Cloudflare sits in front of this and can still override it: `s-maxage`
/// only binds if the zone is configured to respect origin headers, so the
/// header that actually has to be verified at the edge (not just here) is
/// `private, no-cache` on HTML.
#[cfg(feature = "ssr")]
mod cache {
    use axum::extract::State;
    use axum::http::{header, HeaderName, HeaderValue, Method, Request};
    use axum::middleware::Next;
    use axum::response::Response;

    /// Content-hashed under `hash-files = true`, so the filename changes
    /// whenever the bytes do and the old one is never requested again. A year
    /// plus `immutable` (no revalidation at all, even on reload) is the whole
    /// point of paying for hashed filenames.
    const IMMUTABLE: &str = "public, max-age=31536000, s-maxage=31536000, immutable";

    /// `X-Robots-Tag`, which `http` has no constant for.
    const X_ROBOTS_TAG: HeaderName = HeaderName::from_static("x-robots-tag");

    /// The class of a path, as a `Cache-Control` value.
    ///
    /// `hashed` is `LeptosOptions::hash_files`, i.e. the same value that
    /// decides which filenames the HTML asks for. With it off -- which is what
    /// `cargo leptos watch` does, deliberately -- `/pkg/geekgallery.css` is a
    /// fixed name whose contents change on every edit, and `immutable` there
    /// would freeze a stale stylesheet in every visitor's browser for a year
    /// with no way to reach it from the origin.
    pub fn cache_control(path: &str, hashed: bool) -> &'static str {
        let item_prefix = geekgallery::flavor::get().item_prefix();
        if path.starts_with("/pkg/") {
            return if hashed { IMMUTABLE } else { "no-cache" };
        }
        // Local-disk storage only (R2 serves its own origin in production).
        // An upload's bytes never change: the id is in the key and a re-upload
        // is a new item.
        if path.starts_with("/uploads/") {
            return "public, max-age=3600";
        }
        if path == "/robots.txt" || path == "/llms.txt" {
            return "public, max-age=3600";
        }
        if path == "/sitemap.xml" {
            return "public, max-age=600";
        }
        // Anonymous by construction: `list_items` passes `None` for the voter,
        // so `liked_by_me` is always false and there is nothing per-visitor in
        // the body to leak between users of a shared cache.
        if path == "/oembed" || path.starts_with("/api/v1/") {
            return "public, max-age=60";
        }
        if path.starts_with(&item_prefix) && path.ends_with("/download") {
            return "public, max-age=3600";
        }
        // Sessions, moderation and the upload form. `no-store` rather than
        // `no-cache`: not "revalidate", but "do not write this to disk."
        if path.starts_with("/admin")
            || path.starts_with("/upload")
            || path.starts_with("/auth/")
            || path.starts_with("/api/")
        {
            return "no-store";
        }
        // Unhashed static files served out of `public/` (the placeholder brand
        // art under /brand, and the generated manifest). A day, not a year --
        // without a hash in the name, `immutable` would make replacing one
        // impossible.
        if [".png", ".ico", ".svg", ".webp", ".woff2", ".webmanifest"]
            .iter()
            .any(|ext| path.ends_with(ext))
        {
            return "public, max-age=86400";
        }
        // Everything else is server-rendered HTML.
        //
        // `private` is not cosmetic. With `SsrMode::Async` on `/`, the entire
        // document is rendered before the first flush, `AccountAction`
        // included -- a signed-out `/` already contains `aria-label="Sign in"`
        // in the served HTML, which means a signed-in visitor's display name
        // and Google avatar URL are in the body of theirs. A shared cache must
        // never store that. Do not relax this to `public, s-maxage=...` for
        // gallery performance without first moving the account control out of
        // the SSR'd body.
        "private, no-cache"
    }

    /// Paths that should never appear in a search index: byte-identical copies
    /// of images already crawlable at their media origin, machine-readable
    /// duplicates of pages that are indexable in their own right, and the
    /// embed card, which is a thin duplicate of the item page it links to.
    ///
    /// A header rather than (only) a robots.txt `Disallow`, because these are
    /// not all HTML and a `<meta name="robots">` has nowhere to live in a PNG
    /// or a JSON body.
    pub fn noindex(path: &str) -> bool {
        path.starts_with("/api/")
            || path == "/oembed"
            || path.starts_with("/uploads/")
            || path.starts_with("/embed/")
            || (path.starts_with(&geekgallery::flavor::get().item_prefix())
                && path.ends_with("/download"))
    }

    /// Applies the above to every response, without ever overwriting a header
    /// a handler set for itself.
    ///
    /// `/embed/:id` sets its own `Cache-Control` (300 on a hit, `no-store` on
    /// a miss, so a moderation removal is not masked by an edge-cached copy)
    /// and `/item/:id/download` depends on its `Content-Disposition`. Insert
    /// only when absent; never touch `Content-Type` or `Content-Disposition`.
    pub async fn headers(
        State(hashed): State<bool>,
        req: Request<axum::body::Body>,
        next: Next,
    ) -> Response {
        let path = req.uri().path().to_string();
        let idempotent = matches!(*req.method(), Method::GET | Method::HEAD);

        let mut resp = next.run(req).await;
        let failed = resp.status().is_client_error() || resp.status().is_server_error();
        let h = resp.headers_mut();

        // Three overrides, all deliberate. A response that sets a cookie is
        // handing out an identity, and a response to a mutating request is not
        // a resource anyone should keep -- in both cases a handler's own
        // `Cache-Control` would be the bug, not the thing to preserve. And an
        // error is never a resource: during the cutover from the per-site
        // repos an old container answered a request for a new hashed
        // stylesheet with a 404, this stamped it `immutable`, and the edge
        // served that 404 for the CSS -- an unstyled site -- for half an hour
        // after the old container was gone, until the zone cache was purged.
        if !idempotent || failed || h.contains_key(header::SET_COOKIE) {
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        } else if !h.contains_key(header::CACHE_CONTROL) {
            h.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(cache_control(&path, hashed)),
            );
        }

        if noindex(&path) && !h.contains_key(&X_ROBOTS_TAG) {
            h.insert(X_ROBOTS_TAG, HeaderValue::from_static("noindex"));
        }

        // Global, because user-uploaded files are proxied through
        // `/item/:id/download` on this origin: without it a browser that
        // disagrees with the declared type gets to guess, and a guess of
        // `text/html` on attacker-supplied bytes is same-origin script. Videos
        // make this more than theoretical -- an mp4 container will carry
        // almost any payload.
        if !h.contains_key(header::X_CONTENT_TYPE_OPTIONS) {
            h.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
        }

        // Framing. The embed card exists to be framed; nothing else on the
        // site does, and a page with a like button, a delete button and an
        // admin queue on it has no business inside someone else's iframe.
        // Both headers, because `frame-ancestors` is the standard and
        // `X-Frame-Options` is what older embedders' browsers still read.
        if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
            h.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(frame_policy(&path)),
            );
        }
        if !path.starts_with("/embed/") && !h.contains_key(header::X_FRAME_OPTIONS) {
            h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        }
        // The page URL is the referrer a media host or an outbound link sees;
        // a item's own path is public anyway, but a query string never needs
        // to travel, and cross-origin gets the origin alone.
        if !h.contains_key(header::REFERRER_POLICY) {
            h.insert(
                header::REFERRER_POLICY,
                HeaderValue::from_static("strict-origin-when-cross-origin"),
            );
        }
        // Nothing here uses a camera, a microphone or a location; saying so
        // means a script that somehow ran here could not ask for them either.
        if !h.contains_key(&PERMISSIONS_POLICY) {
            h.insert(
                PERMISSIONS_POLICY,
                HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
            );
        }
        // Cloudflare terminates TLS and forwards this header as-is. A browser
        // ignores it over plain http, so a dev box is unaffected.
        if !h.contains_key(header::STRICT_TRANSPORT_SECURITY) {
            h.insert(
                header::STRICT_TRANSPORT_SECURITY,
                HeaderValue::from_static("max-age=31536000; includeSubDomains"),
            );
        }

        resp
    }

    /// `Permissions-Policy`, which `http` has no constant for.
    const PERMISSIONS_POLICY: HeaderName = HeaderName::from_static("permissions-policy");

    /// Who may frame a path. Only `/embed/:id` is built to be framed; every
    /// other response says no.
    pub fn frame_policy(path: &str) -> &'static str {
        if path.starts_with("/embed/") {
            "frame-ancestors *"
        } else {
            "frame-ancestors 'none'"
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn hashed_pkg_assets_are_immutable_and_unhashed_ones_are_not() {
            assert_eq!(
                cache_control("/pkg/geekgallery.abc123.css", true),
                IMMUTABLE
            );
            // The `cargo leptos watch` case: one fixed filename, new contents
            // every edit. A year here is unrecoverable.
            assert_eq!(cache_control("/pkg/geekgallery.css", false), "no-cache");
            assert_eq!(cache_control("/pkg/geekgallery.wasm", false), "no-cache");
        }

        #[test]
        fn only_the_embed_card_may_be_framed() {
            assert_eq!(frame_policy("/embed/item-in-a-box"), "frame-ancestors *");
            for path in ["/", "/item/item-in-a-box", "/admin", "/upload", "/embed"] {
                assert_eq!(frame_policy(path), "frame-ancestors 'none'", "{path}");
            }
        }

        #[test]
        fn html_is_private_and_never_stored_by_a_shared_cache() {
            for path in [
                "/",
                "/item/item-in-a-box",
                "/leaderboard",
                "/search",
                "/tos",
            ] {
                assert_eq!(cache_control(path, true), "private, no-cache", "{path}");
            }
        }

        #[test]
        fn identity_and_moderation_routes_are_no_store() {
            for path in [
                "/admin",
                "/upload",
                "/api/upload",
                "/auth/google/callback",
                "/api/like_item1234",
            ] {
                assert_eq!(cache_control(path, true), "no-store", "{path}");
            }
        }

        #[test]
        fn public_data_routes_are_shareable_but_short_lived() {
            assert_eq!(cache_control("/api/v1/items", true), "public, max-age=60");
            assert_eq!(cache_control("/oembed", true), "public, max-age=60");
            assert_eq!(cache_control("/sitemap.xml", true), "public, max-age=600");
            assert_eq!(cache_control("/robots.txt", true), "public, max-age=3600");
            assert_eq!(cache_control("/llms.txt", true), "public, max-age=3600");
            assert_eq!(
                cache_control("/item/item-in-a-box/download", true),
                "public, max-age=3600"
            );
            assert_eq!(
                cache_control("/uploads/orig/x.png", true),
                "public, max-age=3600"
            );
        }

        /// `/api/v1/` has to be matched before the `/api/` no-store rule, and
        /// an unhashed icon before the HTML fallthrough.
        #[test]
        fn the_specific_class_wins_over_the_general_one() {
            assert_eq!(cache_control("/api/v1/items/x", true), "public, max-age=60");
            assert_eq!(
                cache_control("/favicon-32.png", true),
                "public, max-age=86400"
            );
            assert_eq!(
                cache_control("/apple-touch-icon.png", false),
                "public, max-age=86400"
            );
            assert_eq!(
                cache_control("/manifest.webmanifest", false),
                "public, max-age=86400"
            );
            // Inside /pkg/ the hashing rule wins, extension notwithstanding.
            assert_eq!(cache_control("/pkg/logo.svg", false), "no-cache");
        }

        #[test]
        fn noindex_covers_the_duplicates_and_nothing_else() {
            assert!(noindex("/embed/item-in-a-box"));
            assert!(noindex("/item/item-in-a-box/download"));
            assert!(noindex("/api/v1/items"));
            assert!(noindex("/oembed"));
            assert!(noindex("/uploads/orig/x.png"));
            // The pages that must stay indexable.
            assert!(!noindex("/"));
            assert!(!noindex("/item/item-in-a-box"));
            assert!(!noindex("/leaderboard"));
        }
    }
}
