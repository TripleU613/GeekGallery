//! Grid density presets, and the site's skeleton-loader family.
//!
//! Density is not persisted across visits (an in-memory signal, reset to `Cozy`
//! on every page load) -- keeping it simple avoids the hydration-mismatch
//! complexity of reading `localStorage` before first paint, and a density
//! toggle is a light, low-stakes preference that isn't worth that cost.
//!
//! The skeletons live in this file on purpose, and it is not an accident of
//! history. A skeleton's whole job is to occupy the exact shape the real
//! content will, and for the grid that shape *is* `Density::grid_class` --
//! column counts at five breakpoints, including the compact preset's
//! `min-[1900px]` step. Put the placeholder grid in another module and the two
//! class strings drift apart silently, and the drift only shows up as a layout
//! jump on the one viewport nobody tested. Keeping them adjacent is the cheap
//! version of keeping them in sync.
//!
//! Everything here is `pub` in a `pub mod`, so the components other pages have
//! not wired up yet do not trip clippy's dead-code lint under `-D warnings`.

use leptos::prelude::*;

use crate::components::icon::{Ico, LuGrid2x2, LuLayoutGrid};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Density {
    Compact,
    #[default]
    Cozy,
}

impl Density {
    pub fn grid_class(&self) -> &'static str {
        match self {
            Density::Compact => {
                "grid grid-cols-3 gap-2 sm:grid-cols-4 xl:grid-cols-5 min-[1900px]:grid-cols-8"
            }
            Density::Cozy => {
                "grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-4 min-[1600px]:grid-cols-5 min-[1900px]:grid-cols-6"
            }
        }
    }
}

/// Whole class strings per state, which is the rule everywhere in this codebase:
/// the Tailwind scanner reads these `.rs` files literally and never sees a class
/// assembled from fragments at runtime.
fn seg_class(active: bool) -> &'static str {
    if active {
        "inline-flex h-8 w-8 items-center justify-center rounded-full bg-surface-hover text-accent transition-colors"
    } else {
        "inline-flex h-8 w-8 items-center justify-center rounded-full text-ink-3 transition-colors hover:text-ink"
    }
}

/// The view-mode switch.
///
/// Was three `.icon-btn`s carrying `class:is-active`, which styled nothing:
/// `is-active` has a rule for `.chip` and no rule for `.icon-btn`, so the
/// selected mode looked exactly like the two unselected ones and the control
/// silently did nothing visible. It also had no `aria-pressed`, so the state was
/// unavailable to a screen reader as well as to the eye. Both are fixed here,
/// and the three loose buttons are now one segmented control in a single
/// enclosure so it reads as a switch rather than as scattered icons.
///
/// No masonry. Ragged columns reflow on every image load at 24 tiles a page;
/// the tile is a fixed square and the item's own shape is shown on its page.
#[component]
pub fn DensityToggle(
    density: ReadSignal<Density>,
    set_density: WriteSignal<Density>,
) -> impl IntoView {
    view! {
        <div
            class="ml-auto hidden flex-none items-center gap-0.5 rounded-full border border-line bg-surface p-0.5 min-[700px]:flex"
            role="group"
            aria-label="View mode"
        >
            <button
                class=move || seg_class(density.get() == Density::Compact)
                aria-pressed=move || (density.get() == Density::Compact).to_string()
                on:click=move |_| set_density.set(Density::Compact)
                title="Compact grid"
                aria-label="Compact grid"
            >
                <Ico icon=LuLayoutGrid size=16/>
            </button>
            <button
                class=move || seg_class(density.get() == Density::Cozy)
                aria-pressed=move || (density.get() == Density::Cozy).to_string()
                on:click=move |_| set_density.set(Density::Cozy)
                title="Grid"
                aria-label="Grid"
            >
                <Ico icon=LuGrid2x2 size=16/>
            </button>
        </div>
    }
}

