use crate::app::SitePreview;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};

use crate::api::{item_of_the_day, list_items};
use crate::components::card::ItemCard;
use crate::components::density::{Density, DensityToggle, GridSkeleton};
use crate::components::empty::EmptyState;
use crate::components::icon::LuImage;
use crate::components::infinite_scroll::ScrollSentinel;
use crate::components::more_items::MoreItems;
use crate::components::sort_chips::{
    use_sort, view_from_param, view_param, GalleryView, SortCtx, SortMenu,
};
use crate::models::{Item, Sort};
use crate::seo::absolute;
use leptos_router::hooks::use_query_map;

/// How many tiles load eagerly at full priority.
///
/// Eight, because that is the widest first row any density produces: compact
/// tops out at `grid-cols-8` above 1900px. Anything past the first row is below
/// the fold at every breakpoint and stays lazy -- the point is to stop the LCP
/// tile queueing behind lazy siblings, not to defeat lazy loading.
const EAGER_TILES: usize = 8;

/// The gallery.
///
/// The first page comes from a blocking `Resource` so it renders during SSR — a
/// meme site lives on shared links, and a client-only grid would hand crawlers
/// an empty page. Later pages accumulate in a plain signal and append
/// client-side.
#[component]
pub fn Gallery() -> impl IntoView {
    // The active view comes from app-level context; the tab strip owns writing
    // it, this component only reads.
    let SortCtx { view, set_view } = use_sort();

    // `?view=clips` is a real URL: the sitemap lists it, llms.txt links it, and
    // a shared "look at the clips" link has to open on the clips. Read on the
    // server and the client alike, from the same query map, so both renders
    // agree -- this is a URL, not a viewport or a stored preference, which is
    // what makes it safe to seed from before hydration. The write-back below is
    // client-only and keeps the address bar honest after a menu change.
    let query = use_query_map();
    let from_url = move || query.read().get("view").map(|v| view_from_param(&v));
    if let Some(v) = from_url() {
        if view.get_untracked() != v {
            set_view.set(v);
        }
    }
    Effect::new(move |prev: Option<GalleryView>| {
        let current = view.get();
        if prev.is_some_and(|p| p != current) {
            let target = if current == GalleryView::default() {
                "/".to_string()
            } else {
                format!("/?view={}", view_param(current))
            };
            let navigate = leptos_router::hooks::use_navigate();
            navigate(
                &target,
                leptos_router::NavigateOptions {
                    replace: true,
                    scroll: false,
                    ..Default::default()
                },
            );
        }
        current
    });

    let sort = Signal::derive(move || view.get().sort());
    let item_of_day_view = Signal::derive(move || view.get() == GalleryView::ItemOfDay);

    // Keyed on the ordering, so switching tabs refetches page one from the
    // server rather than re-sorting a partial list in the browser. Not keyed on
    // the view itself: New -> Item of the day -> New would otherwise throw away
    // and re-fetch an identical page.
    let first = Resource::new_blocking(
        move || sort.get(),
        |s| async move { list_items(None, Some(s.as_str().to_string())).await },
    );

    // Pages 2..n, appended below whatever SSR delivered.
    let (extra, set_extra) = signal(Vec::<Item>::new());
    let (cursor, set_cursor) = signal(Option::<String>::None);
    let (exhausted, set_exhausted) = signal(false);
    // Tracked separately from `exhausted`: an empty gallery is technically
    // exhausted, but rendering "That's every item. For now." directly under
    // "No items yet." is self-contradictory, and it was the first thing a
    // visitor saw on the freshly-launched site.
    let (is_empty, set_is_empty) = signal(false);

    // Seeds cursor/exhausted once the first page resolves. An Effect, not a
    // side effect inside the view-producing closure below: a signal write
    // during render can leave the server's rendered HTML and the client's
    // first hydration pass disagreeing about which branch of the `<Show>`
    // near the bottom is active, since the server has no equivalent
    // "post-render" phase to run it in and so never sees it at all. Found
    // via a real crash -- any gallery with fewer than `PAGE_SIZE` items
    // (every brand-new deployment, on day one) hit a hydration panic that
    // took down the whole wasm module. Effects are already client-only by
    // design, so this can't happen here.
    Effect::new(move |_| {
        if let Some(Ok(page)) = first.get() {
            set_is_empty.set(page.items.is_empty());
            if cursor.get_untracked().is_none() && !exhausted.get_untracked() {
                match page.next_cursor {
                    Some(c) => set_cursor.set(Some(c)),
                    None => set_exhausted.set(true),
                }
            }
        }
    });

    let load_more = Action::new(move |_: &()| {
        let from = cursor.get_untracked();
        let s = sort.get_untracked();
        async move {
            // A None cursor means "nothing more to continue from" -- either
            // page one has not resolved yet, or the gallery is exhausted.
            // Fetching with None re-requests page ONE and appends it, which
            // duplicated every card: changing sort resets `exhausted`, the
            // scroll sentinel re-fires before the new first page lands, and the
            // whole list arrived twice. Page one comes from the blocking
            // Resource and never from here. Guarded inside the future because
            // Action's closure must return one.
            let Some(from) = from else { return };
            match list_items(Some(from), Some(s.as_str().to_string())).await {
                Ok(page) => {
                    let end = page.next_cursor.is_none();
                    set_extra.update(|v| v.extend(page.items));
                    set_cursor.set(page.next_cursor);
                    set_exhausted.set(end);
                }
                Err(e) => leptos::logging::error!("could not load more items: {e}"),
            }
        }
    });

    let loading = load_more.pending();
    let (density, set_density) = signal(Density::default());

    // Switching sort invalidates everything accumulated under the old order.
    // An Effect keyed on `sort` rather than a handler on the control: the value
    // is app-level context that anything could write, and `SortMenu` owns only
    // the writing of it. Runs once on mount too, where resetting already-empty
    // accumulators is a no-op.
    Effect::new(move |prev: Option<Sort>| {
        let current = sort.get();
        if prev.is_some_and(|p| p != current) {
            set_extra.set(Vec::new());
            set_cursor.set(None);
            set_exhausted.set(false);
            set_is_empty.set(false);
        }
        current
    });

    view! {
        <Title text=move || {
            let f = crate::flavor::get();
            match view.get() {
                GalleryView::Sort(Sort::Clips) => {
                    format!("{} clips and GIFs — {}", f.noun_title(), f.name)
                }
                GalleryView::ItemOfDay => format!("{} of the day — {}", f.noun_title(), f.name),
                _ => format!("{} — {}", f.name, f.tagline),
            }
        }/>
        <SitePreview/>
        <Meta name="description" content=crate::flavor::get().description.clone()/>
        // The clips view is its own canonical page; every other view canonicals
        // to the root, since they are re-orderings of the same list. Read once,
        // untracked: `<Link>` is not reactive, and the value only matters in the
        // server render, where the view has already been seeded from the URL
        // above. A crawler never sees a client-side menu change.
        <Link rel="canonical" href=match view.get_untracked() {
            GalleryView::Sort(Sort::Clips) => absolute("/?view=clips"),
            _ => absolute("/"),
        }/>

        // No hero. The gallery is the product, so content starts at the top of
        // the page; the title/tagline/marketing copy the old layout opened with
        // is gone rather than reworded.
        // Sort and the view-mode toggle share one row: sort left, modes right.
        // The item-of-the-day banner that used to sit above this is an option in
        // the sort menu instead of a full-width card competing with the grid.
        //
        // One control, not a strip. The four views used to be four pills that
        // were on screen at all times, which at 320-360px meant a row that had
        // to scroll sideways directly under a 56px bar -- two bands of chrome
        // above the fold before a single item. Collapsed, the same four choices
        // cost one pill, so the row no longer needs to bleed to the viewport
        // edges to make a half-scrolled option look intentional.
        //
        // Nothing here may gain `overflow-x` or `overflow-hidden`: the sort
        // menu's panel is absolutely positioned out of this row, and a scroll
        // container would clip it. The density toggle brings its own `ml-auto`,
        // so it stays pinned right whatever the selected label's width.
        <div class="flex min-w-0 items-center gap-3 pb-3 min-[700px]:pb-4">
            <SortMenu/>
            <DensityToggle density=density set_density=set_density/>
        </div>

        <Show when=move || item_of_day_view.get() fallback=|| ()>
            <ItemOfTheDay/>
        </Show>

        <Show when=move || !item_of_day_view.get() fallback=|| ()>
        // Same constant as the eager first row below, rather than two
        // independent 8s that agree by luck: the placeholder and the row it is
        // standing in for are the same tiles. And the same density, so changing
        // sort in compact mode does not draw a cozy-shaped placeholder that
        // reflows the instant the cards land.
        //
        // `get_untracked`, not `get`. A `Suspense` fallback is a `ViewFn` that
        // Leptos calls to build a view, not a closure it re-runs inside a
        // tracking context, so a plain `get` here subscribes to nothing and
        // Leptos says so at runtime -- a console warning on every load of `/`
        // that the four gate commands cannot see, because reactivity is checked
        // when the wasm runs and not when it compiles. Untracked is also the
        // honest read: `GridSkeleton` takes a plain `Density`, not a signal, and
        // the fallback is on screen only until the resource resolves, so there
        // is no window in which reacting to a density change would redraw
        // anything. The real grid below reads `density` in a genuine `move ||`
        // and does react.
        <Suspense fallback=move || view! { <GridSkeleton count=EAGER_TILES density=density.get_untracked()/> }>
            {move || {
                first
                    .get()
                    .map(|res| match res {
                        Err(e) => {
                            view! { <p class="text-danger">{format!("Could not reach the {}: ", crate::flavor::get().nouns)} {e.to_string()}</p> }
                                .into_any()
                        }
                        Ok(page) => {
                            if page.items.is_empty() {
                                return view! {
                                    <EmptyState
                                        icon=LuImage
                                        message=if sort.get_untracked() == Sort::Clips {
                                            "No clips yet. Upload the first one.".to_string()
                                        } else {
                                            format!("No {} yet.", crate::flavor::get().nouns)
                                        }
                                        action_href="/upload"
                                        action_label="Upload"
                                    />
                                }
                                    .into_any();
                            }

                            view! {
                                <div class=move || density.get().grid_class()>
                                    // `.enumerate()` rather than <For>, purely
                                    // so the opening row can be told it is the
                                    // opening row. This first page is a
                                    // snapshot of a resolved Resource and the
                                    // whole branch re-renders when the sort
                                    // changes, so there is no keyed
                                    // reconciliation being given up here.
                                    //
                                    // `extra` keeps <For>: it grows a page at a
                                    // time and must not re-render what is
                                    // already on screen -- and everything in it
                                    // is below the fold by definition, so none
                                    // of it is ever priority.
                                    {page
                                        .items
                                        .into_iter()
                                        .enumerate()
                                        .map(|(i, item)| {
                                            view! { <ItemCard item=item priority=i < EAGER_TILES/> }
                                        })
                                        .collect_view()}
                                    <For each=move || extra.get() key=|s| s.id.clone() let:item>
                                        <ItemCard item=item/>
                                    </For>
                                </div>
                            }
                                .into_any()
                        }
                    })
            }}
        </Suspense>
        </Show>

        <div
            class="py-6 text-center"
            class:hidden=move || is_empty.get() || item_of_day_view.get()
        >
            <Show
                when=move || !exhausted.get()
                fallback=|| ()
            >
                // The sentinel fetches the next page as it comes into view.
                <ScrollSentinel on_visible=move || {
                    if !loading.get_untracked() {
                        load_more.dispatch(());
                    }
                }/>
                // Status, not a control. The "more items" button that used to sit
                // here fired the same action the sentinel had already fired by
                // the time anyone could scroll far enough to press it -- so it
                // was a button whose job was always finished before it was
                // reachable. What is actually worth showing is whether a fetch
                // is in flight.
                //
                // `aria-live` because the alternative to a button is that new
                // items appear silently, which a screen reader would otherwise
                // never mention.
                <p class="m-0 text-[0.8125rem] text-ink-3" aria-live="polite">
                    {move || if loading.get() { format!("loading more {}…", crate::flavor::get().nouns) } else { String::new() }}
                </p>
            </Show>
        </div>
    }
}

