//! Crawler-facing routes: `robots.txt`, `sitemap.xml` (with the Google Images
//! *and* Google Video sitemap extensions), and `llms.txt` (the emerging
//! convention for an LLM-readable site summary).
//!
//! The two sitemap extensions are the few Google-documented, structural levers
//! for ranking in Google Images and in video results specifically -- alongside
//! descriptive `alt`/title text (on every `<img>`) and structured data (the
//! `ImageObject`/`VideoObject` JSON-LD on the detail page). Sitemaps don't
//! guarantee indexing, but they're the difference between "Google might
//! eventually crawl this" and "Google has an explicit, complete list."

use axum::http::header;
use axum::response::IntoResponse;

use crate::models::MediaKind;
use crate::public_route::site_origin;
use crate::seo::w3c_lastmod;

/// One group, `User-agent: *`.
///
/// There is deliberately no AI-crawler group (GPTBot, CCBot, ClaudeBot,
/// Google-Extended). That is a decision, not an omission: this site publishes
/// an `llms.txt` describing itself for exactly those readers, and the content
/// is a meme collection that is meant to spread -- being quoted by an answer
/// engine is traffic. Someone re-reading this file should know it was
/// considered.
///
/// Two things are pointedly *not* disallowed, and both would be mistakes:
///
/// - `/embed/` — a `Disallow` stops a crawler fetching the URL at all, so it
///   never reads the `noindex, follow` the embed page serves, and the URL
///   stays eligible for indexing as a thin duplicate on the strength of
///   inbound links alone. Blocking and noindexing the same path are mutually
///   exclusive; the page carries the tag, so robots must let it be read.
/// - `/search` — same mechanism (it already serves `noindex, follow`), and
///   blocking it would additionally throw away the `follow` that lets a
///   crawler reach individual items through result links.
///
/// `/oembed`, `/uploads/` and `/item/*/download` *are* disallowed even though
/// they also carry `X-Robots-Tag: noindex`, and that is not the same
/// contradiction. The aim there is not to be fetched at all: the download
/// proxy is a byte-identical copy of an image already crawlable at its
/// media.geekgallery.com URL, and serving it twice costs real R2 egress.
/// The header is only a fallback for a crawler that ignores this file.
///
/// `*` wildcards are honoured by Google and Bing and ignored (harmlessly, as a
/// literal path) by everything else.
pub async fn robots_txt() -> impl IntoResponse {
    let origin = site_origin();
    let item_prefix = crate::flavor::get().item_prefix();
    let body = format!(
        "User-agent: *\n\
         Allow: /\n\
         Disallow: /admin\n\
         Disallow: /api/\n\
         Disallow: /auth/\n\
         Disallow: /oembed\n\
         Disallow: /uploads/\n\
         Disallow: {item_prefix}*/download\n\
         Sitemap: {origin}/sitemap.xml\n"
    );
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            // The middleware in main.rs would set the same value; stated here
            // because this handler already owns its header list and one line
            // is cheaper than making a reader go and check.
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
}

