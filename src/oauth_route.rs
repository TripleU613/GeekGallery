//! `GET /auth/google/login`, `GET /auth/google/callback`, `POST /auth/logout`.
//!
//! Plain Axum routes, not server functions: a server function's response is a
//! serialized value, not an HTTP redirect with `Set-Cookie` headers, so this
//! needs the same lower-level access `upload_route.rs` uses for the same
//! reason.

use axum::extract::Query;
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

fn redirect(location: &str, set_cookies: &[String]) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::FOUND)
        .header(LOCATION, location);
    for c in set_cookies {
        if let Ok(v) = HeaderValue::from_str(c) {
            // http::response::Builder::header appends rather than replacing
            // on repeated calls with the same name, which is exactly what
            // multiple Set-Cookie headers require.
            builder = builder.header(SET_COOKIE, v);
        }
    }
    builder.body(axum::body::Body::empty()).unwrap_or_default()
}

#[derive(Deserialize)]
pub struct LoginQuery {
    return_to: Option<String>,
}

pub async fn login(Query(q): Query<LoginQuery>) -> impl IntoResponse {
    let return_to = q
        .return_to
        .filter(|p| p.starts_with('/'))
        .unwrap_or_else(|| "/".to_string());

    match crate::auth::start_login(&return_to) {
        Some((auth_url, set_cookie)) => redirect(&auth_url, &[set_cookie]),
        None => {
            tracing::warn!("someone hit /auth/google/login but Google sign-in isn't configured");
            redirect("/?login_error=not_configured", &[])
        }
    }
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    /// Present when the user declines consent on Google's screen, or Google
    /// itself errors — not a bug on this end, so it gets its own quiet path
    /// rather than falling through to the generic error log.
    error: Option<String>,
}

pub async fn callback(
    headers: axum::http::HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> impl IntoResponse {
    if let Some(err) = q.error {
        tracing::info!("Google sign-in declined or errored: {err}");
        return redirect("/?login_error=declined", &[]);
    }

    let (Some(code), Some(state)) = (q.code, q.state) else {
        return redirect("/?login_error=missing_params", &[]);
    };

    let cookie_header = headers.get(COOKIE).and_then(|v| v.to_str().ok());
    match crate::auth::complete_login(cookie_header, &code, &state).await {
        Ok(result) => {
            tracing::info!(user_id = %result.user.id, "signed in");
            redirect(
                &result.return_to,
                &[result.session_set_cookie, result.clear_flow_set_cookie],
            )
        }
        Err(e) => {
            // Logged, not surfaced: state mismatches and expired flow cookies
            // are common (a double-click, a slow consent screen) and telling
            // the visitor anything more specific than "try again" invites
            // probing the flow rather than helping them.
            tracing::warn!("Google sign-in callback failed: {e}");
            redirect("/?login_error=failed", &[])
        }
    }
}

pub async fn logout() -> impl IntoResponse {
    redirect("/", &[crate::auth::logout_set_cookie()])
}
