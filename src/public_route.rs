//! Plain, stable routes meant for consumption outside this site's own
//! frontend: `/api/v1/*` (a documented public API), `/oembed` (the oEmbed
//! spec, so third-party embedders don't have to scrape OG tags), `/embed/:id`
//! (a framable card for one item) and the same-origin download proxy the
//! detail page's download button needs.
//!
//! Deliberately not Leptos server functions: those live at hashed paths that
//! change on every rebuild (`/api/list_items4217581579200484497`), which is
//! fine for this site's own wasm bundle but useless as a stable public
//! contract for anyone else to integrate against.
//!
//! `/embed/:id` is a plain Axum route for the same reason and one more: it is
//! a whole document, not a page inside the app shell. A Leptos route would
//! drag the nav, the wasm bundle and the hydration island into an iframe that
//! wants none of them.

use axum::extract::{Path, Query};
use axum::http::{header, HeaderMap, HeaderName, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::seo::{attr_escape, embed_iframe, fit_within, EMBED_BAR_H, EMBED_DEFAULT_W};

/// `X-Robots-Tag`, which `http` has no constant for.
const X_ROBOTS_TAG: HeaderName = HeaderName::from_static("x-robots-tag");

#[derive(Deserialize)]
pub struct ListQuery {
    cursor: Option<String>,
}

/// `GET /api/v1/items` — a page of public items, newest first. No personalization
/// (`liked_by_me` is always `false`): an anonymous API consumer never carries
/// this site's voter cookie, so there is nothing to personalize against.
pub async fn list_items(Query(q): Query<ListQuery>) -> impl IntoResponse {
    match crate::db::list_public(q.cursor.as_deref(), crate::models::Sort::Newest, None).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => api_error(e),
    }
}