pub async fn llms_txt() -> impl IntoResponse {
    let f = crate::flavor::get();
    let origin = site_origin();
    let (images, gifs, videos) = crate::db::counts_by_kind().await.unwrap_or_default();
    let noun = &f.noun;
    let nouns = &f.nouns;
    let also = if f.alternate_names.is_empty() {
        String::new()
    } else {
        format!(" (also written {})", f.alternate_names.join(", "))
    };
    let mut body = vec![
        format!("# {}", f.name),
        String::new(),
        format!(
            "> {name}{also}: {tagline} {description} Community uploads, no account \
             required, moderated by report after publishing. Currently {images} images, \
             {gifs} GIFs and {videos} clips.",
            name = f.name,
            tagline = f.tagline,
            description = f.description,
        ),
        String::new(),
        "## Pages".to_string(),
        String::new(),
        format!("- [Gallery]({origin}/): every public {noun}, sortable newest / {liked} / A-Z / clips only / {noun} of the day", liked = f.like.sort_label.to_lowercase()),
        format!("- [Clips]({origin}/?view=clips): only the GIFs and videos"),
        format!("- [About]({origin}/about): what belongs here, how uploads and moderation work"),
        format!("- [Leaderboard]({origin}/leaderboard): top contributors by upload count"),
        format!("- [Upload]({origin}/upload): contribute {a_noun}, no account required", a_noun = f.a_noun()),
        format!("- [Search]({origin}/search?q=...): full-text search over titles and tags"),
        String::new(),
        "## API".to_string(),
        String::new(),
        format!("- `GET {origin}/api/v1/items` -- paginated JSON list of public {nouns}; each carries `kind` (image|gif|video), `orig_url`, `thumb_url`, `poster_url`, `duration`. `/api/v1/{nouns}` is an alias."),
        format!("- `GET {origin}/api/v1/items/:id` -- a single public {noun}"),
        format!("- `GET {origin}/embed/:slug` -- a standalone, framable card for one {noun} (iframe it; clips autoplay muted; noindex, no JavaScript)"),
        format!("- `GET {origin}/oembed?url=...` -- oEmbed 1.0 metadata. A `{prefix}` URL returns `type: photo` (or `video` for a clip); an `/embed/` URL returns `type: rich` with ready-made iframe HTML", prefix = f.item_prefix()),
        format!("- `GET {origin}/sitemap.xml` -- full sitemap, with the Google Images and Google Video extensions"),
        String::new(),
    ];
    if !f.sister_sites.is_empty() {
        body.push("## Sister sites".to_string());
        body.push(String::new());
        for s in &f.sister_sites {
            body.push(format!("- [{}]({})", s.name, s.url));
        }
        body.push(String::new());
    }
    let body = body.join("\n");
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body)
}

/// Escapes the handful of characters that are structurally significant in
/// XML text content. Titles and tag names are free text (only control
/// characters are stripped on the way in — see `upload_route::clean_title`),
/// so `&`/`<` in particular can genuinely appear here.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Comfortably under the sitemap protocol's 50,000-URL-per-file limit, with
/// headroom before a sitemap index (multiple files) is worth the added
/// complexity of building one.
const MAX_SITEMAP_ITEMS: i64 = 45_000;

/// A `<lastmod>` element, or nothing at all.
///
/// `w3c_lastmod` returns `None` for a timestamp it cannot normalise, and an
/// invalid `<lastmod>` is strictly worse than an absent one: Search Console
/// reports the whole URL entry as an error rather than ignoring the date.
fn lastmod_tag(raw: &str) -> String {
    w3c_lastmod(raw)
        .map(|d| format!("<lastmod>{}</lastmod>", xml_escape(&d)))
        .unwrap_or_default()
}

