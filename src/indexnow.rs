//! Tell search engines about a new page the moment it exists.
//!
//! IndexNow is the shared push protocol behind Bing, Yandex, Seznam, Naver and
//! Yep: one POST with a list of URLs and every participating engine has them
//! within minutes, instead of whenever the crawler next reads the sitemap.
//! Google does not take part; for Google the levers are the sitemap, the
//! `lastmod`s, and Search Console -- all of which this site already has.
//!
//! Switched on by `INDEXNOW_KEY`: any 8-128 character hex-ish string. The
//! protocol proves ownership by fetching `https://<host>/<key>.txt` and
//! expecting the key back, which `key_file` serves. Nothing here is a secret
//! -- the key is by design public at that URL -- it is only env-gated so an
//! unconfigured dev box never pings anyone about localhost.
//!
//! Fire-and-forget: a ping that fails is logged and forgotten. Publishing a
//! item must never wait on, or fail because of, a search engine.

use axum::http::StatusCode;
use axum::response::IntoResponse;

/// `None` when unset or too short to be valid under the protocol.
pub fn key() -> Option<String> {
    std::env::var("INDEXNOW_KEY").ok().filter(|k| {
        (8..=128).contains(&k.len()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// `GET /<key>.txt`. Mounted at the exact path by `main.rs`, so by the time
/// this runs the name has already matched; it only has to say the key back.
pub async fn key_file() -> impl IntoResponse {
    match key() {
        Some(k) => (StatusCode::OK, [("content-type", "text/plain")], k).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Submit absolute URLs. Spawned, not awaited, by callers.
pub async fn submit(urls: Vec<String>) {
    let Some(key) = key() else { return };
    if urls.is_empty() {
        return;
    }
    let host = match crate::public_route::site_origin()
        .split_once("://")
        .map(|(_, h)| h.trim_end_matches('/').to_string())
    {
        Some(h) if !h.starts_with("127.") && !h.starts_with("localhost") => h,
        _ => return,
    };
    let body = serde_json::json!({
        "host": host,
        "key": key,
        "keyLocation": format!("https://{host}/{key}.txt"),
        "urlList": urls,
    });
    // api.indexnow.org fans out to every participating engine, so one call is
    // enough; hitting bing.com and yandex.com separately would double-report.
    match reqwest::Client::new()
        .post("https://api.indexnow.org/indexnow")
        .timeout(std::time::Duration::from_secs(10))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 202 => {
            tracing::info!(
                n = body["urlList"].as_array().map_or(0, Vec::len),
                "indexnow: submitted"
            );
        }
        Ok(resp) => tracing::warn!(status = %resp.status(), "indexnow: refused"),
        Err(e) => tracing::warn!("indexnow: unreachable: {e}"),
    }
}

/// The pages that change when a item is published: its own page, and the
/// gallery that now lists it.
pub fn urls_for_new_item(slug: &str) -> Vec<String> {
    vec![
        crate::seo::absolute(&crate::flavor::get().item_path(slug)),
        crate::seo::absolute("/"),
    ]
}
