//! `POST /api/upload` — multipart media intake.
//!
//! Order: sniff+decode+hash → duplicate check → store → insert. Nothing is
//! written until the duplicate check clears, so a rejected upload leaves no
//! files to clean up.
//!
//! There is no content screening. Uploads publish as they arrive, and the
//! report queue in `/admin` plus the three-report auto-hide are the moderation
//! mechanism, not a fallback for one. The site this grew out of ran uploads
//! through a hosted model first; that dependency is gone, on purpose, and this
//! file is shorter for it.

use crate::models::UploadResult;
use axum::extract::Multipart;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

/// Everything the browser sends alongside the file.
///
/// `poster`, `width`, `height` and `duration` only mean anything for a video,
/// and all four come from the browser: nothing on the server can open a video
/// to find out. They are trusted for what they are -- a frame to show and a
/// number to display -- and nothing security-relevant depends on them.
#[derive(Default)]
pub struct Fields {
    pub file: Option<Vec<u8>>,
    pub poster: Option<Vec<u8>>,
    pub title: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
}

/// `POST /api/upload` -- parses the multipart, then answers 202 with a job id and
/// does the slow part in the background.
///
/// The browser polls `/api/upload/status/:id` rather than holding the
/// connection: hashing and pushing a 60MB clip to R2 is long enough that a
/// proxy with a 30-second read timeout in between would kill an upload that was
/// going to succeed.
///
/// `HeaderMap` before `Multipart`: axum requires body-consuming extractors
/// (Multipart reads the request body) to come last in a handler's arguments.
pub async fn upload(headers: axum::http::HeaderMap, mut mp: Multipart) -> impl IntoResponse {
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok());
    // A cookie lookup failure (D1 unreachable) should not block the upload
    // over something unrelated to it; fall back to anonymous rather than
    // erroring the whole request.
    let uploader = crate::auth::current_user(cookie_header)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("could not resolve uploader from session: {e}");
            None
        });

    let mut fields = Fields::default();
    let mut captcha = String::new();

    loop {
        let field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return bad(StatusCode::BAD_REQUEST, format!("malformed upload: {e}")),
        };

        match field.name() {
            Some("file") => match field.bytes().await {
                Ok(b) => fields.file = Some(b.to_vec()),
                Err(e) => return bad(StatusCode::BAD_REQUEST, format!("could not read file: {e}")),
            },
            Some("poster") => match field.bytes().await {
                Ok(b) if !b.is_empty() => fields.poster = Some(b.to_vec()),
                // An empty poster part is the browser saying it could not draw
                // one; not an error.
                Ok(_) => {}
                Err(e) => {
                    return bad(
                        StatusCode::BAD_REQUEST,
                        format!("could not read poster: {e}"),
                    )
                }
            },
            Some("title") => {
                fields.title = field.text().await.unwrap_or_default();
            }
            Some("captcha") => {
                captcha = field.text().await.unwrap_or_default();
            }
            Some("width") => fields.width = field.text().await.ok().and_then(|t| t.parse().ok()),
            Some("height") => fields.height = field.text().await.ok().and_then(|t| t.parse().ok()),
            Some("duration") => {
                fields.duration = field
                    .text()
                    .await
                    .ok()
                    .and_then(|t| t.parse::<f64>().ok())
                    .filter(|d| d.is_finite() && *d > 0.0);
            }
            _ => {}
        }
    }

    // The token first, before any thought is given to the bytes: a request
    // without one is a script, and a script gets nothing but the refusal.
    if let Err(message) = crate::captcha::check(&captcha, crate::captcha::remote_ip(&headers)).await
    {
        return bad(StatusCode::FORBIDDEN, message);
    }

    if fields.file.is_none() {
        return bad(
            StatusCode::BAD_REQUEST,
            "no file in the 'file' field".into(),
        );
    }

    // Everything past here is slow, so it moves off the request. The multipart
    // had to be drained first: it borrows the request body, which does not
    // outlive this handler.
    let job = crate::jobs::start();
    let job_id = job.clone();
    crate::jobs::spawn(job.clone(), async move {
        run(job_id, fields, uploader, None).await
    });

    (StatusCode::ACCEPTED, Json(UploadResult::Queued { job }))
}

