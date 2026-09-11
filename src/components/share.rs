//! Two controls for two different actions: send this item to a person, and put
//! this item on a page.
//!
//! The share control is one button, not two. On a phone `navigator.share`
//! opens the real share sheet (Messages, WhatsApp, AirDrop), which is what
//! someone actually wants when they find a item worth sending. On a desktop
//! browser that API is mostly absent, so the same button writes the URL to the
//! clipboard instead. Offering both side by side would put a dead button in
//! front of half the visitors, and its label and icon follow the capability
//! rather than the viewport because that is what the button will really do --
//! deciding by CSS breakpoint would promise a share sheet to a narrow desktop
//! window that has no such thing.
//!
//! `EmbedButton` is a second control on the same row and that is not a
//! contradiction of the above. It is not a capability-dependent duplicate of
//! sharing; it copies the `<iframe>` markup for `/embed/:slug`, which is a
//! different thing to hand someone. It is always live -- there is no browser
//! where it does nothing -- so the "never show a dead button" rule that keeps
//! share at one control is what allows this one to exist at all.

use leptos::prelude::*;

// Not re-exported through `components::icon` like every other glyph here.
// That module is owned elsewhere and adding `LuCodeXml` to its `pub use` list
// is a one-line change that has not landed; importing the icon set directly is
// the documented exception, not a new convention. Move this to the icon
// module's re-export list when that line lands.
use icondata_lu::LuCodeXml;

use crate::components::icon::{Ico, LuCheck, LuLink, LuShare2};
// Only the click handler touches these, and the click handler only exists in
// the wasm build -- there is no clipboard to write to on the server.
#[cfg(feature = "hydrate")]
use crate::seo::{embed_snippet, embed_url_from_page_url};

/// How long the confirmation stays up after a copy, in milliseconds. Long
/// enough to read, short enough that it doesn't look stuck.
///
/// hydrate-only, like the copy itself: the server build has no clipboard to
/// confirm a write to, and an unused const there is a clippy failure.
#[cfg(feature = "hydrate")]
const CONFIRM_MS: u64 = 1600;

/// Resolves a path against the origin actually being viewed.
///
/// `seo::absolute` only consults `SITE_ORIGIN` under the `ssr` feature, so in
/// the wasm build it hands back exactly what it was given -- and these
/// components run on the client. Left alone, the share button copied
/// "/item/<id>", a path rather than a link, which pastes into a chat as nothing
/// anyone can click. It matters more for the embed snippet: a relative
/// `<iframe src>` resolves against the *host* site, so an embed pasted
/// anywhere else would be dead on arrival.
///
/// Resolving against the live origin is also the right answer when browsing
/// over the tailnet.
#[cfg(feature = "hydrate")]
fn absolute_here(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    let origin = web_sys::window()
        .expect("a browser has a window")
        .location()
        .origin()
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    format!("{origin}{url}")
}

/// Whether this browser exposes a clipboard at all.
///
/// `navigator.clipboard` is undefined outside a secure context, so over plain
/// http on a LAN or tailnet address it simply is not there -- and `web-sys` types
/// the getter as infallible, so calling `write_text` on the undefined value throws
/// from inside the click handler. The button did nothing, said nothing, and
/// logged nothing: the exact dead end this sweep is for.
///
/// `127.0.0.1` and `localhost` count as secure, so local development never sees
/// the fallback -- which is why this had to be reasoned about rather than
/// observed.
#[cfg(feature = "hydrate")]
fn has_clipboard() -> bool {
    js_sys::Reflect::get(
        &web_sys::window()
            .expect("a browser has a window")
            .navigator(),
        &wasm_bindgen::JsValue::from_str("clipboard"),
    )
    .map(|v| !v.is_undefined() && !v.is_null())
    .unwrap_or(false)
}

