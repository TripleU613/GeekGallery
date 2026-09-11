//! A grid of further items, used to fill the space below a single-item view (the
//! detail page, and the item-of-the-day tab).
//!
//! The same grid the gallery uses, one page of it, newest first.
//!
//! The section is deliberately unlabelled. Its visible heading was a second
//! page title competing with the item's own `<h1>` for no information: "More
//! items" says nothing the grid of items underneath it does not already say. Its
//! accessible name now lives in the `<section>`'s `aria-label`, which is also
//! what promotes it to a named landmark — so do not re-add a heading to fix
//! "this section has no title", and do not drop the `aria-label` either, or
//! this becomes a nameless run of two dozen links with no announced boundary.
//! The top rule is the only remaining signal that a new section starts, so it
//! stays too.
//!
//! Two call sites, so all of that lands in both places: the detail page, and
//! the gallery's item-of-the-day tab (`gallery.rs`, `ItemOfTheDay`).

use leptos::prelude::*;

use crate::api::list_items;
use crate::components::card::ItemCard;

#[component]
pub fn MoreItems(
    /// Omit this item from the results -- it is the one already being shown.
    #[prop(optional, into)]
    exclude: Option<String>,
) -> impl IntoView {
    // Newest, one page. No cursor state: this is a "keep browsing" surface, not
    // a second paginated gallery, so it deliberately stops after one page.
    let more = Resource::new(
        || (),
        |_| async move { list_items(None, Some("newest".to_string())).await },
    );

    view! {
        <Suspense fallback=|| ()>
            {move || {
                let exclude = exclude.clone();
                more.get()
                    .and_then(Result::ok)
                    .map(|page| {
                        let items: Vec<_> = page
                            .items
                            .into_iter()
                            .filter(|s| Some(&s.id) != exclude.as_ref())
                            .collect();
                        (!items.is_empty())
                            .then(|| {
                                view! {
                                    // pt raised from 5 to 6 with the heading
                                    // gone: the rule kept 32px of air above it
                                    // and, once the label stopped occupying the
                                    // 33px below it, only 20px underneath, so it
                                    // read as belonging to the section it
                                    // separates from rather than the one it
                                    // introduces.
                                    <section
                                        class="mt-4 border-t border-line pt-6 lg:mt-6"
                                        aria-label=format!("More {}", crate::flavor::get().nouns)
                                    >
                                        <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
                                            <For
                                                each=move || items.clone()
                                                key=|s| s.id.clone()
                                                let:item
                                            >
                                                <ItemCard item=item/>
                                            </For>
                                        </div>
                                    </section>
                                }
                            })
                    })
            }}
        </Suspense>
    }
}
