use crate::app::SitePreview;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::hooks::use_query_map;

use crate::api::{encode_query, search_items};
use crate::components::card::ItemCard;
use crate::components::density::SearchSkeleton;
use crate::components::empty::{EmptyState, ErrorState};
use crate::components::icon::LuSearch;

/// `/search?q=...` — full-text results via `items_search` (see migration 0007).
/// Not paginated: a search box is a "find the one I mean" tool for a gallery
/// this size, not a second browsing surface, so `PAGE_SIZE` results is
/// already generous for that job.
#[component]
pub fn SearchPage() -> impl IntoView {
    let query = use_query_map();
    let q = move || query.read().get("q").unwrap_or_default();

    let results = Resource::new(q, |q| async move {
        if q.trim().is_empty() {
            Ok(vec![])
        } else {
            search_items(q).await
        }
    });

    view! {
        <Title text=move || {
            let q = q();
            let site = &crate::flavor::get().name;
            if q.is_empty() { format!("search — {site}") } else { format!("\"{q}\" — {site}") }
        }/>
        <SitePreview/>
        // Query-dependent results have no stable content worth a search
        // engine's crawl budget or a snippet -- `follow` still lets a
        // crawler reach every item a result links to.
        <Meta name="robots" content="noindex, follow"/>

        // The echoed query is the one word on this page that is the visitor's
        // own, so it is the one that gets the accent. A free yellow moment that
        // costs no extra copy, in the only place a query echo belongs.
        <h1 class="m-0 mb-4 text-[1.375rem] font-bold tracking-tight lg:mb-6 lg:text-[1.75rem]">
            {move || {
                let q = q();
                if q.is_empty() {
                    view! { "Search" }.into_any()
                } else {
                    view! {
                        "\u{201C}"<span class="text-accent">{q}</span>"\u{201D}"
                    }
                        .into_any()
                }
            }}
        </h1>

        // 8, not 4: a half-height placeholder grid reads as a page that has
        // finished loading and found four things, then jumps when it hasn't.
        <Suspense fallback=|| view! { <SearchSkeleton/> }>
            {move || {
                results
                    .get()
                    .map(|res| match res {
                        // Retries this search rather than sending the visitor to
                        // the gallery: ErrorState's default href is "/", so the
                        // button said "Try again" and did something else.
                        Err(_) => {
                            view! {
                                <ErrorState
                                    message="Couldn't run that search."
                                    retry_href=format!("/search?q={}", encode_query(&q()))
                                />
                            }
                                .into_any()
                        }
                        // "No results" is the wrong sentence for a page nobody
                        // has searched on yet -- /search with no query is
                        // reachable from the 404 page and from a bare bookmark,
                        // and it used to answer a question that had not been
                        // asked, with a "Clear search" link that cleared nothing.
                        Ok(_) if q().trim().is_empty() => {
                            view! {
                                <EmptyState
                                    icon=LuSearch
                                    message="Search by title or tag using the box above."
                                    action_href="/"
                                    action_label=format!("Browse every {}", crate::flavor::get().noun)
                                />
                            }
                                .into_any()
                        }
                        Ok(items) if items.is_empty() => {
                            view! {
                                // The action says where it goes, rather than
                                // "Clear search" -- it leaves this page for the
                                // gallery, which is not what clearing a box does.
                                <EmptyState
                                    icon=LuSearch
                                    message="Nothing matched that."
                                    action_href="/"
                                    action_label=format!("Browse every {}", crate::flavor::get().noun)
                                />
                            }
                                .into_any()
                        }
                        Ok(items) => {
                            view! {
                                // Opacity only, never a transform: animating
                                // the grid wrapper's transform promotes it to
                                // its own layer and can force paint of the very
                                // subtrees the tiles' `content-visibility:auto`
                                // is there to skip.
                                <div class="grid animate-fade-in grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-4">
                                    <For each=move || items.clone() key=|s| s.id.clone() let:item>
                                        <ItemCard item=item/>
                                    </For>
                                </div>
                            }
                                .into_any()
                        }
                    })
            }}
        </Suspense>
    }
}