/// `GET /api/v1/items/:id` — a single public item. 404s for a hidden one, same
/// as the site's own detail page.
pub async fn get_item(Path(id): Path<String>) -> impl IntoResponse {
    match crate::db::get(&id, None).await {
        Ok(Some(item)) if item.is_public => Json(item).into_response(),
        Ok(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => api_error(e),
    }
}

fn api_error(e: anyhow::Error) -> axum::response::Response {
    tracing::error!("public API error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

#[derive(Serialize)]
struct OEmbedResponse {
    version: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    title: String,
    author_name: Option<String>,
    provider_name: &'static str,
    provider_url: &'static str,
    /// The image itself for a `photo`; absent for a `rich`, where `html` is
    /// the resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    width: u32,
    height: u32,
    /// Only on a `rich` response. `skip_serializing_if` rather than a `null`:
    /// consumers that branch on the presence of the key exist, and some reject
    /// a null outright.
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<String>,
    /// Seconds. Matches `/embed/:id`'s own `max-age`, so a consumer that
    /// honours this refetches on the same clock the edge does -- which matters
    /// because the three-report auto-hide is the primary moderation mechanism
    /// and a stale embed outlives the decision.
    cache_age: &'static str,
    /// oEmbed requires all three thumbnail fields or none of them, so these
    /// are set and cleared together.
    #[serde(skip_serializing_if = "Option::is_none")]
    thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumbnail_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumbnail_height: Option<u32>,
}

#[derive(Deserialize)]
pub struct OEmbedQuery {
    url: String,
    /// `json` or absent. The spec says a provider that cannot supply the
    /// requested format must answer 501, and answering JSON to `format=xml`
    /// (which is what this did) leaves the consumer parsing HTML-ish noise as
    /// XML instead of seeing a clear "not supported".
    format: Option<String>,
    maxwidth: Option<u32>,
    maxheight: Option<u32>,
}

/// `GET /oembed?url=...` per the oEmbed 1.0 spec (oembed.com): given one of
/// this site's own URLs, return embeddable metadata. This exists alongside the
/// OG tags already on the detail page because oEmbed is what tools that don't
/// just scrape Open Graph (many wikis, some chat platforms' generic embed
/// handling) look for instead.
///
/// Two URL shapes, two resource types, on purpose:
///
/// - `/item/<slug>` -> `type: photo`, the answer the `<link rel="alternate"
///   type="application/json+oembed">` on the detail page has been advertising
///   since before `/embed/` existed. Consumers have that cached. Switching it
///   to `rich` would hand an iframe to everything that currently renders the
///   image, and some of them refuse to frame anything -- a silent unfurl
///   regression with no error anywhere.
/// - `/embed/<slug>` -> `type: rich`, with the iframe markup built by the same
///   `seo::embed_iframe` the copy button uses.
///
/// Both are spec-legal: they are two different resources that happen to
/// describe one item.
pub async fn oembed(Query(q): Query<OEmbedQuery>) -> impl IntoResponse {
    // Absent means "provider's choice" per the spec, and JSON is the choice.
    if !matches!(q.format.as_deref(), None | Some("json")) {
        return (StatusCode::NOT_IMPLEMENTED, "only format=json is supported").into_response();
    }

    let Some((id, wants_embed)) = extract_item_ref(&q.url) else {
        return (
            StatusCode::BAD_REQUEST,
            "url must point at an item page or an /embed/:id page on this site",
        )
            .into_response();
    };

    let item = match crate::db::get(&id, None).await {
        Ok(Some(item)) if item.is_public => item,
        Ok(_) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => return api_error(e),
    };

    // All three or none, per the spec. A zero-sized source (impossible for a
    // stored item, but the columns allow it) drops the group rather than
    // reporting 0x0.
    let (tw, th) = fit_within(item.width, item.height, crate::storage::THUMB_MAX_EDGE);
    let thumb = (tw > 0 && th > 0).then(|| item.thumb_url.clone());

    let mut resp = OEmbedResponse {
        version: "1.0",
        // A clip is `video`, with the same iframe the embed card uses: the
        // oEmbed `photo` type has nowhere to put something that plays.
        kind: if item.kind == crate::models::MediaKind::Video {
            "video"
        } else {
            "photo"
        },
        title: item.title.clone(),
        author_name: item.uploader.map(|u| u.display_name),
        provider_name: &crate::flavor::get().name,
        provider_url: site_origin(),
        url: None,
        width: 0,
        height: 0,
        html: None,
        cache_age: "300",
        thumbnail_url: thumb.clone(),
        thumbnail_width: thumb.is_some().then_some(tw),
        thumbnail_height: thumb.is_some().then_some(th),
    };

    // A `video` response is a `rich`-shaped one: html, width, height. So a
    // clip takes the embed path whichever URL was asked about.
    if wants_embed || item.kind == crate::models::MediaKind::Video {
        // The card is a square image area above a fixed-height bar, so its
        // height follows its width -- which means `maxheight` constrains the
        // width too, not just the height.
        let mut w = EMBED_DEFAULT_W;
        if let Some(mw) = q.maxwidth {
            w = w.min(mw);
        }
        if let Some(mh) = q.maxheight {
            w = w.min(mh.saturating_sub(EMBED_BAR_H));
        }
        // Below this the bar has no room for a title and the card is not worth
        // rendering; honouring an absurd maxwidth exactly would be worse than
        // returning the smallest card that works.
        let w = w.max(120);
        let embed_url = crate::seo::absolute(&crate::seo::embed_path(&item.slug));
        if resp.kind != "video" {
            resp.kind = "rich";
        }
        resp.width = w;
        resp.height = w + EMBED_BAR_H;
        resp.html = Some(embed_iframe(&embed_url, &item.title, w, w + EMBED_BAR_H));
    } else {
        // The cap is whichever of the two bounds binds first; a item is a
        // rectangle and either edge can be the one that does not fit.
        let cap = [q.maxwidth, q.maxheight]
            .into_iter()
            .flatten()
            .min()
            .filter(|c| *c < item.width || *c < item.height);
        match cap {
            Some(c) => {
                // Reported at the size asked for, not the source's own size:
                // a `width` above `maxwidth` is the one thing this parameter
                // exists to prevent. And below a thumbnail's edge, serve the
                // thumbnail -- a 12MB original decoded into a 300px slot is R2
                // egress spent on pixels nobody sees.
                let (dw, dh) = fit_within(item.width, item.height, c);
                resp.url = Some(if c <= crate::storage::THUMB_MAX_EDGE && tw > 0 {
                    item.thumb_url
                } else {
                    item.orig_url
                });
                resp.width = dw;
                resp.height = dh;
            }
            None => {
                resp.url = Some(item.orig_url);
                resp.width = item.width;
                resp.height = item.height;
            }
        }
    }

    Json(resp).into_response()
}

/// Everything the framable card needs, as a single inline stylesheet.
///
/// Deliberately no reference to `/pkg/geekgallery.css`: that filename
/// depends on `hash.txt` plus the `LEPTOS_HASH_FILES` runtime switch, and
/// re-deriving it here would couple third-party embeds to the hashing trap
/// described in CLAUDE.md -- an embed silently losing all styling on every
/// site that uses it, discovered by nobody.
///
/// The colours are copied by hand from `tailwind.config.js` (bg `#0a0b0f`,
/// surface `#10121a`, line `#2b3042`, ink `#f2f4f8`/`#aab0c0`, accent
/// `#9aa4ff`). That file is the palette's source of truth and this is a real
/// duplication of it; there is no `@apply` available in a string constant, so
/// a palette change has to be made in both places.
///
/// No Tailwind class name appears anywhere in this document. The scanner reads
/// .rs files as raw text, so utility names written here would be emitted into
/// `geekgallery.css` -- harmless, since nothing here loads that file, but
/// the reverse is not: a `.card` or `.btn` used *in the markup* would render
/// completely unstyled, because the embed page loads no stylesheet but this
/// one.
///
/// `min-height:0` on `.se-img` is load-bearing. A flex item's default
/// `min-height:auto` sizes it by its content's min-content height, so the
/// image pushes the card past the iframe's box and the bar disappears below
/// the fold -- the same failure CLAUDE.md records for implicit grid tracks.
/// `object-fit:contain` letterboxes, because items are every shape -- a
/// portrait phone clip and a wide screenshot share one snippet.
fn embed_css() -> String {
    let t = &crate::flavor::get().theme;
    format!(
        "html,body{{margin:0;height:100%;background:{bg};color:{ink};\
font:400 14px/1.35 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}}\
a{{color:inherit;text-decoration:none}}\
.se-card{{display:flex;flex-direction:column;height:100%;box-sizing:border-box;\
background:{surface};border:1px solid {line};border-radius:10px;overflow:hidden}}\
.se-img{{flex:1 1 auto;min-height:0;display:flex;align-items:center;\
justify-content:center;padding:12px}}\
.se-img img,.se-img video{{max-width:100%;max-height:100%;width:auto;height:auto;\
object-fit:contain;display:block}}\
.se-bar{{flex:none;display:flex;align-items:center;justify-content:space-between;\
gap:8px;height:44px;padding:0 12px;border-top:1px solid {line}}}\
.se-title{{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:500}}\
.se-brand{{flex:none;color:{accent};font-size:12px}}\
.se-miss{{display:flex;height:100%;align-items:center;justify-content:center;\
padding:12px;text-align:center;color:{ink2}}}",
        bg = t.bg,
        ink = t.ink,
        surface = t.surface,
        line = t.line,
        accent = t.accent,
        ink2 = t.ink_2,
    )
}

/// The document shell every `/embed/:id` response uses, hit or miss.
///
/// `noindex, follow` and no `<link rel="canonical">`. The canonical is the
/// missing half on purpose: Google documents `noindex` plus a canonical
/// pointing elsewhere as a mistake, because the `noindex` propagates to the
/// canonical target -- which here would deindex the item's own page. `follow`
/// still lets a crawler walk the link back to it. The matching mistake is
/// disallowing `/embed/` in robots.txt, which would stop the crawler ever
/// fetching the page and reading this tag; see `seo_route::robots_txt`.
fn embed_document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex, follow\">\
         <title>{title} — geekgallery</title>\
         <style>{css}</style></head><body>{body}</body></html>",
        title = attr_escape(title),
        css = embed_css(),
    )
}

/// `GET /embed/:id` — one item as a standalone, framable document.
///
/// No Leptos, no hydration, no wasm: an iframe on someone else's site should
/// cost them one small HTML document and one image, not this app's bundle.
/// The card fills 100% of whatever box the embedder gives it, so a single
/// snippet works at every width and for every item's aspect ratio.
pub async fn embed(Path(id): Path<String>) -> impl IntoResponse {
    // `db::get` matches slug *or* id, so an embed minted before slugs existed
    // keeps resolving on whatever site it was pasted into.
    let item = match crate::db::get(&id, None).await {
        Ok(Some(item)) if item.is_public => item,
        Ok(_) => {
            return embed_gone(
                StatusCode::NOT_FOUND,
                &format!("This {} is no longer here.", crate::flavor::get().noun),
            )
        }
        Err(e) => {
            tracing::error!("embed lookup failed for {id}: {e}");
            return embed_gone(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("This {} is unavailable.", crate::flavor::get().noun),
            );
        }
    };

    // The thumbnail is the right file for a default-width card and the
    // original only earns its bytes above that, so let the browser choose. The
    // `w` descriptors have to be the real widths: `fit_within` reports what
    // `storage` actually produced. Skipped when the "thumbnail" is an upscale
    // of a small item, where the original is the smaller file of the two.
    let (tw, _) = fit_within(item.width, item.height, crate::storage::THUMB_MAX_EDGE);
    // Stills only: a GIF's thumbnail is a frozen frame and a video is not an
    // <img> at all, so neither gets a srcset.
    let srcset = if item.kind == crate::models::MediaKind::Image && tw > 0 && tw < item.width {
        format!(
            " srcset=\"{thumb} {tw}w, {orig} {ow}w\" sizes=\"100vw\"",
            thumb = attr_escape(&item.thumb_url),
            orig = attr_escape(&item.orig_url),
            ow = item.width,
        )
    } else {
        String::new()
    };

    // A clip plays in the card, muted and looping like a GIF would, because a
    // still of a video in an iframe is a broken-looking embed. Controls are on,
    // so it can be paused and unmuted; autoplay only works muted anyway.
    let media = if item.kind == crate::models::MediaKind::Video {
        format!(
            "<video src=\"{orig}\" poster=\"{poster}\" width=\"{w}\" height=\"{h}\" \
             autoplay muted loop playsinline controls preload=\"metadata\" \
             aria-label=\"{alt}\"></video>",
            orig = attr_escape(&item.orig_url),
            poster = attr_escape(item.still_url()),
            alt = attr_escape(&item.title),
            w = item.width,
            h = item.height,
        )
    } else {
        format!(
            "<img src=\"{orig}\"{srcset} alt=\"{alt}\" width=\"{w}\" height=\"{h}\" loading=\"lazy\">",
            orig = attr_escape(&item.orig_url),
            alt = attr_escape(&item.title),
            w = item.width,
            h = item.height,
        )
    };
    let body = format!(
        "<a class=\"se-card\" href=\"{page}\" target=\"_blank\" rel=\"noopener\">\
         <div class=\"se-img\">{media}</div>\
         <div class=\"se-bar\"><span class=\"se-title\">{title}</span>\
         <span class=\"se-brand\">{brand}</span></div></a>",
        // From the slug, never the id: an embed is the most-copied link this
        // site emits and it should carry the readable form.
        page = attr_escape(&format!(
            "{}{}",
            site_origin(),
            crate::flavor::get().item_path(&item.slug)
        )),
        title = attr_escape(&item.title),
        brand = attr_escape(&crate::flavor::get().name),
    );

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // Five minutes is the ceiling, and `stale-while-revalidate` is
            // deliberately absent: /admin and the three-report auto-hide are
            // the primary moderation mechanism, and an edge that keeps serving
            // a removed item past its TTL would outlive the decision on sites
            // this one does not control.
            (header::CACHE_CONTROL, "public, max-age=300"),
            (X_ROBOTS_TAG, "noindex"),
            // Explicit permission to be framed anywhere -- and a marker, so
            // that a blanket `X-Frame-Options: DENY` added later cannot be
            // dropped in without noticing it breaks this route.
            (header::CONTENT_SECURITY_POLICY, "frame-ancestors *"),
        ],
        embed_document(&item.title, &body),
    )
        .into_response()
}

