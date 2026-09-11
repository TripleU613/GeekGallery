//! Small URL/text helpers shared by every page component that emits
//! `<Meta>`/`<Link>`/JSON-LD needing an absolute URL or a script-safe string.
//! Not gated behind `ssr`: components render on both the server and (post-
//! hydration) in wasm, so this has to compile under both feature sets.
//!
//! That constraint is the reason the embed helpers live here rather than next
//! to the route that serves them. `/embed/:slug` is rendered in
//! `public_route` (ssr only) and the same iframe snippet is written to the
//! clipboard by `components::share` (wasm only); if each side owned its own
//! copy, the two would drift and nobody would notice until an embed pasted
//! from the site rendered differently from one an oEmbed consumer was handed.
//! Nothing in here may reach for `serde_json`, `chrono`, `reqwest`, `image`,
//! `axum` or `uuid` -- none of them exist in the wasm build.

use leptos::prelude::*;

/// Turns a path or already-absolute URL into an absolute one under
/// `SITE_ORIGIN`. Link unfurlers (Discord, Twitter, Slack) and search engines
/// reject relative `og:image`/canonical URLs, so this matters for every page
/// that emits one.
///
/// Already-absolute input passes through unchanged. `orig_url`/`thumb_url`
/// are already absolute in production (R2 serves from
/// `media.geekgallery.com`, a different origin from the app) and only
/// relative in local-disk dev; blindly prepending `SITE_ORIGIN` on top of an
/// already-absolute URL produced a real bug once --
/// `http://site.comhttps://media.site.com/...`.
pub fn absolute(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    let origin = crate::flavor::get().origin.trim_end_matches('/');
    if origin.is_empty() {
        return url.to_string();
    }
    format!("{origin}{url}")
}

/// Escapes a string for embedding inside a JSON string literal that itself
/// sits inside a `<script>` tag (JSON-LD structured data). Hand-rolled rather
/// than pulled from `serde_json`, which isn't available in the wasm/hydrate
/// build this module also compiles under.
///
/// Beyond JSON's own escaping (quotes, backslashes, control characters), this
/// also escapes `<`, `>`, and `&`: a literal `</script>` in a title or
/// uploader name would close the tag early regardless of JSON syntax, since
/// HTML's script-parsing doesn't know or care about JSON.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Escapes a string for use inside a double-quoted HTML attribute value.
///
/// One escaper, used by both the server-rendered `/embed/:slug` document and
/// the iframe snippet the copy button writes to the clipboard, so the two can
/// never disagree about what a title containing `"` or `</iframe>` does. The
/// four characters here are exactly what a double-quoted attribute can be
/// broken out of; `'` is not escaped because nothing here emits
/// single-quoted attributes.
pub fn attr_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// The framable card's path for a item. One place, so `robots.txt`, the
/// sitemap's exclusion comment, the oEmbed URL parser and the copy button all
/// mean the same route.
pub fn embed_path(slug: &str) -> String {
    format!("/embed/{slug}")
}

/// Rewrites the `/item/<x>` segment of a item page URL into `/embed/<x>`,
/// keeping whatever origin the caller had. `None` for anything that is not a
/// item page.
///
/// This exists because `ShareButton`/`EmbedButton` are handed the page URL and
/// nothing else. Deriving the embed target from it means no new prop on the
/// detail page and no second source of truth for what a item's embed URL is.
///
/// Not `url::Url`: that crate is not in the wasm build's dependency set, and
/// this only ever has to parse a URL this site itself produced.
pub fn embed_url_from_page_url(page_url: &str) -> Option<String> {
    let (origin, path) = match page_url.split_once("://") {
        // An absolute URL with no path at all is not a item page.
        Some((scheme, rest)) => {
            let (host, path) = rest.split_once('/')?;
            (format!("{scheme}://{host}"), format!("/{path}"))
        }
        None => (String::new(), page_url.to_string()),
    };
    // A query or fragment is not part of the slug. Nothing emits one on a item
    // URL today, but the clipboard path resolves against `window.location`,
    // which can carry either.
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    let slug = path.strip_prefix(crate::flavor::get().item_prefix().as_str())?;
    if slug.is_empty() || slug.contains('/') {
        return None;
    }
    Some(format!("{origin}{}", embed_path(slug)))
}

