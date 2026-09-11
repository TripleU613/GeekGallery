//! The global top progress bar, and the counter that drives it.
//!
//! Every page on this site waits on D1 over HTTP. Measured against the dev
//! server, a round trip is 82ms / 90ms / 570ms depending on what the query
//! touches, so a navigation holds the *previous* view on screen for roughly
//! 100-600ms with nothing moving. That silence is the thing this fixes: a 2px
//! line at the top of the viewport that starts, creeps, and completes.
//!
//! Two pieces, deliberately separate:
//!
//! * [`Loading`] is a counter in context. Anything that wants the bar increments
//!   it and decrements when done. A counter rather than a bool so overlapping
//!   loads compose -- a route change that lands while an in-page fetch is still
//!   running must not switch the bar off underneath it.
//! * [`TopProgress`] is the one element that reads that counter. It is mounted
//!   once, in `app.rs`, above the header.
//!
//! Route changes are wired up for free: `<Router set_is_routing=...>` in
//! `app.rs` calls [`Loading::start`] / [`Loading::finish`] around client-side
//! navigation. In-page loads are opt-in, because a component that wants the bar
//! should say so and nothing else has to change:
//!
//! ```ignore
//! // in the component body -- resolving context here, not in the future
//! let loading = crate::components::progress::use_loading();
//!
//! let act = Action::new(move |_: &()| async move {
//!     // INSIDE the async block. Never in the body: taking a guard during
//!     // render would write a signal during render, which kills the wasm
//!     // module outright (see CLAUDE.md).
//!     let _g = loading.map(|l| l.guard());
//!     something().await
//! });
//! ```
//!
//! The guard is the whole reason `start`/`finish` are not the recommended API.
//! A navigation that is superseded mid-flight, or a future that is dropped at
//! the `.await`, never reaches its own `finish()` -- and one unbalanced
//! increment pins the bar at 90% forever with no path back down. `Drop` cannot
//! be skipped.
//!
//! Deliberately not adopted by like/report: a 100ms mutation with its own
//! in-place feedback does not deserve a page-level bar.

use std::time::Duration;

use leptos::prelude::*;

/// Where the bar starts on the rising edge. Non-zero so the first frame is
/// visibly *something* rather than a line of zero width appearing to hang.
const START: f64 = 0.08;

/// The bar approaches this and never passes it while work is outstanding.
/// Only completion reaches 1.0, so the bar can never claim a load finished
/// that has not -- the failure mode of every naive timer-driven progress bar.
const CEILING: f64 = 0.90;

/// Fraction of the remaining distance covered per tick. With `TICK_MS` this
/// reaches ~0.5 in 400ms and ~0.8 in 900ms: fast enough to read as motion
/// during the common 100-600ms wait, slow enough that a genuinely slow query
/// still has somewhere to go.
const EASE: f64 = 0.18;

/// Tick interval. 100ms is under the ~150ms at which stepped motion starts
/// reading as stutter, and costs one f64 add per tick.
const TICK_MS: u64 = 100;

/// How long a load must be outstanding before the bar is made visible.
///
/// This is what stops a warm 60ms navigation from flashing a full start-to-
/// finish animation at someone who never perceived a wait in the first place.
const SHOW_DELAY_MS: u64 = 120;

/// How long the completed (full-width) bar stays visible before fading.
const HOLD_MS: u64 = 220;

/// When the bar is snapped back to zero width, after the fade has finished.
/// Later than `HOLD_MS` on purpose: reset it while still visible and the bar
/// visibly runs backwards.
const RESET_MS: u64 = 480;

/// The count of loads currently in flight, shared through context.
///
/// `Copy`, so it can be moved into as many closures as needed without cloning
/// and without a `move` capture fight.
#[derive(Copy, Clone)]
pub struct Loading {
    inflight: RwSignal<usize>,
}

impl Loading {
    /// Create the counter and put it in context. Called once, from `App`.
    ///
    /// Safe during render: this *creates* a signal and provides context, which
    /// is what component bodies are for. It is writing an already-existing
    /// signal during render that takes the wasm module down.
    pub fn provide() -> Self {
        let me = Self {
            inflight: RwSignal::new(0),
        };
        provide_context(me);
        me
    }

    /// Register one outstanding load. Prefer [`Loading::guard`].
    pub fn start(self) {
        // `try_update`, not `update`: `finish` can run from a `Drop` that
        // fires after the owning reactive scope has been disposed (a cancelled
        // navigation), where `update` panics on a dead signal. `start` uses the
        // same call for symmetry -- there is no version of "the bar failed to
        // update" worth taking a page down for.
        let _ = self.inflight.try_update(|n| *n += 1);
    }

    /// Retire one outstanding load. Prefer [`Loading::guard`].
    pub fn finish(self) {
        // Saturating, so a stray extra `finish` cannot wrap a `usize` to
        // 18 quintillion and wedge the bar on permanently.
        let _ = self.inflight.try_update(|n| *n = n.saturating_sub(1));
    }

    /// Is anything loading? A tracked read -- this is what the bar subscribes
    /// to.
    pub fn active(self) -> bool {
        self.inflight.get() > 0
    }

    /// Start a load whose end is whenever the returned guard is dropped.
    pub fn guard(self) -> LoadingGuard {
        self.start();
        LoadingGuard { loading: self }
    }
}

