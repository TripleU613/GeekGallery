//! `POST /api/import` -- a item from a link instead of a file.
//!
//! The body is JSON: `{ "url": "...", "title": "...", "captcha": "..." }`,
//! the last being the Turnstile token (see `captcha.rs`). The URL is handed to
//! `fetch`, which turns a tweet, a reel, a YouTube page or a bare media URL
//! into bytes plus whatever it could learn about them (a title, a frame size,
//! a length, a poster frame). From there it is the upload pipeline, untouched:
//! the same sniffing, hashing, duplicate check, storage and row insert as a
//! file that came in through the form, with the source URL recorded alongside.
//!
//! Like an upload it answers 202 with a job id, and the browser polls
//! `/api/upload/status/:id`. The fetch is the slow part -- a 60MB clip from a
//! video site can take a minute -- and the job registry is how the page shows
//! it is happening rather than hung.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::models::UploadResult;

#[derive(Deserialize)]
pub struct ImportRequest {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub captcha: String,
}

/// Longer than any real share link, shorter than anything hostile.
const MAX_URL_LEN: usize = 2048;

/// Whether a string is the shape of link this importer will even try.
///
/// Shared with the admin bulk form, which filters a pasted list with it
/// rather than queueing junk lines and reporting 40 failures.
pub fn is_web_link(url: &str) -> bool {
    !url.is_empty()
        && url.len() <= MAX_URL_LEN
        && (url.starts_with("https://") || url.starts_with("http://"))
}

/// Queue one link and return the job id to poll.
///
/// The whole of the import, past the point where it stops mattering how the
/// link arrived: `fetch` (so the SSRF guard and the platform allowlist apply
/// however it was submitted), then the ordinary upload pipeline with the
/// source URL recorded, so the piece credits where it came from.
///
/// `title` empty means "use whatever the source called it".
pub fn queue(url: String, title: String, uploader: Option<crate::models::User>) -> String {
    let job = crate::jobs::start();
    let job_id = job.clone();
    crate::jobs::spawn(job.clone(), async move {
        use crate::jobs::set;
        use crate::models::Progress;

        let fetched = match crate::fetch::fetch(&url).await {
            Ok(f) => f,
            Err(crate::fetch::FetchError::Refused(reason)) => {
                set(&job_id, Progress::Rejected { reason });
                return;
            }
            Err(crate::fetch::FetchError::Failed(message)) => {
                tracing::error!(url, "import failed: {message}");
                set(&job_id, Progress::Failed { message });
                return;
            }
        };

        let fields = crate::upload_route::Fields {
            file: Some(fetched.bytes),
            poster: fetched.poster,
            // The visitor's title wins; the source's is the fallback, which is
            // most of the time -- nobody retypes a caption they can see.
            title: if title.trim().is_empty() {
                fetched.title.unwrap_or_default()
            } else {
                title
            },
            width: fetched.width,
            height: fetched.height,
            duration: fetched.duration,
        };
        crate::upload_route::run(job_id, fields, uploader, Some(fetched.source_url)).await;
    });
    job
}

pub async fn import(
    headers: axum::http::HeaderMap,
    Json(req): Json<ImportRequest>,
) -> impl IntoResponse {
    let url = req.url.trim().to_string();
    if url.is_empty() || url.len() > MAX_URL_LEN {
        return bad(StatusCode::BAD_REQUEST, "paste a link first".into());
    }
    if !is_web_link(&url) {
        return bad(
            StatusCode::BAD_REQUEST,
            "that does not look like a web link".into(),
        );
    }

    if let Err(message) =
        crate::captcha::check(&req.captcha, crate::captcha::remote_ip(&headers)).await
    {
        return bad(StatusCode::FORBIDDEN, message);
    }

    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok());
    let uploader = crate::auth::current_user(cookie_header)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("could not resolve importer from session: {e}");
            None
        });

    let job = queue(url, req.title, uploader);

    (StatusCode::ACCEPTED, Json(UploadResult::Queued { job }))
}

fn bad(code: StatusCode, message: String) -> (StatusCode, Json<UploadResult>) {
    (code, Json(UploadResult::Error { message }))
}

#[cfg(test)]
mod tests {
    use super::is_web_link;

    #[test]
    fn only_http_links_of_a_sane_length_are_tried() {
        assert!(is_web_link("https://example.com/a.png"));
        assert!(is_web_link("http://example.com/a.png"));
        assert!(!is_web_link(""));
        assert!(!is_web_link("example.com/a.png"));
        assert!(!is_web_link("javascript:alert(1)"));
        assert!(!is_web_link("file:///etc/passwd"));
        let long = format!("https://example.com/{}", "x".repeat(4096));
        assert!(!is_web_link(&long));
    }
}