/// A styled miss, not a bare status line. An empty 404 body renders as the
/// browser's own error page *inside the iframe*, which looks like the
/// embedder's site is broken rather than like a item that went away.
///
/// `no-store`, so a removal takes effect immediately everywhere and a
/// re-publish is not masked by an edge-cached miss.
fn embed_gone(status: StatusCode, message: &str) -> axum::response::Response {
    let body = format!("<div class=\"se-miss\">{}</div>", attr_escape(message));
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (X_ROBOTS_TAG, "noindex"),
            (header::CONTENT_SECURITY_POLICY, "frame-ancestors *"),
        ],
        embed_document("Not found", &body),
    )
        .into_response()
}

/// Whether this request carries an admin's session cookie.
async fn is_admin(headers: &HeaderMap) -> bool {
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok());
    matches!(
        crate::auth::current_user(cookie).await,
        Ok(Some(u)) if u.is_admin
    )
}

/// Same-origin download proxy. `<a download>` is silently ignored by browsers
/// when the link target is cross-origin, and R2's public domain
/// (media.geekgallery.com) is a different origin from the app -- so this
/// route fetches the object itself and sets `Content-Disposition` on a
/// response that genuinely comes from this site, which is the only way the
/// browser reliably treats the click as "save this file."
///
/// The whole object is buffered in memory, which for a 60MB clip is 60MB per
/// concurrent download. Acceptable at this scale; a streaming body is the
/// change to make if download traffic ever matters.
/// Takes the headers only for the admin exception: an admin reviewing a held item
/// can open its page (see `api::get_item`), and a Download button that 404s on the
/// page it is drawn on is exactly the dead end this route would otherwise create.
/// Everyone else still gets a 404 for anything hidden.
pub async fn download(headers: HeaderMap, Path(id): Path<String>) -> impl IntoResponse {
    let item = match crate::db::get(&id, None).await {
        Ok(Some(item)) if item.is_public || is_admin(&headers).await => item,
        Ok(_) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => return api_error(e),
    };

    let key = crate::storage::orig_key(&item.id, item.extension());
    let bytes = match crate::storage::backend().get(&key).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("download fetch failed for {id}: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not fetch this file",
            )
                .into_response();
        }
    };

    let filename = filename_for(&item.title, item.extension());
    (
        [
            (header::CONTENT_TYPE, item.mime().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// A item's title, filesystem-safe. Titles are free text (see
/// `upload_route::clean_title`) and can contain characters invalid in a
/// filename on some platforms, or nothing usable at all.
fn filename_for(title: &str, extension: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        format!("{}.{extension}", crate::flavor::get().noun)
    } else {
        format!("{trimmed}.{extension}")
    }
}

/// Pull the id out of `.../item/<id>` or `.../embed/<id>` (with or without a
/// trailing slash, from any host), and say which of the two it was.
///
/// oEmbed consumers pass back exactly the URL the site published, so this only
/// needs to parse our own path shapes, not validate an arbitrary URL. The
/// second half of the return value is what selects `photo` versus `rich` --
/// the two are different resources, so the URL is the only thing that can
/// decide.
fn extract_item_ref(url: &str) -> Option<(String, bool)> {
    let path = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once('/'))
        .map(|(_, p)| p)
        .unwrap_or(url);
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut segments = path.trim_matches('/').split('/');
    let noun = crate::flavor::get().noun.as_str();
    let is_embed = match segments.next()? {
        "embed" => true,
        seg if seg == noun => false,
        _ => return None,
    };
    let id = segments.next()?;
    if id.is_empty() {
        None
    } else {
        Some((id.to_string(), is_embed))
    }
}

/// Cached once, not re-leaked per call: `SITE_ORIGIN` is fixed for the
/// process's lifetime, so this only ever allocates a single `String`.
/// `pub(crate)`: `seo_route` needs the same origin for `robots.txt`/
/// `sitemap.xml`/`llms.txt`, and there is exactly one process-wide value to
/// agree on -- not something to look up twice.
pub(crate) fn site_origin() -> &'static str {
    static ORIGIN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(|| crate::flavor::get().origin.clone())
}

