//! Server functions — the typed RPC the wasm bundle calls.
//!
//! The upload itself is NOT here: multipart file bodies go through a plain Axum
//! route (`src/upload_route.rs`) because server functions would have to buffer
//! the whole encoded body through a serde round-trip.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::models::{Item, ItemPage, User};
// Sort::from_str_or_default only runs server-side (the client sends the sort
// as a plain string over the wire), so the type itself is ssr-only here.
#[cfg(feature = "ssr")]
use crate::models::Sort;

/// Who is acting on this request, for everything keyed to a person: which items
/// come back marked as already liked, and whether a like or a report is allowed
/// at all.
///
/// This used to be an anonymous ID in a `item_voter` cookie, minted on the first
/// like. It is now the signed-in user's id, because an identity the holder can
/// reissue at will is not one a write can be attributed to: clearing that cookie
/// between clicks let one person like a item unboundedly, and — worse, since
/// auto-hide is the *primary* moderation mechanism here — let one person file
/// the three reports that pull any item out of the gallery.
///
/// Reads use the same identity, so `liked_by_me` describes the account and not
/// the browser. A signed-out visitor therefore sees every item as un-liked, which
/// is the honest answer now that they cannot un-like one: the alternative is a
/// filled-in tear that does nothing when clicked.
///
/// Rows already written against anonymous voter ids stay exactly where they are.
/// They keep counting — `items.likes` and `items.reports` are recomputed from
/// `COUNT(*)` rather than incremented, so nothing double-counts and no total
/// moves — they are simply no longer attributable to anyone, and so nobody can
/// un-like them. Deleting them would quietly discard real likes to make the data
/// model tidier, which is the wrong way round.
#[cfg(feature = "ssr")]
async fn current_actor() -> Option<String> {
    crate::auth::session_user_id(cookie_header().await.as_deref())
}

/// Where to send someone who has to sign in before an action will work.
///
/// The same shape as the header's sign-in link, so both land the visitor back on
/// the page they were reading. `return_to` is always a same-origin path and is
/// re-checked as one server-side in `oauth_route::login` — the encoding here is
/// only to stop a path with a `?`, `&` or `#` in it truncating the query value.
pub fn sign_in_href(return_to: &str) -> String {
    format!("/auth/google/login?return_to={}", encode_query(return_to))
}

