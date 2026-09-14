//! Take a deleted item's media out of the CDN's edge cache.
//!
//! Stored media is served from the bucket's public domain with a one-year
//! `immutable` cache lifetime, which is the right answer for objects that
//! never change -- and the wrong answer the moment an admin deletes one.
//! Removing the object from the bucket only stops the *next* miss; every edge
//! that already holds the thumbnail keeps answering with it, and an unfurl or
//! a direct link to something an admin pulled for cause goes on working for as
//! long as that edge cares to keep it. Seven items deleted for being porn were
//! still coming back `HIT 200` two hours later, and would have for a year.
//!
//! So a delete is two steps: remove the objects, then ask the CDN to forget
//! their URLs. Switched on by two keys in the flavor:
//!
//! - `CF_ZONE_ID`             -- the zone the media domain lives in;
//! - `CF_CACHE_PURGE_TOKEN`   -- an API token whose only permission is
//!   "Zone > Cache Purge" on that zone.
//!
//! Absent either, this does nothing, which is right for a dev box serving
//! `./uploads` off its own port. Best-effort: a purge that fails is logged and
//! the delete still stands; the objects are gone from the origin either way.

const PURGE_URL: &str = "https://api.cloudflare.com/client/v4/zones";

/// The zone and token, or `None` when the flavor does not carry both.
fn configured() -> Option<(String, String)> {
    let zone = std::env::var("CF_ZONE_ID")
        .ok()
        .map(|z| z.trim().to_string())
        .filter(|z| !z.is_empty())?;
    let token = std::env::var("CF_CACHE_PURGE_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())?;
    Some((zone, token))
}

/// Every URL an item's media could have been served from: one per key in
/// `storage::all_keys`, so the list needs no lookup and misses nothing --
/// purging a URL that was never cached is a no-op at the edge.
pub fn urls_for(public_url: impl Fn(&str) -> String, id: &str) -> Vec<String> {
    crate::storage::all_keys(id)
        .iter()
        .map(|k| public_url(k))
        .collect()
}

/// Forget every URL under `id` at the edge. Awaited by the delete, so the
/// admin's "gone" means gone; bounded so a slow API cannot hang the request.
pub async fn purge_item(id: &str) {
    let Some((zone, token)) = configured() else {
        return;
    };
    let backend = crate::storage::backend();
    let files = urls_for(|k| backend.public_url(k), id);
    purge(&zone, &token, files).await;
}

async fn purge(zone: &str, token: &str, files: Vec<String>) {
    if files.is_empty() {
        return;
    }
    let n = files.len();
    let body = serde_json::json!({ "files": files });
    match reqwest::Client::new()
        .post(format!("{PURGE_URL}/{zone}/purge_cache"))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(10))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!(n, "edge cache: purged");
        }
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(%status, body = %text.chars().take(300).collect::<String>(), "edge cache: purge refused");
        }
        Err(e) => tracing::warn!("edge cache: purge unreachable: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_url_per_possible_key() {
        let urls = urls_for(|k| format!("https://media.example.com/{k}"), "abc");
        assert_eq!(urls.len(), crate::storage::all_keys("abc").len());
        assert!(urls.contains(&"https://media.example.com/thumb/abc.jpg".to_string()));
        assert!(urls.contains(&"https://media.example.com/orig/abc.mp4".to_string()));
        assert!(urls.contains(&"https://media.example.com/poster/abc.jpg".to_string()));
        assert!(urls
            .iter()
            .all(|u| u.starts_with("https://media.example.com/")));
    }
}