/// The pipeline. Reports each step into the job registry as it starts it, so the
/// browser's poll is describing real work rather than a timed animation.
///
/// Shared with `import_route`, which arrives here with bytes it fetched from a
/// link instead of a multipart body and the link itself as `source_url`.
pub async fn run(
    job: String,
    fields: Fields,
    uploader: Option<crate::models::User>,
    source_url: Option<String>,
) {
    use crate::jobs::set;
    use crate::models::{MediaKind, Progress, Step};

    macro_rules! step {
        ($s:expr) => {
            set(&job, Progress::Running { step: $s })
        };
    }
    macro_rules! fail {
        ($msg:expr) => {{
            set(&job, Progress::Failed { message: $msg });
            return;
        }};
    }
    macro_rules! reject {
        ($reason:expr) => {{
            set(&job, Progress::Rejected { reason: $reason });
            return;
        }};
    }

    step!(Step::Fingerprinting);

    let Fields {
        file,
        poster,
        title,
        width,
        height,
        duration,
    } = fields;
    let bytes = file.unwrap_or_default();

    // Decoding is CPU-bound and attacker-influenced; keep it off the async
    // runtime's worker threads so one huge image can't stall request handling.
    // The content hash rides along in the same blocking call, since this is the
    // one place the decoded buffer exists before storage consumes it.
    let decoded = tokio::task::spawn_blocking(move || {
        let media = crate::storage::decode(bytes, poster.as_deref())?;
        let hash = media.fingerprint();
        Ok::<_, anyhow::Error>((media, hash))
    })
    .await;

    let (media, content_hash) = match decoded {
        Ok(Ok(pair)) => pair,
        // Decode errors are worded for a visitor already ("too big", "not a
        // format this site stores"), and they describe the file, not a fault --
        // so they are a refusal, not a failure.
        Ok(Err(e)) => reject!(e.to_string()),
        Err(e) => fail!(format!("decode panicked: {e}")),
    };

    // Exact duplicate: same decoded pixels (or same bytes, for a clip) as
    // something already here. A single indexed lookup.
    match crate::db::find_by_hash(&content_hash).await {
        Ok(Some(existing)) => {
            tracing::info!(existing = existing.id, "upload rejected: exact duplicate");
            reject!(format!(
                "this exact file is already in the collection: {}",
                crate::flavor::get().item_path(&existing.slug)
            ));
        }
        Ok(None) => {}
        // A dedupe-check outage shouldn't block an otherwise-good upload;
        // log loudly and let it through rather than fail the whole request.
        Err(e) => tracing::error!("hash dedupe check failed: {e}"),
    }

    let kind = media.kind();
    let title = clean_title(&title, kind);

    step!(Step::Cropping);
    step!(Step::Storing);
    // First name only. This one is not a display string -- it becomes the
    // `Author` PNG text chunk in the original, served straight from R2, so the
    // full name would be readable by anyone who downloaded a item.
    let uploader_credit = uploader
        .as_ref()
        .map(|u| crate::db::public_first_name(&u.display_name));
    let dims = width.zip(height);
    let stored = match crate::storage::store(media, &title, uploader_credit.as_deref(), dims).await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("store failed: {e}");
            fail!("could not save this item".into());
        }
    };

    // After the title is cleaned, so the slug matches what will be displayed.
    let slug = crate::db::unique_slug(&title, &stored.id).await;

    let item = crate::db::insert(crate::db::NewItem {
        id: &stored.id,
        slug: &slug,
        title: &title,
        kind: stored.kind,
        orig_url: &stored.orig_url,
        thumb_url: &stored.thumb_url,
        poster_url: stored.poster_url.as_deref(),
        width: stored.width,
        height: stored.height,
        duration: if kind == MediaKind::Video {
            duration
        } else {
            None
        },
        content_hash: &content_hash,
        source_url: source_url.as_deref(),
        uploader_id: uploader.as_ref().map(|u| u.id.as_str()),
        // The item handed straight back to the browser that just uploaded it,
        // built in memory rather than re-read through `ItemRow`, so it misses
        // the redaction that conversion applies. Without this the uploader's
        // own new card showed their full name until the next refetch.
        uploader: uploader.as_ref().map(|u| crate::models::Uploader {
            display_name: crate::db::public_first_name(&u.display_name),
            avatar_url: u.avatar_url.clone(),
        }),
    })
    .await;

    let item = match item {
        Ok(item) => item,
        Err(e) => {
            // The row failed but the files landed. Remove them so the bucket
            // does not fill with media nothing references.
            crate::storage::remove(&stored.id).await;
            tracing::error!("insert failed: {e}");
            fail!("could not record this item".into());
        }
    };

    // Search engines are told after the fact and off this task's critical
    // path: the browser sees `Done` now, the ping happens whenever it happens.
    tokio::spawn(crate::indexnow::submit(crate::indexnow::urls_for_new_item(
        &item.slug,
    )));

    set(
        &job,
        Progress::Done {
            item: Box::new(item),
        },
    );
}

/// `GET /api/upload/status/:id`.
///
/// An unknown id answers `Failed` rather than 404: from the browser's side an
/// expired job and a job that never existed are the same situation -- nothing
/// further is coming -- and giving it one shape to handle keeps the polling loop
/// from needing a special case.
pub async fn status(axum::extract::Path(id): axum::extract::Path<String>) -> impl IntoResponse {
    match crate::jobs::get(&id) {
        Some(p) => Json(p),
        None => Json(crate::jobs::Progress::Failed {
            message: "this upload is no longer being tracked".into(),
        }),
    }
}

fn bad(code: StatusCode, message: String) -> (StatusCode, Json<UploadResult>) {
    (code, Json(UploadResult::Error { message }))
}

/// Titles are rendered as text by Leptos (which escapes), so this is about
/// keeping the gallery legible, not about injection.
fn clean_title(raw: &str, kind: crate::models::MediaKind) -> String {
    use crate::models::MediaKind;
    let t: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    if t.is_empty() {
        match kind {
            MediaKind::Video => "untitled clip".to_string(),
            MediaKind::Gif => "untitled gif".to_string(),
            MediaKind::Image => format!("untitled {}", crate::flavor::get().noun),
        }
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MediaKind;

    #[test]
    fn titles_are_trimmed_capped_and_stripped_of_controls() {
        assert_eq!(
            clean_title("  hello\u{0}world  ", MediaKind::Image),
            "helloworld"
        );
        let long = "x".repeat(200);
        assert_eq!(clean_title(&long, MediaKind::Image).chars().count(), 80);
    }

    /// An empty title says what the thing is, so a grid of untitled uploads
    /// still tells clips from stills.
    #[test]
    fn empty_titles_name_the_kind() {
        assert_eq!(clean_title("", MediaKind::Image), "untitled item");
        assert_eq!(clean_title("   ", MediaKind::Gif), "untitled gif");
        assert_eq!(clean_title("", MediaKind::Video), "untitled clip");
    }
}
