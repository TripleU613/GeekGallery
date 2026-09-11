//! One empty/zero-result state, shared by the gallery, search and leaderboard.
//!
//! Deliberately rigid: icon, one line, at most one action. Every page that
//! previously wrote its own empty state grew a subtitle and an explanatory
//! sentence ("The collection is empty. Be the first father.", "Ranked by public
//! items uploaded while signed in. Anonymous uploads don't count toward this…"),
//! which is exactly the narration the redesign has a copy budget against. A
//! component with nowhere to put a paragraph is the enforcement mechanism.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::components::icon::{Ico, LuCircleAlert, LuRefreshCw};

#[component]
pub fn EmptyState(
    icon: icondata_core::Icon,
    /// One short sentence. Not a paragraph.
    #[prop(into)]
    message: String,
    /// Optional single call to action.
    #[prop(optional, into)]
    action_href: Option<String>,
    #[prop(optional, into)] action_label: Option<String>,
) -> impl IntoView {
    let action = match (action_href, action_label) {
        (Some(href), Some(label)) => Some(view! {
            <A href=href attr:class="btn">{label}</A>
        }),
        _ => None,
    };

    view! {
        // The badge is a faint accent wash behind a muted-accent glyph. These
        // two components are the site's most-seen zero-states, and this turns
        // both of them warm without adding a single word to a page whose whole
        // design constraint is that it has no words to spare.
        //
        // `h-13 w-13` resolves now: it is a `spacing` entry in the config. It
        // used to emit nothing at all and the badge was sized by its padding
        // and glyph by accident.
        <section class="flex min-h-[46vh] animate-rise-in flex-col items-center justify-center gap-3 text-center">
            <span class="inline-flex h-13 w-13 items-center justify-center rounded-lg border border-accent-line bg-accent-veil p-3 text-accent-muted">
                <Ico icon=icon size=28/>
            </span>
            <p class="m-0 text-base font-semibold text-ink">{message}</p>
            {action}
        </section>
    }
}

/// Generic failure state: icon, one sentence, one recovery action.
///
/// Deliberately does not surface the underlying error text. The old pages
/// rendered `{e.to_string()}` straight into the page, which leaked internal
/// D1/server-fn wording at a visitor who can do nothing with it; the real
/// error is still logged server-side where it is actionable.
#[component]
pub fn ErrorState(
    #[prop(into)] message: String,
    /// Where "Try again" should point. Defaults to reloading the gallery.
    #[prop(optional, into)]
    retry_href: Option<String>,
) -> impl IntoView {
    let href = retry_href.unwrap_or_else(|| "/".to_string());
    view! {
        <section class="flex min-h-[46vh] animate-rise-in flex-col items-center justify-center gap-3 text-center">
            <span class="inline-flex h-13 w-13 items-center justify-center rounded-lg border border-accent-line bg-accent-veil p-3 text-accent-muted">
                <Ico icon=LuCircleAlert size=28/>
            </span>
            <p class="m-0 text-base font-semibold text-ink">{message}</p>
            <A href=href attr:class="btn">
                <Ico icon=LuRefreshCw size=15/>
                <span>"Try again"</span>
            </A>
        </section>
    }
}