/// Writes to the clipboard and confirms only once the write resolved.
///
/// Clipboard writes are async and can be refused outright (no permission, or a
/// non-secure origin). Claiming "Copied" over a failed write would leave
/// someone pasting whatever was there before, so the confirmation is driven by
/// the promise, not by the click.
#[cfg(feature = "hydrate")]
fn copy_text(text: String, set_copied: WriteSignal<bool>, set_failed: WriteSignal<bool>) {
    let promise = web_sys::window()
        .expect("a browser has a window")
        .navigator()
        .clipboard()
        .write_text(&text);
    leptos::task::spawn_local(async move {
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(_) => {
                set_copied.set(true);
                set_timeout(
                    move || set_copied.set(false),
                    std::time::Duration::from_millis(CONFIRM_MS),
                );
            }
            // A refusal (no permission, focus lost, a policy) used to be a log
            // line and nothing else, which from the outside is identical to the
            // click not registering. It now says so on the control, and clears
            // itself on the same timer as the confirmation so the button does not
            // stay stuck complaining.
            Err(_) => {
                leptos::logging::warn!("clipboard write refused");
                set_failed.set(true);
                set_timeout(
                    move || set_failed.set(false),
                    std::time::Duration::from_millis(CONFIRM_MS),
                );
            }
        }
    });
}

#[component]
pub fn ShareButton(
    /// Absolute page URL to share. Passed in rather than read from
    /// `window.location` so the server render and the client agree, and so it
    /// stays the canonical origin rather than whatever host is in the address
    /// bar (a tailnet IP, a preview domain).
    #[prop(into)]
    url: String,
    /// Used as the share sheet's title; ignored by the clipboard path.
    #[prop(into)]
    title: String,
) -> impl IntoView {
    let (copied, set_copied) = signal(false);
    let (failed, set_failed) = signal(false);

    // Capability, not viewport. Starts false so SSR and the first client render
    // produce identical markup -- flipping it during render is what crashes
    // hydration, so it moves to an effect that only ever runs on the client.
    let (can_share, set_can_share) = signal(false);
    // Assumed present until the client says otherwise, for the same reason: the
    // server cannot know, and rendering the disabled state first would flash a
    // dead-looking button on every load for the majority of visitors who do have
    // a clipboard.
    let (can_copy, set_can_copy) = signal(true);

    #[cfg(feature = "hydrate")]
    Effect::new(move |_| set_can_copy.set(has_clipboard()));

    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        let has = js_sys::Reflect::has(
            &web_sys::window().expect("a browser has a window"),
            &wasm_bindgen::JsValue::from_str("navigator"),
        )
        .unwrap_or(false)
            && js_sys::Reflect::has(
                &web_sys::window()
                    .expect("a browser has a window")
                    .navigator(),
                &wasm_bindgen::JsValue::from_str("share"),
            )
            .unwrap_or(false);
        set_can_share.set(has);
    });

    // Both setters are written only from the hydrate-only paths below. The
    // server renders the button inert -- correct, since there is nothing to copy
    // to until wasm is running -- so the server build sees them as unused.
    #[cfg(not(feature = "hydrate"))]
    let _ = (set_can_share, set_copied, set_failed, set_can_copy);

    let on_click = {
        let url = url.clone();
        let title = title.clone();
        move |_| {
            #[cfg(feature = "hydrate")]
            {
                let url = absolute_here(&url);

                if can_share.get_untracked() {
                    let data = web_sys::ShareData::new();
                    data.set_title(&title);
                    data.set_url(&url);
                    // The returned promise rejects when the sheet is dismissed,
                    // which is a normal user action rather than an error, so it
                    // is deliberately not surfaced or logged.
                    let _ = web_sys::window()
                        .expect("a browser has a window")
                        .navigator()
                        .share_with_data(&data);
                    return;
                }

                copy_text(url, set_copied, set_failed);
            }
            #[cfg(not(feature = "hydrate"))]
            {
                let _ = (&url, &title);
            }
        }
    };

    // Sharing needs no clipboard, so only the copy fallback is gated on one.
    let usable = move || can_share.get() || can_copy.get();

    let label = move || {
        if copied.get() {
            "Link copied"
        } else if failed.get() {
            "Couldn't copy"
        } else if can_share.get() {
            "Share"
        } else if can_copy.get() {
            "Copy link"
        } else {
            // Not a dead button: it says why, and a screen reader gets the same
            // words. This is what an insecure origin looks like -- copy the
            // address bar instead.
            "Copying needs a secure connection"
        }
    };

    view! {
        // aria-live so the confirmation is announced, not just seen: the only
        // feedback for a copy is the icon swap, which a screen reader user
        // would otherwise get nothing from.
        <button
            class="icon-btn"
            type="button"
            on:click=on_click
            disabled=move || !usable()
            aria-label=label
            title=label
            aria-live="polite"
        >
            {move || {
                if copied.get() {
                    view! { <span class="text-ok"><Ico icon=LuCheck/></span> }.into_any()
                } else if failed.get() {
                    view! { <span class="text-danger"><Ico icon=LuLink/></span> }.into_any()
                } else if can_share.get() {
                    view! { <Ico icon=LuShare2/> }.into_any()
                } else {
                    view! { <Ico icon=LuLink/> }.into_any()
                }
            }}
        </button>
        // Rendered here rather than added to the detail page's action row
        // directly, so the row gains the control with no edit to its markup.
        // Still `pub`, so it can be placed on its own elsewhere later.
        <EmbedButton url=url title=title/>
    }
}