/// RAII counterpart to [`Loading::start`]. Not `Clone`: one guard is exactly
/// one outstanding load.
pub struct LoadingGuard {
    loading: Loading,
}

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        self.loading.finish();
    }
}

/// The counter, if an ancestor provided one. `Option` rather than a panicking
/// `expect` so a component carrying a guard can still be rendered in isolation
/// (tests, a future embed) without an `App` above it.
pub fn use_loading() -> Option<Loading> {
    use_context::<Loading>()
}

/// The bar itself. Mounted once, at the top of the app shell.
#[component]
pub fn TopProgress() -> impl IntoView {
    // Resolved here, in the component body, and captured. Calling `use_context`
    // inside the derived closure would look up context on whatever owner
    // happened to be current when the closure ran, which is not this one.
    let loading = use_loading();
    let active = Signal::derive(move || loading.map(|l| l.active()).unwrap_or(false));

    // 0.0..=1.0. Literal constants, both of them: the server renders
    // scaleX(0.0000) with opacity 0 and the client's first render must compute
    // exactly the same string. Seeding either from anything client-only (a
    // media query, a clock, readyState) is a hydration mismatch.
    let value = RwSignal::new(0.0_f64);
    let shown = RwSignal::new(false);

    // Not signals: nothing renders from them, and making them signals would
    // put them in the effect's dependency set.
    let ticker: StoredValue<Option<IntervalHandle>> = StoredValue::new(None);
    // Bumped on every edge. Every deferred callback below captures the epoch it
    // was scheduled under and does nothing if it no longer matches, so a
    // navigation that begins during the previous one's fade cannot be hidden
    // by that fade's timer.
    let epoch: StoredValue<u64> = StoredValue::new(0);

    Effect::new(move |prev: Option<()>| {
        // THE ONLY TRACKED READ IN THIS CLOSURE. Everything below writes, or
        // reads untracked. Read `shown` or `value` with `.get()` here and the
        // effect subscribes to signals it writes, re-runs itself forever, and
        // pins a core with the tab frozen.
        let is_active = active.get();

        if is_active {
            let mine = epoch.get_value().wrapping_add(1);
            epoch.set_value(mine);
            value.set(START);

            // Delayed reveal. Re-checks `active` as well as the epoch, because
            // a load can finish inside the delay window, in which case the
            // right outcome is that the bar was never shown at all.
            set_timeout(
                move || {
                    if epoch.get_value() == mine && active.get_untracked() {
                        shown.set(true);
                    }
                },
                Duration::from_millis(SHOW_DELAY_MS),
            );

            // Asymptotic creep toward CEILING. Guarded on the existing handle
            // so a second overlapping load does not start a second interval
            // that advances the same value twice as fast.
            if ticker.get_value().is_none() {
                match set_interval_with_handle(
                    move || value.update(|v| *v += (CEILING - *v) * EASE),
                    Duration::from_millis(TICK_MS),
                ) {
                    Ok(handle) => ticker.set_value(Some(handle)),
                    // No window, so no bar. The page still works.
                    Err(e) => leptos::logging::error!("progress ticker unavailable: {e:?}"),
                }
            }
        } else {
            if let Some(handle) = ticker.try_update_value(Option::take).flatten() {
                handle.clear();
            }

            // First run after mount: nothing was ever loading, so there is no
            // falling edge to play out. Without this, hydration fires a
            // pointless completion sequence on every page load.
            if prev.is_none() {
                return;
            }

            let mine = epoch.get_value().wrapping_add(1);
            epoch.set_value(mine);

            if shown.get_untracked() {
                value.set(1.0);
                set_timeout(
                    move || {
                        if epoch.get_value() == mine {
                            shown.set(false);
                        }
                    },
                    Duration::from_millis(HOLD_MS),
                );
                set_timeout(
                    move || {
                        if epoch.get_value() == mine {
                            value.set(0.0);
                        }
                    },
                    Duration::from_millis(RESET_MS),
                );
            } else {
                // Finished inside the reveal delay. Nothing was drawn, so
                // there is nothing to animate away.
                value.set(0.0);
            }
        }
    });

    view! {
        // `pointer-events-none` is load-bearing rather than decorative: at
        // opacity 0 this element still hit-tests, and it sits over the top 2px
        // of the fixed header at every viewport width. Without it, clicks land
        // on an invisible bar and nobody connects the bug to a progress
        // indicator.
        //
        // `z-50` clears the header (30) and the account menu's click-away
        // backdrop (40). It ties with the account panel itself, which is fine:
        // that panel hangs below the header and the two never overlap.
        //
        // The only motion here is the CSS transition, and it carries no
        // keyframes on purpose. `style/tailwind.css` ends with a global
        // reduced-motion block that forces every transition-duration to
        // 0.01ms, which turns this into a stepped indicator that still conveys
        // progress -- a keyframed alternative would keep running through that
        // same block and is the one thing it cannot quiet.
        //
        // One `style` closure rather than two `style:` bindings: nothing else
        // writes this element's style, and a single string cannot lose an
        // ordering fight with itself.
        <div
            aria-hidden="true"
            class="pointer-events-none fixed inset-x-0 top-0 z-50 h-[2px] origin-left bg-accent transition-[transform,opacity] duration-200 ease-out"
            style=move || {
                format!(
                    "transform:scaleX({:.4});opacity:{}",
                    value.get(),
                    if shown.get() { 1 } else { 0 },
                )
            }
        />
    }
}
