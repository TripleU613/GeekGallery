//! Turning a pasted link into media bytes.
//!
//! What people paste is rarely a file: it is a tweet, a reel, a YouTube page,
//! sometimes a bare `.jpg`. This module takes any of those and comes back with
//! the original bytes plus whatever the source said about them, ready for the
//! upload pipeline in `upload_route::run`.
//!
//! Where the bytes come from, per site, in the order tried:
//!
//! - **A bare media URL**: fetched as-is, if it sniffs as something the site
//!   stores.
//! - **X / Twitter**: the syndication endpoint that powers embedded tweets.
//!   Public, first-party, JSON, no login -- the highest-bitrate MP4 variant,
//!   or the photo at `name=orig`. All Rust.
//! - **Instagram**: the post page as the Instagram app would fetch it, for its
//!   Open Graph tags; a photo comes out of `og:image`. A reel's video usually
//!   sits behind a login, so it falls through to `yt-dlp`.
//! - **Facebook**: the video page's inline JSON (`browser_native_hd_url` and
//!   friends), then `yt-dlp`.
//! - **YouTube** and the other video platforms in `PLATFORMS`: `yt-dlp`, the
//!   one tool that keeps up with them, run as a subprocess with a hard size
//!   cap. Best video plus best audio merged to MP4 by `ffmpeg`; both live in
//!   the image, pinned by hash.
//! - **Any other page**: its Open Graph / Twitter Card tags -- `og:video`,
//!   `twitter:player:stream`, `og:image` -- and then the media they name.
//!
//! Everything the server fetches itself goes through `guarded_get`, which is
//! the SSRF defence: the host is resolved first, every address checked to be
//! public, and the connection pinned to those addresses so a DNS answer cannot
//! change between the check and the connect. Redirects are followed by hand,
//! each hop checked the same way. `yt-dlp` is only ever pointed at hosts in
//! `PLATFORMS`, never at an arbitrary URL.
//!
//! A video that arrives without a frame size gets one read straight out of its
//! MP4 boxes (`mp4_info`), and a poster frame from `ffmpeg` when it is
//! present -- this is the one path where the browser is not in the loop to
//! draw one, so the server does it or the placeholder shows.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Url;

/// Nothing bigger than the biggest upload, whatever the source.
pub const MAX_BYTES: usize = crate::storage::MAX_VIDEO_BYTES;

/// A browser's UA, because several of these hosts serve a login wall to
/// anything else and the real content to this.
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
/// Instagram serves its Open Graph tags to its own app and a login form to
/// a desktop browser.
const INSTAGRAM_UA: &str = "Instagram 219.0.0.12.117 Android";

const MAX_REDIRECTS: usize = 6;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// yt-dlp on a slow video host, plus the merge.
const YTDLP_TIMEOUT: Duration = Duration::from_secs(300);

/// Hosts `yt-dlp` may be pointed at. An allowlist rather than "anything that
/// is not a direct file", because yt-dlp's generic extractor will happily
/// fetch any URL and follow it anywhere, which is exactly the class of request
/// `guarded_get` exists to prevent.
const PLATFORMS: [&str; 22] = [
    "youtube.com",
    "youtu.be",
    "youtube-nocookie.com",
    "instagram.com",
    "facebook.com",
    "fb.watch",
    "fb.com",
    "x.com",
    "twitter.com",
    "tiktok.com",
    "vm.tiktok.com",
    "reddit.com",
    "redd.it",
    "v.redd.it",
    "vimeo.com",
    "streamable.com",
    "twitch.tv",
    "clips.twitch.tv",
    "threads.net",
    "threads.com",
    "bsky.app",
    "dailymotion.com",
];

/// How a fetch can end short of bytes. `Refused` is worded for the visitor
/// and shown to them; `Failed` is ours and is logged.
#[derive(Debug)]
pub enum FetchError {
    Refused(String),
    Failed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Refused(s) | FetchError::Failed(s) => f.write_str(s),
        }
    }
}

fn refused(s: impl Into<String>) -> FetchError {
    FetchError::Refused(s.into())
}
fn failed(s: impl Into<String>) -> FetchError {
    FetchError::Failed(s.into())
}

/// What a link resolved to.
pub struct Fetched {
    pub bytes: Vec<u8>,
    /// The canonical page URL, for the `source_url` column.
    pub source_url: String,
    pub title: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
    /// A JPEG frame for a video, when something could draw one.
    pub poster: Option<Vec<u8>>,
}

/// Which extractor a URL belongs to.
#[derive(Debug, PartialEq, Eq)]
enum Site {
    X,
    Instagram,
    Facebook,
    /// YouTube and everything else in `PLATFORMS` that has no Rust path.
    Platform,
    Other,
}

/// `www.`, `m.` and `mobile.` are the same site.
fn bare_host(url: &Url) -> String {
    let h = url.host_str().unwrap_or_default().to_ascii_lowercase();
    for p in ["www.", "m.", "mobile.", "old.", "new."] {
        if let Some(rest) = h.strip_prefix(p) {
            return rest.to_string();
        }
    }
    h
}

fn is_platform(host: &str) -> bool {
    PLATFORMS
        .iter()
        .any(|p| host == *p || host.ends_with(&format!(".{p}")))
}

fn classify(url: &Url) -> Site {
    let host = bare_host(url);
    match host.as_str() {
        "x.com" | "twitter.com" => Site::X,
        "instagram.com" => Site::Instagram,
        "facebook.com" | "fb.watch" | "fb.com" => Site::Facebook,
        h if is_platform(h) => Site::Platform,
        _ => Site::Other,
    }
}