/// Percent-encode a value for a query string.
///
/// `/` is left alone because every current caller encodes a same-origin path and
/// escaping the separators makes those unreadable in a location bar for no gain.
/// Public so the search page can rebuild its own URL for a retry link -- it had
/// no way to, so its "Try again" went to the gallery instead of re-running the
/// search it was offering to retry.
pub fn encode_query(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

/// One page of the gallery. `cursor` is the previous page's `next_cursor`.
#[server(ListItems, "/api")]
pub async fn list_items(
    cursor: Option<String>,
    sort: Option<String>,
) -> Result<ItemPage, ServerFnError> {
    let sort = sort
        .as_deref()
        .map(Sort::from_str_or_default)
        .unwrap_or_default();
    let actor = current_actor().await;

    crate::db::list_public(cursor.as_deref(), sort, actor.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server(GetItem, "/api")]
pub async fn get_item(id: String) -> Result<Option<Item>, ServerFnError> {
    let actor = current_actor().await;
    let item = crate::db::get(&id, actor.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // A hidden item must 404 by direct link too, not just vanish from the grid.
    // Direct links are how these spread, so leaving them reachable would make
    // the auto-hide safety valve decorative.
    //
    // Admins excepted, and not as a convenience: every row in the report queue
    // links to its item, and for a hidden one that link 404'd -- so the one person
    // who has to look at a item before deciding whether to delete it was the one
    // person who could not. The check is here rather than in the component
    // because this is where the answer is decided; a client-side "am I an admin"
    // would just be a request for the same data with the gate removed.
    if item.as_ref().is_some_and(|s| !s.is_public) && require_admin().await.is_err() {
        return Ok(None);
    }
    Ok(item)
}

#[server(TotalItems, "/api")]
pub async fn total_items() -> Result<i64, ServerFnError> {
    crate::db::count_public()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Public items by kind, as `(images, gifs, videos)`. Feeds the home page's
/// structured data and nothing a visitor waits on.
#[server(ItemCounts, "/api")]
pub async fn item_counts() -> Result<(i64, i64, i64), ServerFnError> {
    crate::db::counts_by_kind()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server(Leaderboard, "/api")]
pub async fn leaderboard() -> Result<Vec<crate::models::LeaderboardEntry>, ServerFnError> {
    crate::db::leaderboard(50)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server(ItemOfTheDay, "/api")]
pub async fn item_of_the_day() -> Result<Option<Item>, ServerFnError> {
    crate::db::item_of_the_day()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server(SearchItems, "/api")]
pub async fn search_items(query: String) -> Result<Vec<Item>, ServerFnError> {
    let actor = current_actor().await;
    crate::db::search_items(&query, actor.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// The items either side of this one, as `(newer, older)` slugs, for stepping
/// between detail pages without going back to the grid.
///
/// A plain tuple rather than a named struct: it needs no new shared type, and
/// the two positions are named at every call site by the `let (newer, older)`
/// that receives them. Takes the same slug-or-id string the route carries, like
/// `get_item`. Nothing here is per-visitor, so unlike `get_item` it is identical
/// for everyone and cheap to serve.
#[server(ItemNeighbours, "/api")]
pub async fn item_neighbours(
    id: String,
) -> Result<(Option<String>, Option<String>), ServerFnError> {
    crate::db::neighbours(&id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// What a like attempt actually did.
///
/// `SignInRequired` is a variant rather than an `Err`, because the caller has to
/// tell "you need an account" apart from "that request failed" and those are
/// different UI: the first is a sign-in link, the second is a rollback and a log
/// line. Folded into one `ServerFnError` they can only be told apart by matching
/// on message text, and whichever way that match goes wrong, a signed-out
/// visitor ends up with a button that appears to do nothing — the exact dead
/// control requiring sign-in was meant to remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LikeOutcome {
    /// Recorded. `count` is re-read from the database, not the client's guess.
    Toggled { count: i64, liked: bool },
    /// Nobody is signed in. Nothing was written.
    SignInRequired,
}

/// Toggle a like. Signed-in visitors only.
///
/// Enforced here rather than by hiding the button, because a server function is
/// a plain HTTP endpoint that anything can POST to: a control no signed-out
/// visitor sees is not the same thing as a request no signed-out visitor can
/// make, and only the second one is a gate.
#[server(LikeItem, "/api")]
pub async fn like_item(id: String) -> Result<LikeOutcome, ServerFnError> {
    let Some(actor) = current_actor().await else {
        return Ok(LikeOutcome::SignInRequired);
    };

    let (count, liked) = crate::db::toggle_like(&id, &actor)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(LikeOutcome::Toggled { count, liked })
}

/// The raw Cookie header for the in-flight request. Shared by every server fn
/// that needs to read a cookie (voter, session), so the `leptos_axum::extract`
/// dance lives in exactly one place.
#[cfg(feature = "ssr")]
async fn cookie_header() -> Option<String> {
    let headers: axum::http::HeaderMap = leptos_axum::extract().await.ok()?;
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// The Turnstile site key the upload page renders its captcha with, or
/// `None` when the gate is off (see `captcha.rs`). Public by nature -- it is
/// in the HTML of every site that uses one -- so no session is needed.
#[server(CaptchaSiteKey, "/api")]
pub async fn captcha_site_key() -> Result<Option<String>, ServerFnError> {
    Ok(crate::captcha::site_key())
}

/// Who, if anyone, is signed in — for the nav to show a sign-in link or an
/// avatar. `Ok(None)` covers both "never signed in" and "Google sign-in isn't
/// configured yet"; the nav doesn't need to tell those apart.
#[server(CurrentUser, "/api")]
pub async fn current_user() -> Result<Option<User>, ServerFnError> {
    crate::auth::current_user(cookie_header().await.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Require an admin session, for every admin-only server fn below. Checked
/// here, not just hidden behind a UI element — a button no ordinary visitor
/// sees is not the same thing as a request no ordinary visitor can make, and
/// the admin route is otherwise a plain server fn like any other.
#[cfg(feature = "ssr")]
async fn require_admin() -> Result<User, ServerFnError> {
    let user = crate::auth::current_user(cookie_header().await.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    match user {
        Some(u) if u.is_admin => Ok(u),
        _ => Err(ServerFnError::new("admin access required")),
    }
}

/// Same two states as `LikeOutcome`, for the same reason: the form has to show
/// a sign-in link, not "Flagged. Someone will look." over a report nobody filed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportOutcome {
    Recorded,
    SignInRequired,
}
/// Flag a item for review. Signed-in visitors only.
///
/// It used to be open, on the reasoning that a false report only costs one
/// hidden meme and that requiring accounts means nobody reports anything. What
/// changed is the other side of that trade: `/admin` and the three-report
/// auto-hide are now the primary moderation mechanism rather than a backstop, so
/// `reports`' one-per-voter primary key is the only thing standing between one
/// annoyed visitor and any item they like being pulled from the gallery — and
/// against a self-issued cookie that key is worth nothing, since three clears of
/// `item_voter` were three distinct voters. Against an account it is worth what
/// it claims to be.
#[server(ReportItem, "/api")]
pub async fn report_item(
    id: String,
    reason: String,
    message: Option<String>,
) -> Result<ReportOutcome, ServerFnError> {
    let Some(actor) = current_actor().await else {
        return Ok(ReportOutcome::SignInRequired);
    };
    let reason = crate::models::ReportReason::from_str_or_default(&reason);
    // A blank textarea should store as absent, not as an empty string forever
    // shown in the queue.
    let message = message
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());

    crate::db::report(&id, &actor, reason.as_str(), message.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(ReportOutcome::Recorded)
}

/// The report queue, or the reason it is not being shown.
///
/// Returns the refusal as data rather than as an error: the page has to be able to
/// tell "sign in" from "your account cannot see this", and an error string cannot be
/// matched on safely. The gate itself is unchanged -- `require_admin` still decides,
/// and every mutating admin fn below still calls it.
#[server(AdminFlaggedItems, "/api")]
pub async fn admin_flagged_items() -> Result<crate::models::AdminQueue, ServerFnError> {
    use crate::models::AdminQueue;

    let user = crate::auth::current_user(cookie_header().await.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    match user {
        None => return Ok(AdminQueue::SignInRequired),
        Some(u) if !u.is_admin => return Ok(AdminQueue::Denied),
        Some(_) => {}
    }

    crate::db::flagged_items()
        .await
        .map(AdminQueue::Queue)
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server(AdminSetPublic, "/api")]
pub async fn admin_set_public(id: String, public: bool) -> Result<(), ServerFnError> {
    require_admin().await?;
    crate::db::set_public(&id, public)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Removes the row and every R2 object under the item's id -- original, thumbnail
/// and poster. There is no undo — the confirmation happens client-side (see the
/// admin page), not here, since a server fn has no way to ask "are you sure"
/// mid-request.
#[server(AdminDeleteItem, "/api")]
pub async fn admin_delete_item(id: String) -> Result<(), ServerFnError> {
    require_admin().await?;
    crate::storage::remove(&id).await;
    crate::db::delete_item(&id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}
