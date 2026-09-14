// ItemDetail's view! (Title/Meta/Link/script siblings for OG, Twitter, JSON-LD,
// and oEmbed discovery, plus the article body) generates deeply nested
// tachys types -- release-mode monomorphization hits rustc's default query
// depth limit (128) here specifically, even though debug builds never do.
// Found through a real CI failure across several pushes, not by reading docs.
#![recursion_limit = "512"]

pub mod api;
pub mod app;
pub mod components;
/// One deployment's identity: name, noun, colours, like verb, art. Read
/// from the environment on the server and from the embedded JSON in wasm.
pub mod flavor;
pub mod models;
pub mod seo;

/// `GET|POST /admin/import`: paste a list of links, queue them all.
#[cfg(feature = "ssr")]
pub mod admin_import;
#[cfg(feature = "ssr")]
pub mod auth;
#[cfg(feature = "ssr")]
pub mod captcha;
#[cfg(feature = "ssr")]
pub mod d1;
#[cfg(feature = "ssr")]
pub mod db;
#[cfg(feature = "ssr")]
pub mod dedupe;
/// Purges a deleted item's media from the CDN edge, when a zone is configured.
#[cfg(feature = "ssr")]
pub mod edge_cache;
/// Reorders an MP4 so it starts playing before the whole file has arrived.
#[cfg(feature = "ssr")]
pub mod faststart;
/// A pasted link turned into media bytes: X, Instagram, Facebook, YouTube,
/// any page with Open Graph tags, or a bare file URL.
#[cfg(feature = "ssr")]
pub mod fetch;
#[cfg(feature = "ssr")]
pub mod import_route;
/// In-memory upload progress, polled by the upload page.
#[cfg(feature = "ssr")]
pub mod indexnow;
#[cfg(feature = "ssr")]
pub mod jobs;
/// ffmpeg frames, GIF frame choice, and the flatness test behind covers.
#[cfg(feature = "ssr")]
pub mod media_tools;
/// An hourly pull from another gallery, when one is configured.
#[cfg(feature = "ssr")]
pub mod mirror;
/// A one-off pass at startup that replaces covers stored black.
#[cfg(feature = "ssr")]
pub mod repair;

#[cfg(feature = "ssr")]
pub mod oauth_route;
#[cfg(feature = "ssr")]
pub mod public_route;
#[cfg(feature = "ssr")]
pub mod seo_route;
#[cfg(feature = "ssr")]
pub mod storage;
#[cfg(feature = "ssr")]
pub mod upload_route;
#[cfg(feature = "ssr")]
pub mod watermark;

/// Wasm entry point. `cargo-leptos` wires this into the generated JS loader.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    // The flavor first, before a single component renders: every label that
    // says the site's name or its noun reads it, and the server rendered from
    // the same value, so this is what keeps the two in agreement.
    let embedded = leptos::prelude::document()
        .get_element_by_id(flavor::EMBED_ID)
        .and_then(|el| el.text_content())
        .and_then(|json| serde_json::from_str::<flavor::Flavor>(&json).ok());
    match embedded {
        Some(f) => flavor::set(f),
        None => leptos::logging::error!("no flavor in the document; rendering the default"),
    }
    leptos::mount::hydrate_body(app::App);
}
