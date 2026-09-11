//! A near-invisible sentinel placed after the grid. Once it comes within
//! `PREFETCH_MARGIN` of the viewport, `on_visible` fires and the next page is
//! fetched. There is no longer a button beside it -- the sentinel is the whole
//! mechanism.
//!
//! The margin is the part that matters. With the observer's default options the
//! sentinel only counts as visible once it is genuinely on screen, so a visitor
//! scrolls to the bottom of the grid and *then* waits out a round trip, once per
//! page, forever. Firing a screenful early means the next page has usually
//! already landed by the time they reach it.

use leptos::prelude::*;

/// How far below the viewport the sentinel starts pulling the next page.
///
/// Roughly one tall phone screen. Enough for a fetch to finish at ordinary
/// scroll speed, and not so far ahead that someone who stops scrolling has
/// dragged down pages they will never look at -- which at 10k items is the
/// difference between a few hundred KB of thumbnails and a few MB.
///
/// Hydrate-only, like the observer it configures: the server never scrolls, and
/// an ungated const is dead code in the ssr build.
#[cfg(feature = "hydrate")]
const PREFETCH_MARGIN: &str = "800px 0px";

#[component]
pub fn ScrollSentinel(on_visible: impl Fn() + Clone + 'static) -> impl IntoView {
    let node: NodeRef<leptos::html::Div> = NodeRef::new();

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;

        Effect::new(move |_| {
            let Some(el) = node.get() else {
                return;
            };
            let cb = on_visible.clone();

            // Fires with every entry the observer is watching, which is
            // always exactly this one sentinel -- checking "did anything
            // intersect" is enough, no need to identify which entry.
            let closure =
                Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
                    let visible = entries.iter().any(|entry| {
                        entry
                            .dyn_into::<web_sys::IntersectionObserverEntry>()
                            .map(|e| e.is_intersecting())
                            .unwrap_or(false)
                    });
                    if visible {
                        cb();
                    }
                });

            let opts = web_sys::IntersectionObserverInit::new();
            opts.set_root_margin(PREFETCH_MARGIN);

            match web_sys::IntersectionObserver::new_with_options(
                closure.as_ref().unchecked_ref(),
                &opts,
            ) {
                Ok(observer) => {
                    observer.observe(&el);
                    // The browser's own reference to the observer keeps it
                    // (and this callback) alive for as long as the element
                    // stays observed, which is the page's lifetime here --
                    // `forget` is the standard wasm-bindgen idiom for a
                    // closure meant to outlive the scope that created it.
                    closure.forget();
                }
                Err(e) => {
                    leptos::logging::error!("IntersectionObserver unavailable: {e:?}");
                }
            }
        });
    }

    #[cfg(not(feature = "hydrate"))]
    let _ = on_visible;

    view! { <div class="scroll-sentinel" node_ref=node></div> }
}