/// `GET /manifest.webmanifest`: the installable-app manifest, built from the
/// flavor rather than shipped as a file, because the name, the colours and the
/// icon URLs in it are exactly the things that differ per deployment.
pub async fn manifest() -> impl IntoResponse {
    let f = crate::flavor::get();
    let icon = |file: &str, size: &str| serde_json::json!({ "src": f.asset(file), "sizes": size, "type": "image/png" });
    let body = serde_json::json!({
        "name": f.name,
        "short_name": f.name,
        "description": f.description,
        "id": "/",
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "background_color": f.theme.bg,
        "theme_color": f.theme.bg,
        "icons": [
            icon("favicon-192.png", "192x192"),
            icon("favicon-512.png", "512x512"),
        ],
        "shortcuts": [
            {
                "name": format!("Upload {}", f.a_noun()),
                "short_name": "Upload",
                "description": format!("Add a picture, GIF or clip to {}", f.name),
                "url": "/upload",
                "icons": [icon("favicon-192.png", "192x192")]
            },
            {
                "name": "Clips",
                "short_name": "Clips",
                "description": "Only the videos",
                "url": "/?view=clips",
                "icons": [icon("favicon-192.png", "192x192")]
            }
        ]
    });
    (
        [
            (header::CONTENT_TYPE, "application/manifest+json"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_id_from_a_normal_url() {
        assert_eq!(
            extract_item_ref("https://geekgallery.com/item/abc-123"),
            Some(("abc-123".to_string(), false))
        );
    }

    #[test]
    fn extracts_id_with_trailing_slash() {
        assert_eq!(
            extract_item_ref("https://geekgallery.com/item/abc-123/"),
            Some(("abc-123".to_string(), false))
        );
    }

    /// The flag, not just the id: this is what makes `/embed/<slug>` answer
    /// `rich` while the item page keeps answering `photo`.
    #[test]
    fn extracts_id_from_an_embed_url_and_flags_it() {
        assert_eq!(
            extract_item_ref("https://geekgallery.com/embed/abc-123"),
            Some(("abc-123".to_string(), true))
        );
        assert_eq!(
            extract_item_ref("http://127.0.0.1:3100/embed/abc-123/"),
            Some(("abc-123".to_string(), true))
        );
    }

    #[test]
    fn rejects_urls_that_are_not_a_item_page() {
        assert_eq!(extract_item_ref("https://geekgallery.com/upload"), None);
        assert_eq!(extract_item_ref("https://evil.example/item/"), None);
        assert_eq!(extract_item_ref("https://geekgallery.com/embed/"), None);
        assert_eq!(extract_item_ref("https://geekgallery.com/"), None);
    }

    /// The embed page's own CSS must never grow a Tailwind class name: it
    /// loads no stylesheet but its own, so one would render unstyled, and the
    /// scanner would emit the rule into `geekgallery.css` for nothing.
    #[test]
    fn embed_document_is_self_contained_and_noindex() {
        let html = embed_document("Item in a box", "<div class=\"se-miss\">x</div>");
        assert!(html.contains("<meta name=\"robots\" content=\"noindex, follow\">"));
        assert!(!html.contains("rel=\"canonical\""));
        assert!(!html.contains("geekgallery.css"));
        assert!(html.contains(".se-img{flex:1 1 auto;min-height:0"));
    }

    /// A title is free text and lands in `<title>` and in three attributes.
    #[test]
    fn embed_document_escapes_a_hostile_title() {
        let html = embed_document("</title><script>alert(1)</script>", "");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;/title&gt;"));
    }

    #[test]
    fn filename_strips_unsafe_characters() {
        assert_eq!(
            filename_for("Item/Aversary: the *best*", "png"),
            "Item_Aversary_ the _best_.png"
        );
        assert_eq!(filename_for("   ", "mp4"), "item.mp4");
    }
}