pub async fn sitemap_xml() -> impl IntoResponse {
    let origin = site_origin();

    // Loaded first, because the two collection-driven static pages take their
    // `<lastmod>` from the newest item. `sitemap_items` is already ordered
    // `created_at DESC`, so that is `first()` -- no second query.
    let items = match crate::db::sitemap_items(MAX_SITEMAP_ITEMS).await {
        Ok(items) => {
            if items.len() as i64 >= MAX_SITEMAP_ITEMS {
                tracing::warn!(
                    "sitemap.xml: hit the {MAX_SITEMAP_ITEMS}-item cap -- older items are being \
                     omitted. Time to build a sitemap index instead of one file."
                );
            }
            items
        }
        Err(e) => {
            tracing::error!("sitemap.xml: could not load items: {e}");
            Vec::new()
        }
    };
    let newest = items.first().map(|s| lastmod_tag(&s.created_at));

    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" \
         xmlns:image=\"http://www.google.com/schemas/sitemap-image/1.1\" \
         xmlns:video=\"http://www.google.com/schemas/sitemap-video/1.1\">\n",
    );

    // Static routes. `/search`, `/admin` and `/embed/` are deliberately
    // absent: search results are per-query (nothing stable to point a crawler
    // at, and the page itself carries a noindex meta tag), `/admin` is
    // disallowed in robots.txt entirely, and an embed card is a noindex
    // duplicate of the item page that already has its own entry below.
    //
    // `<changefreq>` and `<priority>` are ignored by Google outright. They are
    // kept because they cost nothing, other consumers of the protocol still
    // read them, and removing them is churn with no upside.
    //
    // The bool is "does this page change when the collection does" -- only
    // those two get the newest item's timestamp. `/tos` did not change because
    // somebody uploaded a item, and claiming otherwise trains a crawler to
    // ignore `<lastmod>` on this site.
    for (path, priority, changefreq, tracks_collection) in [
        ("/", "1.0", "hourly", true),
        ("/?view=clips", "0.8", "hourly", true),
        ("/leaderboard", "0.5", "daily", true),
        ("/upload", "0.3", "monthly", false),
        ("/about", "0.3", "monthly", false),
        ("/privacy", "0.1", "yearly", false),
        ("/tos", "0.1", "yearly", false),
        ("/dmca", "0.1", "yearly", false),
    ] {
        let lastmod = if tracks_collection {
            newest.clone().unwrap_or_default()
        } else {
            String::new()
        };
        xml.push_str(&format!(
            "  <url><loc>{origin}{path}</loc>{lastmod}<changefreq>{changefreq}</changefreq><priority>{priority}</priority></url>\n",
            path = xml_escape(path),
        ));
    }

    for item in items {
        // `SitemapItem::id` is the slug where there is one (`db::sitemap_items`
        // maps it), so this `<loc>` matches the canonical the detail page
        // emits. Two different URLs for one item in a sitemap and a canonical
        // is how a page gets crawled twice and indexed once, at random.
        let lastmod = match lastmod_tag(&item.created_at) {
            tag if tag.is_empty() => String::new(),
            tag => format!(
                "    {tag}
"
            ),
        };
        let extension = match item.kind {
            // `<image:title>` is kept, but do not add `image:caption` or
            // `image:license` expecting an effect: Google deprecated all three
            // in 2022 and now reads only `<image:loc>` from this extension.
            MediaKind::Image | MediaKind::Gif => format!(
                "    <image:image>
      <image:loc>{img}</image:loc>
      <image:title>{title}</image:title>
    </image:image>
",
                img = xml_escape(&item.orig_url),
                title = xml_escape(&item.title),
            ),
            // The video extension. Google requires all of thumbnail_loc, title,
            // description and content_loc (or player_loc); duration is optional
            // but used. The description is the title again with the site named,
            // because there is no separate description field and an empty
            // element is a validation error.
            MediaKind::Video => {
                let duration = item
                    .duration
                    .filter(|d| *d > 0.0)
                    .map(|d| {
                        format!(
                            "      <video:duration>{}</video:duration>
",
                            d.round().max(1.0) as u64
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "    <video:video>
      <video:thumbnail_loc>{thumb}</video:thumbnail_loc>
      <video:title>{title}</video:title>
      <video:description>{title} — a clip on {site}</video:description>
      <video:content_loc>{content}</video:content_loc>
{duration}      <video:publication_date>{published}</video:publication_date>
      <video:family_friendly>yes</video:family_friendly>
      <video:live>no</video:live>
    </video:video>
",
                    // Absolute, whatever the storage backend: R2 URLs already
                    // are, local-disk ones are not, and a sitemap with a
                    // relative <loc> is rejected outright.
                    thumb = xml_escape(&crate::seo::absolute(
                        item.poster_url.as_deref().unwrap_or(&item.thumb_url)
                    )),
                    title = xml_escape(&item.title),
                    content = xml_escape(&crate::seo::absolute(&item.orig_url)),
                    published = xml_escape(&w3c_lastmod(&item.created_at).unwrap_or_default()),
                    site = xml_escape(&crate::flavor::get().name),
                )
            }
        };
        xml.push_str(&format!(
            "  <url>
    <loc>{origin}{page}</loc>
{lastmod}{extension}  </url>
",
            page = xml_escape(&crate::flavor::get().item_path(&item.id)),
        ));
    }

    xml.push_str(
        "</urlset>
",
    );
    (
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_covers_the_five_predefined_entities() {
        assert_eq!(
            xml_escape("<Item & Co> \"quoted\" 'item'"),
            "&lt;Item &amp; Co&gt; &quot;quoted&quot; &apos;item&apos;"
        );
    }

    /// The whole point of routing through `w3c_lastmod`: a timestamp the
    /// protocol would reject produces no tag rather than a broken one.
    #[test]
    fn lastmod_tag_is_empty_rather_than_invalid() {
        assert_eq!(
            lastmod_tag("2026-08-12T00:15:42.835017700+00:00"),
            "<lastmod>2026-08-12T00:15:42+00:00</lastmod>"
        );
        assert_eq!(
            lastmod_tag("2025-01-09 13:04:55"),
            "<lastmod>2025-01-09T13:04:55Z</lastmod>"
        );
        assert_eq!(lastmod_tag(""), "");
        assert_eq!(lastmod_tag("whenever"), "");
    }
}