/// Copies the `<iframe>` snippet for this item's `/embed/:slug` card.
///
/// Takes the page URL, not an embed URL: `seo::embed_url_from_page_url`
/// derives one from the other, which means no new prop on the detail page and
/// exactly one definition of what a item's embed URL is.
#[component]
pub fn EmbedButton(
    /// The item's page URL, the same value `ShareButton` gets.
    #[prop(into)]
    url: String,
    /// Becomes the iframe's `title`, which is the accessible name of the frame
    /// on whatever site the snippet is pasted into.
    #[prop(into)]
    title: String,
) -> impl IntoView {
    let (copied, set_copied) = signal(false);
    let (failed, set_failed) = signal(false);
    let (can_copy, set_can_copy) = signal(true);

    #[cfg(feature = "hydrate")]
    Effect::new(move |_| set_can_copy.set(has_clipboard()));

    // Written only from the hydrate-only path below; the server build would
    // otherwise fail clippy on an unused setter.
    #[cfg(not(feature = "hydrate"))]
    let _ = (set_copied, set_failed, set_can_copy);

    let on_click = move |_| {
        #[cfg(feature = "hydrate")]
        {
            // Absolute first, then rewritten: the snippet is going onto
            // someone else's page, where a relative src points at *their*
            // /embed/ and renders nothing.
            let page_url = absolute_here(&url);
            let Some(embed_url) = embed_url_from_page_url(&page_url) else {
                // Only reachable if this component is ever placed on a page
                // that is not a item's, which would be a wiring mistake rather
                // than something a visitor can cause.
                leptos::logging::warn!("embed: {page_url} is not a item page");
                return;
            };
            copy_text(embed_snippet(&embed_url, &title), set_copied, set_failed);
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (&url, &title);
        }
    };

    // Unlike sharing, this control has no second route: there is no share sheet
    // for an iframe snippet, so with no clipboard it has nothing it can do and
    // says that rather than pretending.
    let label = move || {
        if copied.get() {
            "Embed code copied"
        } else if failed.get() {
            "Couldn't copy"
        } else if can_copy.get() {
            "Copy embed code"
        } else {
            "Copying needs a secure connection"
        }
    };

    view! {
        // Same structure as the share button on purpose: it reuses the
        // `.icon-btn` primitive as-is and adds no utility that could fight it
        // for a property.
        <button
            class="icon-btn"
            type="button"
            on:click=on_click
            disabled=move || !can_copy.get()
            aria-label=label
            title=label
            aria-live="polite"
        >
            {move || {
                if copied.get() {
                    view! { <span class="text-ok"><Ico icon=LuCheck/></span> }.into_any()
                } else if failed.get() {
                    view! { <span class="text-danger"><Ico icon=LuCodeXml/></span> }.into_any()
                } else {
                    view! { <Ico icon=LuCodeXml/> }.into_any()
                }
            }}
        </button>
    }
}