/// Entrance delay for the i-th item in a staggered list.
///
/// Whole literal class strings out of a fixed table, for the same reason
/// everything else in this codebase is: the Tailwind scanner reads these `.rs`
/// files as raw text, so the moment one of these is assembled with `format!`
/// the rule is never generated, the stagger silently disappears, and nothing
/// anywhere reports an error.
///
/// Clamping past the end rather than wrapping is deliberate. With a modulo, a
/// 24-tile page would restart the cascade from 0ms a third of the way down and
/// again two thirds of the way down, so tiles lower on the page would animate
/// *before* tiles above them. Clamping just means everything past the eighth
/// item shares the last step, which reads as one wave that runs out of runway.
pub fn stagger(i: usize) -> &'static str {
    const STEPS: [&str; 8] = [
        "[animation-delay:0ms]",
        "[animation-delay:40ms]",
        "[animation-delay:80ms]",
        "[animation-delay:120ms]",
        "[animation-delay:160ms]",
        "[animation-delay:200ms]",
        "[animation-delay:240ms]",
        "[animation-delay:280ms]",
    ];
    STEPS[i.min(STEPS.len() - 1)]
}

/// A grid of shimmering placeholder tiles, matching the real grid's shape --
/// replaces a bare "loading…" line so the layout doesn't jump once real
/// cards arrive. Square, and with no caption bar, because that is what a card
/// is.
///
/// `density` takes the same preset the real grid is about to use, so the
/// placeholder has the same column count at every breakpoint. It used to
/// hardcode the cozy 2/3/4, which meant a compact-mode load drew four columns
/// and then reflowed to eight at `min-[1900px]` the instant the cards landed.
#[component]
pub fn GridSkeleton(
    #[prop(default = 8)] count: usize,
    #[prop(optional)] density: Option<Density>,
) -> impl IntoView {
    let grid = density.unwrap_or_default().grid_class();
    view! {
        <div class=grid aria-hidden="true">
            {(0..count)
                .map(|i| {
                    // The only interpolated fragment is a whole class string
                    // returned by `stagger`, which is the pattern like.rs
                    // already uses -- never a class built from pieces.
                    let cls = format!("skeleton aspect-square rounded-lg border border-line {}", stagger(i));
                    view! { <div class=cls></div> }
                })
                .collect_view()}
        </div>
    }
}

/// Placeholder rows for the leaderboard.
///
/// The shape mirrors the real row element for element -- rank, avatar, name,
/// count -- because the point of a skeleton is that nothing moves when the data
/// arrives. If leaderboard.rs's row changes, this changes with it.
#[component]
pub fn RowSkeleton(#[prop(default = 6)] count: usize) -> impl IntoView {
    view! {
        <ol class="m-0 grid list-none gap-2 p-0" aria-hidden="true">
            {(0..count)
                .map(|i| {
                    let cls = format!(
                        "flex items-center gap-3 rounded border border-line bg-surface px-3.5 py-2.5 animate-rise-in {}",
                        stagger(i),
                    );
                    view! {
                        <li class=cls>
                            <span class="skeleton h-3 w-5 flex-none rounded"></span>
                            <span class="skeleton h-8 w-8 flex-none rounded-full"></span>
                            <span class="skeleton h-3.5 min-w-0 flex-1 rounded"></span>
                            <span class="skeleton h-3 w-14 flex-none rounded"></span>
                        </li>
                    }
                })
                .collect_view()}
        </ol>
    }
}