/// Default width of the embed iframe, in CSS pixels. Also the widest a
/// thumbnail gets (`storage::THUMB_MAX_EDGE`), which is not a coincidence:
/// wider than this and the card is showing an image the site never made a
/// small copy of.
pub const EMBED_DEFAULT_W: u32 = 480;

/// Height of the attribution bar under the image, in CSS pixels. Must match
/// `.se-bar`'s height in `public_route::EMBED_CSS` -- the iframe is sized from
/// the outside and the bar is the one part of the card that does not scale, so
/// a disagreement here shows up as a letterboxed or clipped bar.
pub const EMBED_BAR_H: u32 = 44;

/// ISO 8601 duration for a number of seconds, as `VideoObject.duration` and
/// the video sitemap want it: `PT1M5S`. Whole seconds; nothing here needs
/// fractions, and Google's parser does not want them.
pub fn iso8601_duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    let mut out = String::from("PT");
    if h > 0 {
        out.push_str(&format!("{h}H"));
    }
    if m > 0 {
        out.push_str(&format!("{m}M"));
    }
    if s > 0 || total == 0 {
        out.push_str(&format!("{s}S"));
    }
    out
}

/// The iframe markup for a item, at the default size.
///
/// Deliberately the *only* place this markup exists: the copy button hands it
/// to a human and `/oembed` hands it to a machine, and an embed that renders
/// differently depending on which route produced it is the failure this
/// prevents.
pub fn embed_snippet(embed_url: &str, title: &str) -> String {
    embed_iframe(
        embed_url,
        title,
        EMBED_DEFAULT_W,
        EMBED_DEFAULT_W + EMBED_BAR_H,
    )
}

/// The same markup at an explicit size, for an oEmbed consumer that asked for
/// a `maxwidth`/`maxheight`. `embed_snippet` is this with the defaults filled
/// in, rather than a second template.
///
/// No `frameborder` or `allowtransparency`: both were removed from HTML years
/// ago and only ever meant what `style="border:0"` means here. `max-width:100%`
/// so the card shrinks inside a narrower host column instead of overflowing it,
/// and `loading="lazy"` so embedding a item costs the host page nothing until
/// it scrolls into view.
pub fn embed_iframe(embed_url: &str, title: &str, width: u32, height: u32) -> String {
    format!(
        "<iframe src=\"{src}\" title=\"{title} — {site}\" width=\"{width}\" \
         height=\"{height}\" style=\"border:0;max-width:100%\" loading=\"lazy\"></iframe>",
        src = attr_escape(embed_url),
        title = attr_escape(title),
        site = attr_escape(&crate::flavor::get().name),
    )
}

/// Normalises a stored timestamp into the W3C datetime subset `<lastmod>`
/// accepts, or `None` when it cannot -- in which case the caller must omit the
/// tag entirely. An invalid `<lastmod>` is worse than none: Search Console
/// reports the whole sitemap entry as an error rather than just ignoring the
/// date.
///
/// Three shapes are accepted, because the column has held all three:
/// RFC 3339 with a zone (live data measured as
/// `2026-08-12T00:15:42.835017700+00:00` -- valid, but nine fractional digits
/// say nothing a crawler can use, so they are dropped), the legacy SQLite
/// `YYYY-MM-DD HH:MM:SS` form (no zone at all, assumed UTC, which is what
/// wrote it), and a bare `YYYY-MM-DD`.
///
/// Hand-parsed rather than `chrono::DateTime::parse_from_rfc3339`: this module
/// compiles for wasm, where chrono is not a dependency.
pub fn w3c_lastmod(raw: &str) -> Option<String> {
    let s = raw.trim();
    let b = s.as_bytes();

    let digits = |r: &[u8]| r.iter().all(u8::is_ascii_digit);
    if b.len() < 10 || !digits(&b[0..4]) || b[4] != b'-' || !digits(&b[5..7]) || b[7] != b'-' {
        return None;
    }
    if !digits(&b[8..10]) {
        return None;
    }
    if b.len() == 10 {
        return Some(s.to_string());
    }

    // Either separator: 'T' is RFC 3339, ' ' is what SQLite's own datetime()
    // writes and what the legacy rows hold.
    if b[10] != b'T' && b[10] != b' ' {
        return None;
    }
    if b.len() < 19
        || !digits(&b[11..13])
        || b[13] != b':'
        || !digits(&b[14..16])
        || b[16] != b':'
        || !digits(&b[17..19])
    {
        return None;
    }

    let zone = normalise_zone(&s[19..])?;
    Some(format!("{}T{}{}", &s[..10], &s[11..19], zone))
}