/// The entry point. See the module comment for the order of attempts.
pub async fn fetch(raw: &str) -> Result<Fetched, FetchError> {
    let url =
        Url::parse(raw.trim()).map_err(|_| refused("that is not a link this site can read"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(refused("only http and https links can be fetched"));
    }
    match classify(&url) {
        Site::X => x::fetch(&url).await,
        Site::Instagram => instagram::fetch(&url).await,
        Site::Facebook => facebook::fetch(&url).await,
        Site::Platform => ytdlp::fetch(&url).await,
        Site::Other => generic(&url).await,
    }
}

/// The text of a page, fetched with the guards a pasted link gets: resolved,
/// refused unless every address it answers on is publicly routable, pinned to
/// the addresses that were checked, and capped at `cap` bytes.
///
/// For `mirror`'s sitemap poll. That URL comes from the environment rather
/// than from a visitor, but this module is still the only place allowed to
/// make an outbound request to a URL it did not write itself (CLAUDE.md), and
/// a sitemap on somebody else's host is exactly the kind of URL that rule is
/// about.
pub async fn guarded_text(raw: &str, cap: usize) -> Result<String, FetchError> {
    let url =
        Url::parse(raw.trim()).map_err(|_| refused("that is not a link this site can read"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(refused("only http and https links can be fetched"));
    }
    let (_final_url, resp) = guarded_get(&url, UA).await?;
    body_text(resp, cap).await
}

/// A bare media URL, or a page whose Open Graph tags name one.
async fn generic(url: &Url) -> Result<Fetched, FetchError> {
    let (final_url, resp) = guarded_get(url, UA).await?;
    let ctype = content_type(&resp);
    if ctype.starts_with("text/html") || ctype.starts_with("application/xhtml") {
        let html = body_text(resp, 2 * 1024 * 1024).await?;
        let og = OpenGraph::parse(&html);
        let Some(media_url) = og.media_url() else {
            return Err(refused(
                "no picture or clip at that link -- paste the media itself, or a post that has one",
            ));
        };
        // A gallery that advertises a thumbnail still has the original on the
        // page; take that instead when it does.
        let media_url = full_size_for(&html, &media_url).unwrap_or(media_url);
        let media_url = final_url
            .join(&media_url)
            .map_err(|_| refused("that page points at a media link this site can't read"))?;
        let bytes = download_media(&media_url).await?;
        return finish(bytes, final_url.to_string(), og.title(), None, None, None).await;
    }
    let bytes = body_capped(resp).await?;
    if crate::storage::sniff(&bytes).is_none() {
        return Err(refused(
            "that link is not a picture or a clip this site stores (PNG, JPG, WEBP, GIF, MP4, WEBM)",
        ));
    }
    finish(bytes, final_url.to_string(), None, None, None, None).await
}

/// The page's own full-size copy of the picture its `og:image` advertises.
///
/// Some galleries point `og:image` at a thumbnail -- a derived "conversion"
/// kept under a path of its own -- and serve the original next to it under the
/// same media id. Importing the thumbnail would archive a 480px copy of a
/// 4000px picture, and worse: where the original is an mp4, the thumbnail is a
/// still, so the clip would arrive as a frozen frame.
///
/// Matched on the conversion's own filename stem rather than by guessing a URL
/// shape: same stem, same media item, so this cannot wander off onto a logo or
/// an avatar that happens to be bigger. `None` unless `og:image` is itself a
/// conversion, which is the only signal that a page has two sizes at all.
fn full_size_for(html: &str, og_image: &str) -> Option<String> {
    if !og_image.contains("/conversions/") {
        return None;
    }
    let file = og_image.rsplit('/').next()?;
    // `01M253MW3F4CYXPY9R4X37QEPB-grid_thumb.webp` -> the id before the
    // conversion's name.
    let id = file.split('.').next()?.split('-').next()?;
    // Short enough to match half the page by accident is not an id.
    if id.len() < 8 {
        return None;
    }
    absolute_urls(html)
        .into_iter()
        .find(|u| u.contains(id) && !u.contains("/conversions/"))
}

/// Every absolute http(s) URL in a page, with JSON's escaped slashes undone.
///
/// The unescaping is not optional: a single-page app carries its data as a
/// JSON blob inside an attribute, where every `/` arrives as `\/`, and on the
/// gallery this was written for the original's URL appears nowhere else.
fn absolute_urls(html: &str) -> Vec<String> {
    let unescaped = html.replace("\\/", "/");
    let mut out = Vec::new();
    let mut rest = unescaped.as_str();
    while let Some(i) = rest.find("http") {
        let from = &rest[i..];
        if !(from.starts_with("http://") || from.starts_with("https://")) {
            // "http" inside a word. Past it, not past the whole document.
            rest = &from["http".len()..];
            continue;
        }
        let end = from
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '&' | ')')
            })
            .unwrap_or(from.len());
        out.push(from[..end].to_string());
        rest = &from[end..];
    }
    out
}

/// Fetch a media URL and insist it is media.
async fn download_media(url: &Url) -> Result<Vec<u8>, FetchError> {
    let (_, resp) = guarded_get(url, UA).await?;
    let bytes = body_capped(resp).await?;
    if crate::storage::sniff(&bytes).is_none() {
        return Err(refused(
            "the media behind that link is not a format this site stores",
        ));
    }
    Ok(bytes)
}

/// Fill in what the source did not say: a frame size and length read from
/// the MP4 itself, and a poster frame drawn by ffmpeg.
async fn finish(
    bytes: Vec<u8>,
    source_url: String,
    title: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<f64>,
) -> Result<Fetched, FetchError> {
    let is_video = matches!(
        crate::storage::sniff(&bytes),
        Some(crate::storage::Container::Mp4 | crate::storage::Container::Webm)
    );
    let (mut width, mut height, mut duration, mut poster) = (width, height, duration, None);
    if is_video {
        // The container knows its own frame size better than the page did:
        // a site reports the size it displays at, not the variant it served.
        if let Some(info) = mp4_info(&bytes) {
            width = info.width.or(width);
            height = info.height.or(height);
            duration = duration.or(info.duration);
        }
        poster = crate::media_tools::video_poster(&bytes).await;
        if width.is_none() || height.is_none() || duration.is_none() {
            if let Some((w, h, d)) = crate::media_tools::probe(&bytes).await {
                width = width.or(w);
                height = height.or(h);
                duration = duration.or(d);
            }
        }
    }
    Ok(Fetched {
        bytes,
        source_url,
        title: title.map(|t| tidy_title(&t)).filter(|t| !t.is_empty()),
        width,
        height,
        duration,
        poster,
    })
}

/// Captions arrive with hashtags, URLs and three paragraphs; a title is one
/// line. First line, links and `#tags` dropped, whitespace collapsed; the
/// pipeline caps the length.
pub fn tidy_title(raw: &str) -> String {
    let first = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let words: Vec<&str> = first
        .split_whitespace()
        .filter(|w| !(w.starts_with("http://") || w.starts_with("https://") || w.starts_with('#')))
        .collect();
    let mut s = words.join(" ");
    // A trailing " | Site" or " - Site" is the page's, not the post's.
    for sep in [" | ", " – ", " — "] {
        if let Some(i) = s.rfind(sep) {
            if i > 8 {
                s.truncate(i);
            }
        }
    }
    s.trim()
        .trim_end_matches(['.', ',', ':', '-', '|'])
        .trim()
        .to_string()
}

// -- the guarded HTTP layer -------------------------------------------------

/// GET with the SSRF checks described at the top of the file. Returns the
/// final URL after redirects along with the response.
async fn guarded_get(url: &Url, ua: &str) -> Result<(Url, reqwest::Response), FetchError> {
    guarded_get_with(url, ua, &[]).await
}

/// `guarded_get` with extra request headers (a cookie, an app id). The
/// headers go to every hop, so only ever pass ones meant for the host asked.
async fn guarded_get_with(
    url: &Url,
    ua: &str,
    extra: &[(&str, String)],
) -> Result<(Url, reqwest::Response), FetchError> {
    let mut url = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        let host = url
            .host_str()
            .ok_or_else(|| refused("that link has no host"))?
            .to_ascii_lowercase();
        if !matches!(url.scheme(), "http" | "https") {
            return Err(refused("that link left the web"));
        }
        let port = url.port_or_known_default().unwrap_or(443);
        let addrs = public_addrs(&host, port).await?;

        let client = reqwest::Client::builder()
            .user_agent(ua)
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&host, &addrs)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(FETCH_TIMEOUT)
            .build()
            .map_err(|e| failed(format!("http client: {e}")))?;
        let build = |client: &reqwest::Client| {
            let mut req = client
                .get(url.clone())
                .header("Accept", "*/*")
                .header("Accept-Language", "en-US,en;q=0.8");
            for (k, v) in extra {
                req = req.header(*k, v.as_str());
            }
            req
        };
        let mut resp = build(&client)
            .send()
            .await
            .map_err(|e| refused(format!("could not reach {host}: {e}")))?;
        // One polite retry on a rate limit: the sites that do this lift it
        // within seconds for a client that backs off.
        if resp.status().as_u16() == 429 {
            tokio::time::sleep(Duration::from_secs(3)).await;
            resp = build(&client)
                .send()
                .await
                .map_err(|e| refused(format!("could not reach {host}: {e}")))?;
            if resp.status().as_u16() == 429 {
                return Err(refused(format!(
                    "{host} is rate-limiting this server right now; try again in a few minutes"
                )));
            }
        }

        if resp.status().is_redirection() {
            let Some(loc) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            else {
                return Err(refused(format!("{host} redirected nowhere")));
            };
            url = url
                .join(loc)
                .map_err(|_| refused(format!("{host} redirected somewhere unreadable")))?;
            continue;
        }
        if !resp.status().is_success() {
            return Err(refused(format!(
                "{host} answered {} for that link",
                resp.status().as_u16()
            )));
        }
        return Ok((url, resp));
    }
    Err(refused("that link redirects too many times"))
}

