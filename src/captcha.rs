//! The captcha in front of `POST /api/upload` and `POST /api/import`.
//!
//! Cloudflare Turnstile: the page renders a small widget, the visitor's
//! browser solves it (usually silently, sometimes with one checkbox), and the
//! resulting token travels with the upload or the import. The server hands
//! that token to Cloudflare's `siteverify` endpoint and refuses the request
//! unless Cloudflare vouches for it. A script driving the API directly never
//! has a token, which is the whole point: a node script with a rotating VPN
//! posted a few hundred images in an afternoon before this existed, at a rate
//! the edge rate limit alone could only slow down.
//!
//! Two environment variables, both or neither:
//!
//! - `TURNSTILE_SITE_KEY` -- public, rendered into the page;
//! - `TURNSTILE_SECRET`   -- server-side only, sent to `siteverify`.
//!
//! With either missing the gate is off: the page shows no widget and the
//! routes accept requests without a token. That is the local-dev and test
//! configuration, not a production one.
//!
//! Tokens are single-use. The client resets the widget after every submit,
//! whatever the outcome, because the server has already spent the token by
//! the time it answers.

use std::time::Duration;

const VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/// What the page renders. `None` means the gate is off.
pub fn site_key() -> Option<String> {
    configured(
        std::env::var("TURNSTILE_SITE_KEY").ok(),
        std::env::var("TURNSTILE_SECRET").ok(),
    )
}

/// Both values, trimmed, both non-empty -- or nothing. A site key without a
/// secret would render a widget whose token nobody checks, and a secret
/// without a site key would refuse every upload for want of a token the page
/// cannot produce. Neither half is useful alone, so neither is honoured alone.
fn configured(key: Option<String>, secret: Option<String>) -> Option<String> {
    let key = key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())?;
    let secret_set = secret.is_some_and(|s| !s.trim().is_empty());
    secret_set.then_some(key)
}

fn secret() -> Option<String> {
    std::env::var("TURNSTILE_SECRET")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .filter(|_| site_key().is_some())
}

/// Cloudflare's answer, the two fields that matter.
#[derive(Debug, serde::Deserialize)]
struct Verdict {
    success: bool,
    #[serde(default, rename = "error-codes")]
    error_codes: Vec<String>,
}

/// Accept or refuse a request on the strength of its token. `Ok(())` when the
/// gate is off, or when Cloudflare confirms the token. `Err` carries the
/// sentence the page shows the visitor.
///
/// Fails closed: if `siteverify` cannot be reached the request is refused
/// with a "try again" rather than let through. The routes this guards exist
/// to be hard to script, and an outage that opened them would be exactly the
/// moment a script would notice.
pub async fn check(token: &str, remote_ip: Option<&str>) -> Result<(), String> {
    let Some(secret) = secret() else {
        return Ok(());
    };
    let token = token.trim();
    if token.is_empty() {
        return Err("Tick the box to show you're human, then try again.".into());
    }
    // Tokens are ~2KB; anything far past that is not one.
    if token.len() > 4096 {
        return Err("That captcha answer does not look right. Reload and try again.".into());
    }

    let mut form = vec![("secret", secret.as_str()), ("response", token)];
    if let Some(ip) = remote_ip {
        form.push(("remoteip", ip));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| {
            tracing::error!("turnstile client: {e}");
            String::from("Could not check the captcha just now. Try again in a moment.")
        })?;
    let verdict: Verdict = client
        .post(VERIFY_URL)
        .form(&form)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            tracing::warn!("turnstile siteverify unreachable: {e}");
            String::from("Could not check the captcha just now. Try again in a moment.")
        })?
        .json()
        .await
        .map_err(|e| {
            tracing::warn!("turnstile siteverify answered nonsense: {e}");
            String::from("Could not check the captcha just now. Try again in a moment.")
        })?;

    if verdict.success {
        return Ok(());
    }
    tracing::info!(codes = ?verdict.error_codes, "turnstile refused a token");
    Err(
        if verdict
            .error_codes
            .iter()
            .any(|c| c == "timeout-or-duplicate")
        {
            "That captcha has expired. Tick the box again and resubmit.".into()
        } else {
            "The captcha did not check out. Tick the box again and resubmit.".into()
        },
    )
}

/// The address Cloudflare saw the visitor at, for `remoteip`. Behind the
/// tunnel every connection is from Cloudflare, so the socket address is
/// useless; the edge puts the real one in this header.
pub fn remote_ip(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_halves_or_nothing() {
        let some = |s: &str| Some(s.to_string());
        assert_eq!(
            configured(some(" 0xkey "), some("sec")),
            Some("0xkey".into())
        );
        assert_eq!(configured(some("0xkey"), None), None);
        assert_eq!(configured(some("0xkey"), some("  ")), None);
        assert_eq!(configured(None, some("sec")), None);
        assert_eq!(configured(some(""), some("sec")), None);
    }

    #[test]
    fn verdict_shapes() {
        let ok: Verdict = serde_json::from_str(r#"{"success":true,"challenge_ts":"2026-09-10T00:00:00Z","hostname":"geekgallery.com"}"#).unwrap();
        assert!(ok.success && ok.error_codes.is_empty());
        let no: Verdict =
            serde_json::from_str(r#"{"success":false,"error-codes":["timeout-or-duplicate"]}"#)
                .unwrap();
        assert!(!no.success);
        assert_eq!(no.error_codes, ["timeout-or-duplicate"]);
    }

    #[tokio::test]
    async fn off_when_unconfigured() {
        // No TURNSTILE_* in the test environment: every token passes,
        // including none at all.
        assert_eq!(check("", None).await, Ok(()));
    }
}