/// The most-liked item from the last 24h (or overall, on a quiet day) —
/// computed on read in `db::item_of_the_day`, not curated by anyone.
#[component]
fn ItemOfTheDay() -> impl IntoView {
    let featured = Resource::new(|| (), |_| item_of_the_day());

    view! {
        <Suspense fallback=|| view! { <GridSkeleton count=1/> }>
            {move || {
                match featured.get().and_then(Result::ok).flatten() {
                    // Its own tab now, so it gets the grid's card treatment
                    // rather than a full-width banner shouting above the
                    // gallery. One card, sized like the others.
                    Some(s) => {
                        let id = s.id.clone();
                        view! {
                            <div class="max-w-[320px]">
                                <ItemCard item=s/>
                            </div>
                            // Fills the rest of the page rather than leaving one
                            // card alone in the viewport.
                            <MoreItems exclude=id/>
                        }
                            .into_any()
                    }
                    // Nothing featured yet means an empty collection; the same
                    // empty state the gallery uses.
                    None => {
                        view! {
                            <EmptyState
                                icon=LuImage
                                message=format!("No {} of the day yet.", crate::flavor::get().noun)
                                action_href="/upload"
                                action_label="Contribute"
                            />
                        }
                            .into_any()
                    }
                }
            }}
        </Suspense>
    }
}