/// The trailing `[.fraction][zone]` of a timestamp, reduced to a bare zone.
/// A missing zone becomes `Z`: the W3C profile of ISO 8601 requires one once
/// the time is present, and the rows that omit it were written in UTC.
fn normalise_zone(rest: &str) -> Option<String> {
    let rest = match rest.strip_prefix('.') {
        Some(frac) => {
            let n = frac.chars().take_while(char::is_ascii_digit).count();
            if n == 0 {
                return None;
            }
            &frac[n..]
        }
        None => rest,
    };
    if rest.is_empty() || rest == "Z" || rest == "z" {
        return Some("Z".to_string());
    }
    let b = rest.as_bytes();
    let signed = b.len() == 6
        && (b[0] == b'+' || b[0] == b'-')
        && b[1..3].iter().all(u8::is_ascii_digit)
        && b[3] == b':'
        && b[4..6].iter().all(u8::is_ascii_digit);
    signed.then(|| rest.to_string())
}

/// The dimensions `image`'s `thumbnail(max, max)` will actually produce for a
/// `w`x`h` source, so oEmbed can report `thumbnail_width`/`thumbnail_height`
/// that match the bytes stored rather than a guess.
///
/// Mirrors `image::imageops::resize_dimensions(.., fill = false)` exactly:
/// the smaller of the two ratios, applied to both edges and rounded, with no
/// early return for a small source -- `thumbnail` scales *up* as well, so a
/// 300px item really does get a 480px thumbnail and claiming otherwise would
/// misreport it.
pub fn fit_within(w: u32, h: u32, max: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let ratio = f64::min(max as f64 / w as f64, max as f64 / h as f64);
    let scale = |v: u32| ((v as f64 * ratio).round() as u32).max(1);
    (scale(w), scale(h))
}

/// `schema.org/WebSite` for the site as a whole, with the `SearchAction` that
/// makes a site-search box eligible to appear under the result in Google, the
/// `alternateName`s people actually type for the brand ("Item anniversary",
/// the hyphenated form), and a `publisher` Organization carrying the logo,
/// which is what a search engine reads to show a brand mark beside results.
///
/// Site-level, and deliberately separate from the `ImageObject` a item page
/// emits: two JSON-LD blocks on one page is normal and each describes a
/// different thing. What must *not* be duplicated is `og:image` -- `leptos_meta`
/// resolves duplicate tags first-set-wins, and a site-wide default in `App` is
/// exactly what silently beat every item's own image once (commit 31390dc).
///
/// Hand-built like `json_escape`'s other callers, for the same reason: no
/// `serde_json` in the wasm build.
pub fn site_json_ld(origin: &str) -> String {
    let f = crate::flavor::get();
    let origin = json_escape(origin.trim_end_matches('/'));
    let name = json_escape(&f.name);
    let alternates = f
        .alternate_names
        .iter()
        .map(|n| format!("\"{}\"", json_escape(n)))
        .collect::<Vec<_>>()
        .join(",");
    let logo_asset = f.asset("logo-large.png");
    let logo = json_escape(&if logo_asset.starts_with("http") {
        logo_asset
    } else {
        format!("{}{logo_asset}", origin)
    });
    format!(
        "{{\"@context\":\"https://schema.org\",\"@type\":\"WebSite\",\
         \"name\":\"{name}\",\
         \"alternateName\":[{alternates}],\
         \"url\":\"{origin}/\",\
         \"publisher\":{{\"@type\":\"Organization\",\"name\":\"{name}\",\"url\":\"{origin}/\",\
         \"logo\":{{\"@type\":\"ImageObject\",\"url\":\"{logo}\",\"width\":1200,\"height\":630}}}},\
         \"potentialAction\":{{\"@type\":\"SearchAction\",\
         \"target\":{{\"@type\":\"EntryPoint\",\
         \"urlTemplate\":\"{origin}/search?q={{search_term_string}}\"}},\
         \"query-input\":\"required name=search_term_string\"}}}}"
    )
}