/// Placeholder for a single item's page.
///
/// The outer class string is detail.rs's article wrapper verbatim, so the
/// figure and the meta column start in their final positions and the page does
/// not reflow around the reader when the item resolves. The media block is a
/// square placeholder -- the item's real shape is unknown until it loads -- and
/// it carries the same `68vh` cap the real figure does.
///
/// That cap is `w-full max-w-[68vh]` rather than a single `min(100%, 68vh)`,
/// which would say the same thing in one value. `width: 100%` already cannot
/// exceed the container, so capping it again at `68vh` is exactly equivalent --
/// and it is equivalent in plain lengths. Release builds minify through
/// lightningcss against `browserquery = "defaults"`, `min()` appears nowhere
/// else in this codebase, and there is no way to check what that pass does to
/// it from the dev server, which does not minify at all. Two proven properties
/// beat one unverified function.
#[component]
pub fn DetailSkeleton() -> impl IntoView {
    view! {
        <article
            class="flex flex-col gap-4 pb-4 min-[860px]:flex-row min-[860px]:items-start min-[860px]:justify-center min-[860px]:gap-8 min-[860px]:pb-6"
            aria-hidden="true"
        >
            <div class="skeleton mx-auto aspect-square w-full max-w-[68vh] rounded-lg border border-line min-[860px]:mx-0"></div>
            <div class="min-w-0 min-[860px]:w-[320px] min-[860px]:flex-none">
                <div class="skeleton h-7 w-3/4 rounded"></div>
                <div class="skeleton mt-3 h-3.5 w-1/2 rounded"></div>
                // Four blocks for the four action controls, so the row that
                // holds them reserves its height rather than appearing later.
                <div class="mt-3 flex items-center gap-2 border-t border-line pt-3">
                    <div class="skeleton h-9 w-9 rounded"></div>
                    <div class="skeleton h-9 w-9 rounded"></div>
                    <div class="skeleton h-9 w-9 rounded"></div>
                    <div class="skeleton h-9 w-9 rounded"></div>
                </div>
            </div>
        </article>
    }
}

/// Placeholder for a search results page: the result-summary line, then a grid.
#[component]
pub fn SearchSkeleton(#[prop(default = 8)] count: usize) -> impl IntoView {
    view! {
        <div aria-hidden="true">
            <div class="skeleton mb-4 h-3 w-24 rounded"></div>
            <GridSkeleton count=count/>
        </div>
    }
}

/// Generic placeholder for a block of text -- the one to reach for on any panel
/// that is not a grid, a row list or a item.
///
/// Ragged widths from a fixed literal table, clamped exactly like `stagger`:
/// equal-width bars read as a form, not as prose.
#[component]
pub fn LineSkeleton(#[prop(default = 3)] lines: usize) -> impl IntoView {
    const WIDTHS: [&str; 4] = ["w-full", "w-11/12", "w-4/5", "w-2/3"];
    view! {
        <div class="grid gap-2" aria-hidden="true">
            {(0..lines)
                .map(|i| {
                    let cls = format!("skeleton h-3.5 rounded {}", WIDTHS[i.min(WIDTHS.len() - 1)]);
                    view! { <div class=cls></div> }
                })
                .collect_view()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::stagger;

    /// The clamp is the whole point, so it gets a test. With a modulo, item 8
    /// would go back to 0ms while item 7 sat at 280ms, and a tile lower on the
    /// page would animate *before* the one above it -- a cascade that visibly
    /// runs backwards a third of the way down a 24-tile grid.
    #[test]
    fn stagger_clamps_instead_of_wrapping() {
        let last = stagger(7);
        assert_eq!(last, "[animation-delay:280ms]");
        for i in [8, 9, 23, 100, usize::MAX] {
            assert_eq!(stagger(i), last, "index {i} should hold at the last step");
        }
    }

    /// Every step must be distinct up to the clamp, or the "stagger" is really
    /// two or three items firing together and the effect is lost.
    #[test]
    fn every_step_before_the_clamp_is_distinct() {
        let steps: Vec<&str> = (0..8).map(stagger).collect();
        let mut sorted = steps.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), steps.len(), "duplicate delay in {steps:?}");
    }

    /// The scanner reads these files as raw text, so a delay class is only real
    /// if it is spelled out in full. Anything built with `format!` would still
    /// pass the two tests above and generate no CSS at all, so this asserts the
    /// shape a Tailwind arbitrary value has to have.
    #[test]
    fn every_step_is_a_whole_arbitrary_value_class() {
        for i in 0..8 {
            let s = stagger(i);
            assert!(
                s.starts_with("[animation-delay:") && s.ends_with("ms]"),
                "{s:?} is not a whole arbitrary-value class"
            );
        }
    }
}