/// A form POST with the same checks as `guarded_get`, no redirects followed:
/// the API-shaped endpoints this is for answer in place or not at all.
async fn guarded_post_form(
    url: &Url,
    ua: &str,
    body: String,
    headers: &[(&str, &str)],
) -> Result<(Url, reqwest::Response), FetchError> {
    let host = url
        .host_str()
        .ok_or_else(|| refused("that link has no host"))?
        .to_ascii_lowercase();
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = public_addrs(&host, port).await?;
    let client = reqwest::Client::builder()
        .user_agent(ua)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addrs)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| failed(format!("http client: {e}")))?;
    let mut req = client
        .post(url.clone())
        .header("Accept", "*/*")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| refused(format!("could not reach {host}: {e}")))?;
    if !resp.status().is_success() {
        return Err(refused(format!(
            "{host} answered {} for that request",
            resp.status().as_u16()
        )));
    }
    Ok((url.clone(), resp))
}

/// Resolve a host and insist every address is one the public internet can
/// reach. A literal IP is refused outright: nobody pastes one by accident.
async fn public_addrs(host: &str, port: u16) -> Result<Vec<SocketAddr>, FetchError> {
    if host.parse::<IpAddr>().is_ok() || host == "localhost" || host.ends_with(".local") {
        return Err(refused("that link points inside a network, not at a site"));
    }
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| refused(format!("could not find {host}")))?
        .collect();
    if addrs.is_empty() {
        return Err(refused(format!("could not find {host}")));
    }
    if addrs.iter().any(|a| !is_public(a.ip())) {
        return Err(refused("that link points inside a network, not at a site"));
    }
    Ok(addrs)
}

/// Globally routable, as far as an address alone can say. Written out rather
/// than through `is_global`, which is unstable, and with the ranges named so
/// a reviewer can check them against the RFCs.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || a == 0 // "this" network
                || (a == 100 && (64..=127).contains(&b)) // shared / CGNAT
                || (a == 192 && b == 0) // IETF protocol assignments, incl. 192.0.0.0/24
                || (a == 198 && (b == 18 || b == 19)) // benchmarking
                || a >= 240) // reserved and 255.255.255.255
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation
                || (seg[0] == 0x0064 && seg[1] == 0xff9b) // NAT64 well-known
                || seg[0] == 0x0100) // discard-only
        }
    }
}

fn content_type(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// The whole body, or a refusal the moment it passes `MAX_BYTES`. Checks the
/// declared length first so a 2GB file is refused before a byte is read.
async fn body_capped(mut resp: reqwest::Response) -> Result<Vec<u8>, FetchError> {
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BYTES {
            return Err(refused(format!(
                "that file is {:.0} MB; the limit is {} MB",
                len as f64 / 1_048_576.0,
                MAX_BYTES / 1_048_576
            )));
        }
    }
    let mut out = Vec::with_capacity(resp.content_length().unwrap_or(1 << 20) as usize);
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| refused(format!("the download broke off: {e}")))?
    {
        out.extend_from_slice(&chunk);
        if out.len() > MAX_BYTES {
            return Err(refused(format!(
                "that file is over the {} MB limit",
                MAX_BYTES / 1_048_576
            )));
        }
    }
    if out.is_empty() {
        return Err(refused("that link returned nothing"));
    }
    Ok(out)
}

async fn body_text(mut resp: reqwest::Response, cap: usize) -> Result<String, FetchError> {
    let mut out = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| refused(format!("the page broke off: {e}")))?
    {
        out.extend_from_slice(&chunk);
        if out.len() > cap {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

// -- Open Graph -------------------------------------------------------------

/// The handful of `<meta>` tags that name a page's media and title.
#[derive(Default, Debug, PartialEq)]
pub struct OpenGraph {
    pub video: Option<String>,
    pub image: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
}

impl OpenGraph {
    pub fn parse(html: &str) -> OpenGraph {
        let mut og = OpenGraph::default();
        let mut rest = html;
        while let Some(i) = rest.find("<meta") {
            let tag_start = &rest[i..];
            let Some(end) = tag_start.find('>') else {
                break;
            };
            let tag = &tag_start[..end];
            rest = &tag_start[end + 1..];
            let key = attr(tag, "property")
                .or_else(|| attr(tag, "name"))
                .unwrap_or_default()
                .to_ascii_lowercase();
            let Some(content) = attr(tag, "content") else {
                continue;
            };
            let content = html_unescape(&content);
            if content.is_empty() {
                continue;
            }
            match key.as_str() {
                "og:video:secure_url" => og.video = Some(content),
                "og:video" | "og:video:url" | "twitter:player:stream" => {
                    if og.video.is_none() {
                        og.video = Some(content)
                    }
                }
                "og:image" | "og:image:secure_url" | "og:image:url" | "twitter:image" => {
                    if og.image.is_none() {
                        og.image = Some(content)
                    }
                }
                "og:title" | "twitter:title" => {
                    if og.title.is_none() {
                        og.title = Some(content)
                    }
                }
                "og:type" => og.kind = Some(content),
                _ => {}
            }
        }
        og
    }

    /// The clip if there is one, else the picture.
    pub fn media_url(&self) -> Option<String> {
        self.video.clone().or_else(|| self.image.clone())
    }

    pub fn title(&self) -> Option<String> {
        self.title.clone()
    }
}

/// The value of `name="..."` (or `name='...'`) inside one tag, or `None`.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        // A whole attribute name: preceded by whitespace, followed by `=`.
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[at + name.len()..].trim_start();
        if before_ok && after.starts_with('=') {
            let value = after[1..].trim_start();
            let value_at = tag.len() - value.len();
            let value = &tag[value_at..];
            let (quote, body) = match value.chars().next() {
                Some(q @ ('"' | '\'')) => (Some(q), &value[1..]),
                _ => (None, value),
            };
            let end = match quote {
                Some(q) => body.find(q).unwrap_or(body.len()),
                None => body
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(body.len()),
            };
            return Some(body[..end].to_string());
        }
        from = at + name.len();
    }
    None
}