/// `Home > {title}` for a item page. Google renders the breadcrumb in place of
/// the raw URL in a result, which for `/item/<slug>` is the difference between
/// showing a path and showing the site's name.
///
/// Takes the page URL rather than deriving it, so it cannot disagree with the
/// canonical the same component emits.
pub fn breadcrumb_json_ld(title: &str, page_url: &str) -> String {
    let home = json_escape(&absolute("/"));
    let page = json_escape(page_url);
    let title = json_escape(title);
    let name = json_escape(&crate::flavor::get().name);
    format!(
        "{{\"@context\":\"https://schema.org\",\"@type\":\"BreadcrumbList\",\"itemListElement\":[\
         {{\"@type\":\"ListItem\",\"position\":1,\"name\":\"{name}\",\"item\":\"{home}\"}},\
         {{\"@type\":\"ListItem\",\"position\":2,\"name\":\"{title}\",\"item\":\"{page}\"}}]}}"
    )
}

/// Mounts `site_json_ld` in the document. A component rather than a bare
/// string so the app root adds it with one line and `leptos_meta` handles the
/// head placement.
///
/// The origin comes from `absolute("/")`, which is a no-op in the wasm build --
/// the same trade `image_object_json_ld` on the detail page already makes.
/// Structured data is read from the server-rendered HTML by crawlers that run
/// no JavaScript, so the server's answer is the one that matters.
#[component]
pub fn SiteJsonLd() -> impl IntoView {
    let json = site_json_ld(&absolute("/"));
    view! { <script type="application/ld+json" inner_html=json/> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_url_passes_through_unchanged() {
        assert_eq!(
            absolute("https://media.geekgallery.com/orig/x.png"),
            "https://media.geekgallery.com/orig/x.png"
        );
    }

    #[test]
    fn json_escape_neutralizes_script_close() {
        let escaped = json_escape("</script><script>alert(1)</script>");
        assert!(!escaped.contains("</script>"));
        assert!(escaped.contains("\\u003c/script\\u003e"));
    }

    #[test]
    fn json_escape_handles_quotes_and_backslashes() {
        assert_eq!(
            json_escape("a \"quote\" and \\backslash"),
            "a \\\"quote\\\" and \\\\backslash"
        );
    }

    #[test]
    fn attr_escape_covers_the_four_that_break_a_quoted_attribute() {
        assert_eq!(
            attr_escape("a \"b\" & <c> d"),
            "a &quot;b&quot; &amp; &lt;c&gt; d"
        );
    }

    #[test]
    fn embed_url_from_an_absolute_item_page() {
        assert_eq!(
            embed_url_from_page_url("https://geekgallery.com/item/item-in-a-box"),
            Some("https://geekgallery.com/embed/item-in-a-box".to_string())
        );
    }

    #[test]
    fn embed_url_from_a_relative_item_page() {
        assert_eq!(
            embed_url_from_page_url("/item/item-in-a-box"),
            Some("/embed/item-in-a-box".to_string())
        );
    }

    #[test]
    fn embed_url_ignores_a_trailing_slash_query_and_fragment() {
        assert_eq!(
            embed_url_from_page_url("http://127.0.0.1:3100/item/abc-123/"),
            Some("http://127.0.0.1:3100/embed/abc-123".to_string())
        );
        assert_eq!(
            embed_url_from_page_url("https://geekgallery.com/item/abc-123?utm=x#top"),
            Some("https://geekgallery.com/embed/abc-123".to_string())
        );
    }

    #[test]
    fn embed_url_rejects_anything_that_is_not_a_item_page() {
        assert_eq!(embed_url_from_page_url("/upload"), None);
        assert_eq!(
            embed_url_from_page_url("https://geekgallery.com/upload"),
            None
        );
        assert_eq!(
            embed_url_from_page_url("https://geekgallery.com/item/"),
            None
        );
        assert_eq!(embed_url_from_page_url("https://geekgallery.com"), None);
        assert_eq!(embed_url_from_page_url("/item/a/b"), None);
    }

    /// A title is free text. If it could close the attribute it sits in, the
    /// snippet a visitor pastes into their own site would be an injection
    /// vector on *their* page, not ours.
    #[test]
    fn embed_snippet_cannot_be_broken_out_of() {
        let html = embed_snippet(
            "https://geekgallery.com/embed/x",
            "a \"item\"</iframe><script>alert(1)</script>",
        );
        assert!(!html.contains("</iframe><script>"));
        assert!(html.contains("&quot;item&quot;&lt;/iframe&gt;"));
        // Exactly one closing tag: the one this function wrote.
        assert_eq!(html.matches("</iframe>").count(), 1);
    }

    #[test]
    fn embed_snippet_is_the_default_size() {
        let html = embed_snippet("https://geekgallery.com/embed/x", "Item in a box");
        assert!(html.contains("width=\"480\" height=\"524\""));
        assert!(html.contains("src=\"https://geekgallery.com/embed/x\""));
        assert!(!html.contains("frameborder"));
    }

    #[test]
    fn lastmod_truncates_rfc3339_fractional_seconds() {
        assert_eq!(
            w3c_lastmod("2026-08-12T00:15:42.835017700+00:00").as_deref(),
            Some("2026-08-12T00:15:42+00:00")
        );
        assert_eq!(
            w3c_lastmod("2026-08-12T00:15:42.835Z").as_deref(),
            Some("2026-08-12T00:15:42Z")
        );
        assert_eq!(
            w3c_lastmod("2026-08-12T00:15:42Z").as_deref(),
            Some("2026-08-12T00:15:42Z")
        );
    }

    #[test]
    fn lastmod_converts_the_legacy_sqlite_form() {
        assert_eq!(
            w3c_lastmod("2025-01-09 13:04:55").as_deref(),
            Some("2025-01-09T13:04:55Z")
        );
    }

    #[test]
    fn lastmod_accepts_a_bare_date() {
        assert_eq!(w3c_lastmod("2025-01-09").as_deref(), Some("2025-01-09"));
    }

    #[test]
    fn lastmod_rejects_rather_than_guessing() {
        // Each of these would produce a sitemap Google reports as an error.
        assert_eq!(w3c_lastmod(""), None);
        assert_eq!(w3c_lastmod("not a date"), None);
        assert_eq!(w3c_lastmod("09/01/2025"), None);
        assert_eq!(w3c_lastmod("2025-01-09X13:04:55"), None);
        assert_eq!(w3c_lastmod("2025-01-09 13:04"), None);
        assert_eq!(w3c_lastmod("2025-01-09 13:04:55+0100"), None);
        assert_eq!(w3c_lastmod("2025-01-09 13:04:55."), None);
    }

    #[test]
    fn fit_within_preserves_aspect_both_ways() {
        // The two shapes actually in the collection.
        assert_eq!(fit_within(1024, 1024, 480), (480, 480));
        assert_eq!(fit_within(1919, 1080, 480), (480, 270));
        assert_eq!(fit_within(1080, 1919, 480), (270, 480));
        // `thumbnail` scales up too; reporting the source size would be a lie.
        assert_eq!(fit_within(240, 240, 480), (480, 480));
        assert_eq!(fit_within(0, 100, 480), (0, 0));
    }

    #[test]
    fn durations_are_iso8601() {
        assert_eq!(iso8601_duration(0.0), "PT0S");
        assert_eq!(iso8601_duration(12.4), "PT12S");
        assert_eq!(iso8601_duration(65.0), "PT1M5S");
        assert_eq!(iso8601_duration(3600.0), "PT1H");
        assert_eq!(iso8601_duration(3725.6), "PT1H2M6S");
    }

    #[test]
    fn site_json_ld_carries_the_search_action() {
        let json = site_json_ld("https://geekgallery.com/");
        assert!(json.contains("\"url\":\"https://geekgallery.com/\""));
        assert!(json.contains("https://geekgallery.com/search?q={search_term_string}"));
        assert!(json.contains("required name=search_term_string"));
        assert!(json.contains("\"alternateName\":[]"));
        assert!(json.contains("https://geekgallery.com/brand/logo-large.png"));
        // Still one valid JSON document.
        serde_json::from_str::<serde_json::Value>(&json).expect("site JSON-LD parses");
    }

    #[test]
    fn breadcrumb_json_ld_escapes_a_hostile_title() {
        let json = breadcrumb_json_ld("</script>x", "https://geekgallery.com/item/x");
        assert!(!json.contains("</script>"));
        assert!(json.contains("\"position\":2"));
    }
}
