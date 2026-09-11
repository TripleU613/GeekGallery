//! The one way to link into the sign-in flow.
//!
//! This exists because the anchor has a non-obvious requirement and four places
//! needed it. `leptos_router` intercepts every same-origin `<a>` click and routes
//! it client-side unless the anchor carries `download` or `rel="external"` (see
//! `leptos_router`'s `location/mod.rs`). `/auth/google/login` is an Axum route
//! with no Leptos route behind it, so an intercepted click matches nothing and
//! renders the 404 page: the endpoint is fine, the button looks broken.
//!
//! Two of the four sign-in links had the attribute and two did not, which is the
//! shape of a bug that comes back. There is now one anchor, and `rel` is not a
//! prop -- a caller cannot get it wrong or leave it off.
//!
//! `return_to` is not a prop either. Every sign-in link on the site means "come
//! back to where I am", so the component reads the current path itself rather
//! than having four callers pass the same value from the same source.

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::api::sign_in_href;

#[component]
pub fn SignInLink(
    /// Accessible name and tooltip, for links whose visible content is an icon.
    /// Left unset where the link's own text already says it -- an `aria-label`
    /// replaces the visible text for a screen reader, so setting one that
    /// disagrees with it makes the link read differently than it looks.
    #[prop(optional, into)]
    label: Option<String>,
    /// Styling comes in as `attr:class` from the call site, so this component
    /// carries no opinion about it: it is an icon button in the header, a text
    /// link under the report form, and a pill shaped exactly like the like
    /// button it replaces.
    children: ChildrenFn,
) -> impl IntoView {
    let pathname = use_location().pathname;

    view! {
        <a rel="external" href=move || sign_in_href(&pathname.get()) aria-label=label.clone() title=label>
            {children()}
        </a>
    }
}