/// The entities that appear in attribute values in practice.
fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail.find(';').filter(|n| *n <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let decoded = match ent {
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" | "#39" | "#x27" => Some('\''),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "#x2F" | "#x2f" | "#47" => Some('/'),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

// -- MP4 box reading --------------------------------------------------------

#[derive(Debug, Default, PartialEq)]
pub struct Mp4Info {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
}

/// Frame size and length from the container's own headers: `mvhd` for the
/// length, the first `tkhd` with a non-zero size for the picture. Enough of
/// the box grammar to walk `moov > trak > tkhd`; nothing is decoded.
pub fn mp4_info(bytes: &[u8]) -> Option<Mp4Info> {
    fn boxes(data: &[u8], mut pos: usize, end: usize) -> Vec<(usize, usize, [u8; 4])> {
        let mut out = Vec::new();
        while pos + 8 <= end {
            let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = data[pos + 4..pos + 8].try_into().unwrap();
            let (header, size) = match size {
                0 => (8, end - pos),
                1 => {
                    if pos + 16 > end {
                        break;
                    }
                    let big = u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap());
                    (16, usize::try_from(big).unwrap_or(usize::MAX))
                }
                n => (8, n),
            };
            if size < header || pos.checked_add(size).is_none_or(|e| e > end) {
                break;
            }
            out.push((pos + header, pos + size, kind));
            pos += size;
        }
        out
    }
    let moov = boxes(bytes, 0, bytes.len())
        .into_iter()
        .find(|(_, _, k)| k == b"moov")?;
    let mut info = Mp4Info::default();
    for (start, end, kind) in boxes(bytes, moov.0, moov.1) {
        match &kind {
            b"mvhd" => {
                let v = *bytes.get(start)?;
                let (ts, dur) = if v == 1 {
                    (
                        u32::from_be_bytes(bytes.get(start + 20..start + 24)?.try_into().ok()?),
                        u64::from_be_bytes(bytes.get(start + 24..start + 32)?.try_into().ok()?),
                    )
                } else {
                    (
                        u32::from_be_bytes(bytes.get(start + 12..start + 16)?.try_into().ok()?),
                        u64::from(u32::from_be_bytes(
                            bytes.get(start + 16..start + 20)?.try_into().ok()?,
                        )),
                    )
                };
                if ts > 0 && dur > 0 && dur != u32::MAX as u64 {
                    info.duration = Some(dur as f64 / f64::from(ts));
                }
            }
            b"trak" if info.width.is_none() => {
                for (s, _e, k) in boxes(bytes, start, end) {
                    if &k == b"tkhd" {
                        let v = *bytes.get(s)?;
                        // Width and height are the last 8 bytes of the box
                        // in both versions, as 16.16 fixed point.
                        let off = if v == 1 { 88 } else { 76 };
                        let w =
                            u32::from_be_bytes(bytes.get(s + off..s + off + 4)?.try_into().ok()?)
                                >> 16;
                        let h = u32::from_be_bytes(
                            bytes.get(s + off + 4..s + off + 8)?.try_into().ok()?,
                        ) >> 16;
                        if w > 0 && h > 0 {
                            info.width = Some(w);
                            info.height = Some(h);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (info.width.is_some() || info.duration.is_some()).then_some(info)
}

// -- X / Twitter ------------------------------------------------------------

mod x {
    use super::*;

    /// `/user/status/123`, `/i/status/123`, `/i/web/status/123`, with or
    /// without `/video/1` and a query string.
    fn status_id(url: &Url) -> Option<u64> {
        let segs: Vec<&str> = url.path_segments()?.collect();
        let i = segs
            .iter()
            .position(|s| *s == "status" || *s == "statuses")?;
        segs.get(i + 1)?
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .ok()
    }

    /// The `token` query parameter the syndication endpoint wants: the id
    /// scaled and written in base 36 with the zeros and the point dropped.
    /// The same arithmetic the embed script does; the server checks the
    /// shape, not the digits.
    pub fn token(id: u64) -> String {
        let x = (id as f64 / 1e15) * std::f64::consts::PI;
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut int = x.trunc() as u64;
        let mut frac = x - x.trunc();
        let mut s = String::new();
        if int == 0 {
            s.push('0');
        }
        let mut ip = Vec::new();
        while int > 0 {
            ip.push(digits[(int % 36) as usize] as char);
            int /= 36;
        }
        s.extend(ip.iter().rev());
        s.push('.');
        for _ in 0..12 {
            frac *= 36.0;
            let d = frac.trunc() as usize;
            s.push(digits[d.min(35)] as char);
            frac -= frac.trunc();
        }
        s.chars().filter(|c| *c != '0' && *c != '.').collect()
    }

    #[cfg(test)]
    pub fn status_id_for_test(url: &Url) -> Option<u64> {
        status_id(url)
    }

    pub async fn fetch(url: &Url) -> Result<Fetched, FetchError> {
        let Some(id) = status_id(url) else {
            return Err(refused(
                "paste the link to a specific post on X (the one with /status/ in it)",
            ));
        };
        let api = Url::parse(&format!(
            "https://cdn.syndication.twimg.com/tweet-result?id={id}&token={}",
            token(id)
        ))
        .expect("static url");
        let (_, resp) = guarded_get(&api, UA).await?;
        let text = body_text(resp, 4 * 1024 * 1024).await?;
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|_| refused("X did not hand over that post (it may be private or deleted)"))?;

        let tweet_text = v["text"].as_str().unwrap_or("").to_string();
        let user = v["user"]["screen_name"].as_str().unwrap_or("").to_string();
        let source_url = if user.is_empty() {
            url.to_string()
        } else {
            format!("https://x.com/{user}/status/{id}")
        };
        let title = {
            let t = tidy_title(&tweet_text);
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        };

        let media = v["mediaDetails"].as_array().cloned().unwrap_or_default();
        // A clip beats a picture; the clip's own still is the poster.
        let mut best: Option<(u64, String)> = None;
        let mut width = None;
        let mut height = None;
        let mut duration = None;
        for m in &media {
            if matches!(m["type"].as_str(), Some("video" | "animated_gif")) {
                if let Some(vars) = m["video_info"]["variants"].as_array() {
                    for var in vars {
                        if var["content_type"].as_str() != Some("video/mp4") {
                            continue;
                        }
                        let br = var["bitrate"].as_u64().unwrap_or(0);
                        if let Some(u) = var["url"].as_str() {
                            if best.as_ref().is_none_or(|(b, _)| br > *b) {
                                best = Some((br, u.to_string()));
                            }
                        }
                    }
                }
                width = m["original_info"]["width"].as_u64().map(|w| w as u32);
                height = m["original_info"]["height"].as_u64().map(|h| h as u32);
                duration = m["video_info"]["duration_millis"]
                    .as_f64()
                    .map(|ms| ms / 1000.0);
                if best.is_some() {
                    break;
                }
            }
        }
        if let Some((_, video_url)) = best {
            let vu =
                Url::parse(&video_url).map_err(|_| failed("X gave an unreadable video URL"))?;
            let bytes = download_media(&vu).await?;
            return finish(bytes, source_url, title, width, height, duration).await;
        }
        for m in &media {
            if m["type"].as_str() == Some("photo") {
                if let Some(u) = m["media_url_https"].as_str() {
                    // `name=orig` is the full-resolution original; the bare
                    // URL is a resized copy.
                    let orig = format!("{u}?name=orig");
                    let pu =
                        Url::parse(&orig).map_err(|_| failed("X gave an unreadable photo URL"))?;
                    let bytes = download_media(&pu).await?;
                    return finish(bytes, source_url, title, None, None, None).await;
                }
            }
        }
        Err(refused("that post has no picture or clip in it"))
    }
}

// -- Instagram ----------------------------------------------------------------

mod instagram {
    use super::*;

    /// The Instagram web app's id, sent as `X-IG-App-ID` on API calls.
    const APP_ID: &str = "936619743392459";

    /// A signed-in session, if the operator provided one. Instagram shows a
    /// reel's video to nobody anonymous from a server address; with the
    /// `sessionid` cookie of any ordinary account it shows everything a
    /// logged-in visitor sees. Set `INSTAGRAM_SESSIONID` in the deploy env.
    fn session_cookie() -> Option<String> {
        std::env::var("INSTAGRAM_SESSIONID")
            .ok()
            .map(|v| v.trim().trim_start_matches("sessionid=").to_string())
            .filter(|v| v.len() > 10 && !v.contains(';') && !v.contains(' '))
            .map(|v| format!("sessionid={v}"))
    }

    /// Headers for a request to instagram.com itself: the app id, and the
    /// session when there is one.
    fn ig_headers() -> Vec<(&'static str, String)> {
        let mut h = vec![("X-IG-App-ID", APP_ID.to_string())];
        if let Some(c) = session_cookie() {
            h.push(("Cookie", c));
        }
        h
    }

    /// A shortcode is the media id in Instagram's base-64 alphabet. Needed
    /// for the `media/<id>/info` API, which takes the number, not the code.
    pub fn media_id(shortcode: &str) -> Option<u64> {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut n: u64 = 0;
        for c in shortcode.bytes().take(11) {
            let v = ALPHABET.iter().position(|a| *a == c)? as u64;
            n = n.checked_mul(64)?.checked_add(v)?;
        }
        (n > 0).then_some(n)
    }

    /// The post's shortcode: the segment after `p`, `reel`, `reels` or `tv`.
    pub fn shortcode(url: &Url) -> Option<String> {
        let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
        let i = segs
            .iter()
            .position(|s| matches!(*s, "p" | "reel" | "reels" | "tv"))?;
        let code = segs.get(i + 1)?;
        (code.len() >= 5
            && code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .then(|| code.to_string())
    }

    /// The first `https://...` string value after `key` in a page or a script,
    /// with JSON escaping (`\/`, `&`) and `&amp;` undone. Instagram's
    /// pages carry the media URLs inside `<script>` JSON.
    pub fn url_after(text: &str, key: &str) -> Option<String> {
        let mut from = 0;
        while let Some(i) = text[from..].find(key) {
            let at = from + i + key.len();
            let rest = &text[at..];
            let start = rest.find("http")?;
            if start > 12 {
                from = at;
                continue;
            }
            let rest = &rest[start..];
            // The value ends at the first quote, tag or space that is not
            // part of a JSON escape.
            let mut end = 0;
            let b = rest.as_bytes();
            while end < b.len() {
                match b[end] {
                    b'"' | b'\'' | b'<' | b' ' | b'>' => break,
                    b'\\' => {
                        if rest[end..].starts_with("\\/") {
                            end += 2;
                        } else if rest[end..].starts_with("\\u0026") {
                            end += 6;
                        } else {
                            break;
                        }
                    }
                    _ => end += 1,
                }
            }
            let raw = &rest[..end];
            let clean = raw
                .replace("\\/", "/")
                .replace("\\u0026", "&")
                .replace("&amp;", "&");
            if clean.starts_with("https://") {
                return Some(clean);
            }
            from = at;
        }
        None
    }

    /// The embed page: what a blog gets when it pastes a post. Served without
    /// a login far more often than the post page itself.
    async fn via_embed(code: &str) -> Option<(String, Option<String>)> {
        let u = Url::parse(&format!(
            "https://www.instagram.com/p/{code}/embed/captioned/"
        ))
        .ok()?;
        let (_, resp) = guarded_get(&u, UA).await.ok()?;
        let html = body_text(resp, 3 * 1024 * 1024).await.ok()?;
        let title = OpenGraph::parse(&html).title();
        if let Some(v) = url_after(&html, "video_url") {
            return Some((v, title));
        }
        if let Some(v) = url_after(&html, "<video") {
            return Some((v, title));
        }
        if let Some(i) = html.find("EmbeddedMediaImage") {
            if let Some(src) = url_after(&html[i..], "src=") {
                return Some((src, title));
            }
        }
        url_after(&html, "display_url").map(|u| (u, title))
    }

    /// The GraphQL query the web app itself makes for a post, which answers
    /// without a login for public posts. The document id is the app's own and
    /// changes rarely; when it stops answering this path is skipped and the
    /// others still run.
    async fn via_graphql(code: &str) -> Option<(String, Option<String>)> {
        let u = Url::parse("https://www.instagram.com/graphql/query").ok()?;
        let body = format!(
            "variables={}&doc_id=8845758582119845",
            urlencode(&format!(r#"{{"shortcode":"{code}"}}"#))
        );
        let mut headers: Vec<(&str, String)> = ig_headers();
        headers.push(("X-Requested-With", "XMLHttpRequest".to_string()));
        let borrowed: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let (_, resp) = guarded_post_form(&u, UA, body, &borrowed).await.ok()?;
        let text = body_text(resp, 2 * 1024 * 1024).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let media = &v["data"]["xdt_shortcode_media"];
        if media.is_null() {
            return None;
        }
        let node = if media["edge_sidecar_to_children"]["edges"][0]["node"].is_object() {
            &media["edge_sidecar_to_children"]["edges"][0]["node"]
        } else {
            media
        };
        let title = media["edge_media_to_caption"]["edges"][0]["node"]["text"]
            .as_str()
            .map(str::to_string);
        if let Some(vu) = node["video_url"].as_str() {
            return Some((vu.to_string(), title));
        }
        node["display_url"].as_str().map(|d| (d.to_string(), title))
    }

    /// The app's own media endpoint: JSON with every rendition. Answers for
    /// a signed-in session; anonymous from a server it says `login_required`.
    async fn via_api(code: &str) -> Option<(String, Option<String>)> {
        let id = media_id(code)?;
        let u = Url::parse(&format!(
            "https://www.instagram.com/api/v1/media/{id}/info/"
        ))
        .ok()?;
        let (_, resp) = guarded_get_with(&u, INSTAGRAM_UA, &ig_headers())
            .await
            .ok()?;
        let text = body_text(resp, 2 * 1024 * 1024).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let item = &v["items"][0];
        if item.is_null() {
            return None;
        }
        // A carousel: the first slide.
        let node = if item["carousel_media"][0].is_object() {
            &item["carousel_media"][0]
        } else {
            item
        };
        let title = item["caption"]["text"].as_str().map(str::to_string);
        if let Some(vu) = node["video_versions"][0]["url"].as_str() {
            return Some((vu.to_string(), title));
        }
        node["image_versions2"]["candidates"][0]["url"]
            .as_str()
            .map(|u| (u.to_string(), title))
    }

    /// Mirrors that exist to fix Instagram embeds in chat apps: they fetch
    /// the post with their own session and answer with Open Graph tags that
    /// point at the media. Third parties, so last before yt-dlp, and only
    /// ever for the public post URL.
    const MIRRORS: [&str; 2] = ["https://ddinstagram.com", "https://kkinstagram.com"];

    async fn via_mirror(code: &str) -> Option<(String, Option<String>)> {
        for base in MIRRORS {
            let Ok(u) = Url::parse(&format!("{base}/p/{code}/")) else {
                continue;
            };
            let Ok((_, resp)) = guarded_get(&u, UA).await else {
                continue;
            };
            let Ok(html) = body_text(resp, 2 * 1024 * 1024).await else {
                continue;
            };
            let og = OpenGraph::parse(&html);
            if let Some(m) = og.media_url() {
                return Some((m, og.title()));
            }
        }
        None
    }

    fn urlencode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    pub async fn fetch(url: &Url) -> Result<Fetched, FetchError> {
        let mut clean = url.clone();
        clean.set_query(None);
        clean.set_fragment(None);
        let Some(code) = shortcode(&clean) else {
            return Err(refused(
                "paste the link to a specific Instagram post or reel (the one with /p/ or /reel/ in it)",
            ));
        };
        let canonical = format!("https://www.instagram.com/p/{code}/");
        let is_reel = url.path().contains("/reel") || url.path().contains("/tv/");

        // 1. The post page as the app fetches it: Open Graph. A photo's
        //    og:image is the photo; a reel's og:video is there some days.
        let mut page_title = None;
        if let Ok((_, resp)) = guarded_get_with(&clean, INSTAGRAM_UA, &ig_headers()).await {
            if let Ok(html) = body_text(resp, 3 * 1024 * 1024).await {
                let og = OpenGraph::parse(&html);
                page_title = og.title();
                if let Some(v) = &og.video {
                    if let Some(f) = try_media(v, canonical.clone(), page_title.clone()).await {
                        return Ok(f);
                    }
                }
                if !is_reel {
                    if let Some(i) = &og.image {
                        if let Some(f) = try_media(i, canonical.clone(), page_title.clone()).await {
                            return Ok(f);
                        }
                    }
                }
            }
        }
        // 2. The media API (needs the session, and is the one thing that
        //    always answers when there is one). 3. The embed page. 4. The
        //    GraphQL document. 5. The embed-fixing mirrors. All Rust; each is
        //    skipped the moment it does not answer.
        let attempts = [
            via_api(&code).await,
            via_embed(&code).await,
            via_graphql(&code).await,
            via_mirror(&code).await,
        ];
        for (media_url, title) in attempts.into_iter().flatten() {
            let title = title.or_else(|| page_title.clone());
            if let Some(f) = try_media(&media_url, canonical.clone(), title).await {
                return Ok(f);
            }
        }
        // 6. yt-dlp, which knows more ways in (and honours IMPORT_COOKIES_FILE).
        match ytdlp::fetch(&clean).await {
            Ok(f) => Ok(f),
            Err(e) => {
                if session_cookie().is_none() {
                    tracing::warn!(
                        "instagram import refused for {code}; no INSTAGRAM_SESSIONID is set, and \
                         Instagram shows reels to no anonymous server client"
                    );
                }
                let rate_limited = e.to_string().to_ascii_lowercase().contains("rate");
                Err(refused(if rate_limited {
                    "Instagram is rate-limiting this server right now. Give it a few minutes and paste the link again, or save the post to your phone and upload the file."
                } else {
                    "Instagram will only show that post to someone signed in. Save it to your phone and upload the file instead."
                }))
            }
        }
    }

    async fn try_media(
        media_url: &str,
        source_url: String,
        title: Option<String>,
    ) -> Option<Fetched> {
        let mu = Url::parse(media_url).ok()?;
        let bytes = download_media(&mu).await.ok()?;
        finish(bytes, source_url, title, None, None, None)
            .await
            .ok()
    }
}

// -- Facebook -----------------------------------------------------------------

mod facebook {
    use super::*;

    /// The first JSON string value for any of `keys` in the page, unescaped.
    fn json_string_after(html: &str, keys: &[&str]) -> Option<String> {
        for key in keys {
            let needle = format!("\"{key}\":\"");
            let Some(i) = html.find(&needle) else {
                continue;
            };
            let rest = &html[i + needle.len()..];
            // Find the closing quote that is not escaped.
            let mut end = None;
            let b = rest.as_bytes();
            let mut j = 0;
            while j < b.len() {
                if b[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if b[j] == b'"' {
                    end = Some(j);
                    break;
                }
                j += 1;
            }
            let raw = &rest[..end?];
            if let Ok(s) = serde_json::from_str::<String>(&format!("\"{raw}\"")) {
                if s.starts_with("https://") {
                    return Some(s);
                }
            }
        }
        None
    }

    pub async fn fetch(url: &Url) -> Result<Fetched, FetchError> {
        if let Ok((final_url, resp)) = guarded_get(url, UA).await {
            let html = body_text(resp, 4 * 1024 * 1024).await?;
            let og = OpenGraph::parse(&html);
            let direct = json_string_after(
                &html,
                &[
                    "browser_native_hd_url",
                    "playable_url_quality_hd",
                    "browser_native_sd_url",
                    "playable_url",
                ],
            )
            .or_else(|| og.video.clone());
            if let Some(v) = direct {
                if let Ok(vu) = Url::parse(&v) {
                    if let Ok(bytes) = download_media(&vu).await {
                        return finish(bytes, final_url.to_string(), og.title(), None, None, None)
                            .await;
                    }
                }
            }
            if let Some(i) = &og.image {
                // A photo post: the picture is the post.
                if og.video.is_none()
                    && !url.path().contains("/videos/")
                    && !url.path().contains("/watch")
                    && !url.path().contains("/reel")
                {
                    if let Ok(iu) = Url::parse(i) {
                        if let Ok(bytes) = download_media(&iu).await {
                            return finish(
                                bytes,
                                final_url.to_string(),
                                og.title(),
                                None,
                                None,
                                None,
                            )
                            .await;
                        }
                    }
                }
            }
        }
        match ytdlp::fetch(url).await {
            Ok(f) => Ok(f),
            Err(e) => Err(refused(format!(
                "Facebook would not hand over that video ({e}). If it is yours, download it from Facebook and upload the file.",
            ))),
        }
    }
}

// -- yt-dlp -------------------------------------------------------------------

mod ytdlp {
    use super::*;

    /// Where the binaries are. Overridable so a host can point at its own.
    fn bin() -> String {
        std::env::var("YTDLP_BIN").unwrap_or_else(|_| "yt-dlp".into())
    }

    pub async fn fetch(url: &Url) -> Result<Fetched, FetchError> {
        if !is_platform(&bare_host(url)) {
            return Err(refused("that site is not one this importer fetches from"));
        }
        // A quick, cheap SSRF check on the pasted host itself; yt-dlp will
        // follow the platform's own redirects from there, which is the point
        // of the allowlist above.
        let port = url.port_or_known_default().unwrap_or(443);
        public_addrs(url.host_str().unwrap_or_default(), port).await?;

        let dir = std::env::temp_dir().join(format!("item-import-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| failed(format!("tmp dir: {e}")))?;
        let result = run(url, &dir).await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        result
    }

    async fn run(url: &Url, dir: &Path) -> Result<Fetched, FetchError> {
        let out_tpl = dir.join("media.%(ext)s");
        let max = format!("{}M", MAX_BYTES / 1_048_576);
        let mut cmd = tokio::process::Command::new(bin());
        cmd.arg("--no-playlist")
            .arg("--no-warnings")
            .arg("--no-progress")
            .arg("--no-cache-dir")
            .arg("--socket-timeout")
            .arg("20")
            .arg("--retries")
            .arg("2")
            .arg("--max-filesize")
            .arg(&max)
            // Best video plus best audio that together fit, else the best
            // single file that fits, else the best there is (and let the size
            // cap decide). MP4 preferred so the browser plays it everywhere.
            .arg("-f")
            .arg("(bv*[filesize<50M]+ba[filesize<10M])/(b[filesize<60M])/bv*+ba/b")
            .arg("-S")
            .arg("res,ext:mp4:m4a")
            .arg("--merge-output-format")
            .arg("mp4")
            .arg("--remote-components")
            .arg("ejs:github")
            .arg("-o")
            .arg(&out_tpl)
            .arg("--print-json")
            .arg(url.as_str());
        if let Ok(ff) = std::env::var("FFMPEG_DIR") {
            cmd.arg("--ffmpeg-location").arg(ff);
        }
        if let Ok(cookies) = std::env::var("IMPORT_COOKIES_FILE") {
            if !cookies.is_empty() {
                cmd.arg("--cookies").arg(cookies);
            }
        }
        cmd.kill_on_drop(true);
        let output = match tokio::time::timeout(YTDLP_TIMEOUT, cmd.output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(failed(
                    "this server cannot fetch from video sites yet (yt-dlp is not installed)",
                ))
            }
            Ok(Err(e)) => return Err(failed(format!("yt-dlp: {e}"))),
            Err(_) => return Err(refused("that video took too long to fetch")),
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            return Err(explain(&stderr));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let info: serde_json::Value = stdout
            .lines()
            .rev()
            .find(|l| l.starts_with('{'))
            .and_then(|l| serde_json::from_str(l).ok())
            .unwrap_or(serde_json::Value::Null);

        // The one file in the directory is the media.
        let mut rd = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| failed(format!("read tmp: {e}")))?;
        let mut path: Option<PathBuf> = None;
        while let Ok(Some(ent)) = rd.next_entry().await {
            let p = ent.path();
            if p.extension().is_some_and(|e| e != "part" && e != "json") {
                path = Some(p);
            }
        }
        let Some(path) = path else {
            return Err(explain(&stderr));
        };
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| failed(format!("read media: {e}")))?;
        if bytes.len() > MAX_BYTES {
            return Err(refused(format!(
                "that video is over the {} MB limit",
                MAX_BYTES / 1_048_576
            )));
        }
        if crate::storage::sniff(&bytes).is_none() {
            return Err(refused(
                "that site handed back something this site cannot store",
            ));
        }

        let title = info["title"].as_str().map(str::to_string);
        let width = info["width"].as_u64().map(|w| w as u32);
        let height = info["height"].as_u64().map(|h| h as u32);
        let duration = info["duration"].as_f64();
        let source_url = info["webpage_url"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| url.to_string());
        finish(bytes, source_url, title, width, height, duration).await
    }

    /// yt-dlp's stderr, turned into one line a visitor can act on.
    fn explain(stderr: &str) -> FetchError {
        let s = stderr.to_ascii_lowercase();
        if s.contains("rate-limit")
            || s.contains("rate limit")
            || s.contains("429")
            || s.contains("too many requests")
        {
            refused("that site is rate-limiting this server right now. Give it a few minutes and try again, or save the file and upload it.")
        } else if s.contains("sign in")
            || s.contains("login")
            || s.contains("cookies")
            || s.contains("private")
        {
            refused("that site wants a login to hand over the video. Save it to your device and upload the file instead.")
        } else if s.contains("unsupported url") || s.contains("no video") {
            refused("no clip at that link that this site knows how to fetch")
        } else if s.contains("larger than max") || s.contains("max-filesize") {
            refused(format!(
                "that video is over the {} MB limit",
                MAX_BYTES / 1_048_576
            ))
        } else if s.contains("unavailable")
            || s.contains("removed")
            || s.contains("does not exist")
            || s.contains("404")
        {
            refused("that video is not available any more")
        } else if s.contains("live") {
            refused("live streams cannot be imported; wait for the upload")
        } else {
            let last = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim();
            failed(format!(
                "could not fetch that video ({})",
                last.chars().take(160).collect::<String>()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real shape this was written for: a gallery whose `og:image` is a
    /// `-grid_thumb.webp` conversion while the original sits beside it under
    /// the same id, reachable only through the JSON blob with its slashes
    /// escaped. Getting this wrong archives a thumbnail of a clip.
    #[test]
    fn the_original_wins_over_the_conversion_the_page_advertises() {
        let html = r#"<meta property="og:image" content="https://media.example.com/f7/conversions/01M253MW3F4CYXPY9R4X37QEPB-grid_thumb.webp">
            <div data-page="{&quot;media&quot;:&quot;https:\/\/media.example.com\/f7\/01M253MW3F4CYXPY9R4X37QEPB.mp4&quot;}"></div>"#;
        assert_eq!(
            full_size_for(
                html,
                "https://media.example.com/f7/conversions/01M253MW3F4CYXPY9R4X37QEPB-grid_thumb.webp"
            )
            .as_deref(),
            Some("https://media.example.com/f7/01M253MW3F4CYXPY9R4X37QEPB.mp4")
        );
    }

    /// Only a conversion implies a second size exists. An ordinary `og:image`
    /// is left exactly as the page gave it, or every page with a hyphenated
    /// filename becomes a guessing game.
    #[test]
    fn an_ordinary_og_image_is_left_alone() {
        let html = "<img src=\"https://media.example.com/my-photo-2000px.jpg\">";
        assert!(full_size_for(html, "https://media.example.com/my-photo.jpg").is_none());
        // A conversion whose original is not on the page: nothing to prefer.
        assert!(full_size_for(
            "<p>no media here</p>",
            "https://media.example.com/f7/conversions/01M253MW3F4CYXPY9R4X37QEPB-grid_thumb.webp"
        )
        .is_none());
        // An id too short to identify anything is not used as a needle.
        assert!(full_size_for(
            "<img src=\"https://media.example.com/a/b.jpg\">",
            "https://media.example.com/a/conversions/ab-thumb.jpg"
        )
        .is_none());
    }

    #[test]
    fn absolute_urls_unescapes_json_slashes_and_stops_at_delimiters() {
        let found = absolute_urls(
            "<a href=\"https://a.example/one\">x</a> {\"u\":\"https:\\/\\/b.example\\/two\"} \
             mailto:nope httpsomething https://c.example/three?x=1&amp;y=2",
        );
        assert_eq!(
            found,
            vec![
                "https://a.example/one".to_string(),
                "https://b.example/two".to_string(),
                "https://c.example/three?x=1".to_string(),
            ]
        );
    }

    #[test]
    fn hosts_route_to_the_right_extractor() {
        let c = |u: &str| classify(&Url::parse(u).unwrap());
        assert_eq!(c("https://x.com/NASA/status/1"), Site::X);
        assert_eq!(c("https://mobile.twitter.com/NASA/status/1"), Site::X);
        assert_eq!(c("https://www.instagram.com/reel/abc/"), Site::Instagram);
        assert_eq!(c("https://m.facebook.com/watch/?v=1"), Site::Facebook);
        assert_eq!(c("https://fb.watch/abc/"), Site::Facebook);
        assert_eq!(c("https://youtu.be/abc"), Site::Platform);
        assert_eq!(c("https://www.youtube.com/shorts/abc"), Site::Platform);
        assert_eq!(c("https://vm.tiktok.com/abc"), Site::Platform);
        assert_eq!(c("https://i.imgur.com/abc.jpg"), Site::Other);
        assert_eq!(c("https://example.com/page"), Site::Other);
        // A lookalike host is not the platform.
        assert!(!is_platform("notyoutube.com"));
        assert!(is_platform("music.youtube.com"));
    }

    #[test]
    fn private_addresses_are_not_public() {
        let p = |s: &str| is_public(s.parse().unwrap());
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "192.0.0.1",
            "198.18.0.1",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "2001:db8::1",
            "64:ff9b::a00:1",
        ] {
            assert!(!p(bad), "{bad} should not be public");
        }
        for good in [
            "8.8.8.8",
            "104.16.1.1",
            "2606:4700::1111",
            "2a00:1450:4001::1",
        ] {
            assert!(p(good), "{good} should be public");
        }
    }

    #[test]
    fn open_graph_reads_the_tags_that_matter() {
        let html = r#"<html><head>
            <meta property="og:title" content="A &amp; B &#x2F; C" />
            <meta name="twitter:image" content="https://cdn/x.jpg">
            <meta content='https://cdn/v.mp4' property='og:video:secure_url'>
            <meta property="og:video" content="https://cdn/other.mp4">
            <meta property="og:type" content="video.other">
        </head></html>"#;
        let og = OpenGraph::parse(html);
        assert_eq!(og.title.as_deref(), Some("A & B / C"));
        assert_eq!(og.video.as_deref(), Some("https://cdn/v.mp4"));
        assert_eq!(og.image.as_deref(), Some("https://cdn/x.jpg"));
        assert_eq!(og.media_url().as_deref(), Some("https://cdn/v.mp4"));
        assert_eq!(og.kind.as_deref(), Some("video.other"));
        assert_eq!(OpenGraph::parse("<p>nothing</p>").media_url(), None);
    }

    #[test]
    fn attribute_reader_is_not_fooled_by_prefixes() {
        // `data-content` must not satisfy `content`.
        let tag = r#"<meta data-content="no" content="yes" property="og:title""#;
        assert_eq!(attr(tag, "content").as_deref(), Some("yes"));
        assert_eq!(attr(tag, "property").as_deref(), Some("og:title"));
        assert_eq!(attr(tag, "name"), None);
        assert_eq!(
            attr(r#"<meta content=bare property=x>"#, "content").as_deref(),
            Some("bare")
        );
    }

    #[test]
    fn titles_are_one_clean_line() {
        assert_eq!(
            tidy_title("🎵 We've got it.\n\nOn its way https://t.co/x #space"),
            "🎵 We've got it"
        );
        assert_eq!(
            tidy_title("NASA on X: \"hello\" | X"),
            "NASA on X: \"hello\""
        );
        assert_eq!(tidy_title("   "), "");
        assert_eq!(tidy_title("#only #tags"), "");
    }

    #[test]
    fn x_status_ids_and_tokens() {
        let id = |u: &str| x::status_id_for_test(&Url::parse(u).unwrap());
        assert_eq!(
            id("https://x.com/NASA/status/1491475671058681863"),
            Some(1491475671058681863)
        );
        assert_eq!(
            id("https://x.com/NASA/status/1491475671058681863/video/1?s=20"),
            Some(1491475671058681863)
        );
        assert_eq!(id("https://twitter.com/i/web/status/42"), Some(42));
        assert_eq!(id("https://x.com/NASA"), None);
        // Checked against what the embed script computes for this id.
        assert_eq!(x::token(1491475671058681863), "3m5lxayrcgmfl");
    }

    /// Live network, so ignored by default: `cargo test --features ssr
    /// -- --ignored live_` runs it. Exercises the guarded client, redirects,
    /// the X syndication path, a bare file and an Open Graph page.
    #[tokio::test]
    #[ignore = "reaches the public internet"]
    async fn live_fetches_the_three_shapes() {
        // A tweet with a clip: the Rust path, no yt-dlp.
        let x = fetch("https://x.com/NASA/status/1491475671058681863")
            .await
            .expect("tweet");
        assert_eq!(
            crate::storage::sniff(&x.bytes),
            Some(crate::storage::Container::Mp4)
        );
        // The first line of the tweet is the title.
        assert_eq!(x.title.as_deref(), Some("🎵 We’ve got it"));
        assert_eq!(
            x.source_url,
            "https://x.com/NASA/status/1491475671058681863"
        );
        assert!(
            x.width.is_some() && x.duration.is_some(),
            "{:?} {:?}",
            x.width,
            x.duration
        );
        eprintln!(
            "x: {} bytes, {}x{}, {:?}s, poster {:?}",
            x.bytes.len(),
            x.width.unwrap(),
            x.height.unwrap(),
            x.duration,
            x.poster.as_ref().map(Vec::len)
        );

        // A bare file, through a redirect (http -> https).
        let f = fetch("http://geekgallery.com/logo.png")
            .await
            .expect("file");
        assert_eq!(
            crate::storage::sniff(&f.bytes),
            Some(crate::storage::Container::Png)
        );

        // A page: Open Graph names the still.
        let p = fetch("https://geekgallery.com/item/never-forget")
            .await
            .expect("page");
        assert!(crate::storage::sniff(&p.bytes).is_some());
        assert!(p.title.is_some());
        eprintln!("page: {:?} {} bytes", p.title, p.bytes.len());

        // Inside a network: refused before a byte moves.
        for bad in [
            "http://127.0.0.1:3000/",
            "http://localhost/x",
            "http://169.254.169.254/latest",
        ] {
            assert!(
                matches!(fetch(bad).await, Err(FetchError::Refused(_))),
                "{bad}"
            );
        }
        // Not media, no tags.
        assert!(matches!(
            fetch("https://example.com/").await,
            Err(FetchError::Refused(_))
        ));
    }

    #[test]
    fn instagram_shortcodes_and_escaped_urls() {
        let sc = |u: &str| instagram::shortcode(&Url::parse(u).unwrap());
        assert_eq!(
            sc("https://www.instagram.com/reel/C0hQSaMpD97/?igsh=x"),
            Some("C0hQSaMpD97".into())
        );
        assert_eq!(
            sc("https://www.instagram.com/nasagoddard/p/C0hQSaMpD97/"),
            Some("C0hQSaMpD97".into())
        );
        assert_eq!(
            sc("https://www.instagram.com/tv/ABCDEFG/"),
            Some("ABCDEFG".into())
        );
        assert_eq!(sc("https://www.instagram.com/nasa/"), None);

        let script = r#"{"video_url":"https:\/\/scontent.cdninstagram.com\/v\/t50.mp4?efg=abc&_nc=1","x":1}"#;
        assert_eq!(
            instagram::url_after(script, "video_url").as_deref(),
            Some("https://scontent.cdninstagram.com/v/t50.mp4?efg=abc&_nc=1")
        );
        let img = r#"<img class="EmbeddedMediaImage" alt="" src="https://cdn/x.jpg?a=1&amp;b=2">"#;
        assert_eq!(
            instagram::url_after(&img[img.find("EmbeddedMediaImage").unwrap()..], "src=")
                .as_deref(),
            Some("https://cdn/x.jpg?a=1&b=2")
        );
        assert_eq!(instagram::url_after("nothing here", "video_url"), None);

        // The shortcode is the media id in Instagram's base 64; checked
        // against the id the app itself uses for this reel.
        assert_eq!(
            instagram::media_id("Dc8WG_3DOat"),
            Some(3980153408598107821)
        );
        assert_eq!(
            instagram::media_id("C0hQSaMpD97"),
            Some(3251952039762345851)
        );
        assert_eq!(instagram::media_id("not valid!"), None);
    }

    #[test]
    fn mp4_headers_yield_size_and_length() {
        fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(payload);
            v
        }
        // mvhd v0: version/flags(4) ctime(4) mtime(4) timescale(4) duration(4) ...
        let mut mvhd = vec![0u8; 12];
        mvhd.extend(1000u32.to_be_bytes());
        mvhd.extend(12_500u32.to_be_bytes());
        mvhd.extend([0u8; 80]);
        // tkhd v0: 84 bytes, width/height at 76..84 as 16.16.
        let mut tkhd = vec![0u8; 76];
        tkhd.extend((1280u32 << 16).to_be_bytes());
        tkhd.extend((720u32 << 16).to_be_bytes());
        let trak = bx(b"trak", &bx(b"tkhd", &tkhd));
        let mut moov_payload = bx(b"mvhd", &mvhd);
        moov_payload.extend(trak);
        let mut file = bx(b"ftyp", b"isom\0\0\x02\0isomiso2");
        file.extend(bx(b"moov", &moov_payload));
        file.extend(bx(b"mdat", &[0; 8]));
        let info = mp4_info(&file).unwrap();
        assert_eq!(info.width, Some(1280));
        assert_eq!(info.height, Some(720));
        assert_eq!(info.duration, Some(12.5));
        assert_eq!(mp4_info(b"not an mp4"), None);
    }
}
