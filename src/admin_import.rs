//! `GET|POST /admin/import` -- paste a list of links, queue them all.
//!
//! The bulk twin of the upload page's link field, for moving a batch of media
//! in at once. Plain Axum and hand-written HTML rather than a Leptos page:
//! this is a textarea and a button, it is reached from a phone as often as a
//! desktop, and it has no reason to cost a wasm bundle or a hydration pass.
//!
//! Admin only, and no captcha. The gate is the session, which is a stronger
//! claim than a checkbox; a captcha here would only ask an admin to prove
//! they are not the script they just signed in as.
//!
//! Nothing about the import itself is new. Every link goes through
//! `import_route::queue`, which means `fetch` (so the SSRF guard and the
//! platform allowlist apply exactly as they do to a pasted link), the same
//! sniffing, hashing, duplicate check and storage, and the source URL
//! recorded on the row -- so each piece credits where it came from and a
//! takedown request has somewhere to point.
//!
//! The markup below quotes its attributes with `'` rather than `"`. Both are
//! legal HTML, and it keeps a page-sized literal free of the backslash noise
//! that makes `public_route::embed_document` hard to read.

use axum::extract::{Form, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

/// Enough for a whole small gallery in one paste, few enough that a
/// fat-fingered box cannot queue a week of fetching. Not a throughput limit:
/// `jobs::spawn` runs three at a time whatever arrives here, so a batch
/// drains at the speed one link would, one after another.
const MAX_URLS: usize = 250;

/// The signed-in admin, or `None` for everyone else.
///
/// Returned rather than reduced to a bool because the same lookup answers
/// both questions this page has: may you do this, and who gets the credit on
/// the rows.
async fn current_admin(headers: &HeaderMap) -> Option<crate::models::User> {
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok());
    match crate::auth::current_user(cookie).await {
        Ok(Some(user)) if user.is_admin => Some(user),
        Ok(_) => None,
        // A D1 outage is not an authorization decision, but it resolves to
        // the same answer here, and the safe direction is no.
        Err(e) => {
            tracing::warn!("could not resolve admin for /admin/import: {e}");
            None
        }
    }
}

/// 404, not 403: a page nobody but an admin may use has no reason to tell a
/// stranger it exists.
fn refuse() -> Response {
    (StatusCode::NOT_FOUND, "no such page").into_response()
}

/// What the redirect after a submit carries back, so the form can report what
/// happened without keeping any server-side state.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Outcome {
    queued: Option<usize>,
    skipped: Option<usize>,
}

pub async fn page(headers: HeaderMap, Query(outcome): Query<Outcome>) -> Response {
    if current_admin(&headers).await.is_none() {
        return refuse();
    }
    let note = match outcome.queued {
        Some(queued) => {
            let skipped = outcome.skipped.unwrap_or(0);
            let tail = if skipped > 0 {
                format!(" {skipped} line(s) were not web links and were skipped.")
            } else {
                String::new()
            };
            format!(
                "<p class='note'>Queued {queued} link(s). They import three at a \
                 time in the background, so give the gallery a few minutes. \
                 Anything already here is refused as a duplicate, which makes \
                 re-submitting the same list safe.{tail}</p>"
            )
        }
        None => String::new(),
    };
    Html(document(&note)).into_response()
}

#[derive(Deserialize)]
pub struct Batch {
    urls: String,
}

/// `HeaderMap` before `Form`: the extractor that reads the body comes last.
pub async fn submit(headers: HeaderMap, Form(batch): Form<Batch>) -> Response {
    let Some(admin) = current_admin(&headers).await else {
        return refuse();
    };

    let mut urls: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for line in batch.urls.lines() {
        let url = line.trim();
        if url.is_empty() {
            continue;
        }
        if !crate::import_route::is_web_link(url) {
            skipped += 1;
            continue;
        }
        // A list pasted out of a sitemap or a page carries the same link twice
        // more often than not, and a duplicate here would be two fetches to
        // learn what the content hash already knows.
        if urls.iter().any(|seen| seen.as_str() == url) {
            continue;
        }
        urls.push(url.to_string());
        if urls.len() == MAX_URLS {
            break;
        }
    }

    let queued = urls.len();
    for url in urls {
        crate::import_route::queue(url, String::new(), Some(admin.clone()));
    }
    tracing::info!("admin bulk import: {queued} queued, {skipped} skipped");

    Redirect::to(&format!("/admin/import?queued={queued}&skipped={skipped}")).into_response()
}

/// Its own small stylesheet rather than the site's: this page is served
/// outside Leptos, so it never sees `/pkg/geekgallery.css`, and a form does
/// not justify wiring the hashed asset name in by hand.
const CSS: &str = "
:root { color-scheme: dark }
body { margin: 0 auto; padding: 24px 16px 48px; max-width: 46rem;
  background: #0b0b0f; color: #e8e8ef;
  font: 15px/1.5 system-ui, -apple-system, Segoe UI, Roboto, sans-serif }
h1 { font-size: 1.375rem; margin: 0 0 4px }
p { margin: 0 0 16px }
label { display: block; margin: 0 0 8px; color: #a6a6b3; font-size: 0.875rem }
textarea { width: 100%; box-sizing: border-box; padding: 10px 12px;
  border: 1px solid #2a2a35; border-radius: 8px; background: #14141b;
  color: #e8e8ef; resize: vertical;
  font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace }
button { margin-top: 12px; padding: 11px 20px; border: 0; border-radius: 8px;
  background: #a5a3ff; color: #0b0b0f; font-size: 0.9375rem; font-weight: 700 }
.note { padding: 12px 14px; border: 1px solid #2f5d3f; border-radius: 8px;
  background: #16241b; color: #cfe9d8 }
.quiet { color: #7c7c8a; font-size: 0.8125rem }
";

fn document(note: &str) -> String {
    format!(
        "<!doctype html><html lang='en'><head><meta charset='utf-8'>\
         <meta name='viewport' content='width=device-width,initial-scale=1'>\
         <meta name='robots' content='noindex, nofollow'>\
         <title>Bulk import \u{2014} {site}</title>\
         <style>{CSS}</style></head><body>\
         <h1>Bulk import</h1>\
         <p class='quiet'>Admin only. Every link goes through the same importer \
         as the upload page, and each piece records where it came from.</p>\
         {note}\
         <form method='post' action='/admin/import'>\
         <label for='urls'>One link per line. A page link works as well as a \
         direct file link \u{2014} the importer reads the page and takes the \
         image it advertises.</label>\
         <textarea id='urls' name='urls' rows='14' spellcheck='false' \
         autocapitalize='off' autocorrect='off' \
         placeholder='https://example.com/gallery/one&#10;https://example.com/gallery/two'\
         ></textarea>\
         <button type='submit'>Queue them</button>\
         </form>\
         <p class='quiet'>Up to {MAX_URLS} links a submit.</p>\
         </body></html>",
        site = crate::flavor::get().name,
    )
}

#[cfg(test)]
mod tests {
    use super::document;

    /// The form has to post to itself and name the field the handler reads; a
    /// rename on one side alone is a page that silently queues nothing.
    #[test]
    fn the_form_posts_to_this_route_with_the_field_the_handler_expects() {
        let html = document("");
        assert!(html.contains("method='post' action='/admin/import'"));
        assert!(html.contains("name='urls'"));
        assert!(html.contains("noindex"));
    }

    #[test]
    fn the_outcome_note_is_rendered_where_one_is_given() {
        let html = document("<p class='note'>Queued 3 link(s).</p>");
        assert!(html.contains("Queued 3 link(s)."));
    }
}
