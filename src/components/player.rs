//! The clip player: a `<video>` with its own controls, drawn by this site.
//!
//! The native player was the wrong tool three times over. Its controls are a
//! different design on every browser and none of them match the page; on a
//! phone the only gestures it offers are the buttons, when the ones people
//! actually reach for are tap-to-pause and double-tap-to-skip; and its
//! autoplay is a lottery -- an element built client-side with a `muted`
//! *attribute* does not get a muted *property*, so the browser's autoplay
//! policy refused it and every clip reached by swipe or by a click in the
//! grid sat frozen on its poster. That is the "plays and instantly pauses"
//! report.
//!
//! So: the element itself is bare, everything visible is this component, and
//! playback is driven imperatively from an `Effect` after mount, where
//! `set_muted(true)` and `play()` are ordinary calls with ordinary results.
//!
//! What it does, in the order a visitor meets it:
//!
//! - Plays muted and looping the moment it lands, like a GIF, with a "tap for
//!   sound" pill until the first unmute.
//! - Tap anywhere on the picture to pause and resume. Double-tap the left or
//!   right half to skip ten seconds that way, with a flash saying so. A mouse
//!   click toggles at once; a finger waits a beat to see if a second tap is
//!   coming.
//! - A scrubber with the buffered range behind the played range, draggable
//!   with pointer capture, and time as `0:12 / 0:44`.
//! - Mute, speed (1x -> 1.25 -> 1.5 -> 2 -> 0.5), fullscreen.
//! - Keys: space or K play/pause, J/L and the arrows skip, M mutes, F goes
//!   fullscreen. `ItemDetail`'s own arrow keys stand down while a player is
//!   mounted (see `PlayerMounted`).
//! - Controls fade out after a couple of seconds of playing without a touch,
//!   and are always up while paused.
//!
//! Every signal here starts from a constant, never from the element: the
//! server prints the paused, muted, controls-up state and the client flips
//! to the truth in event handlers, which is the only arrangement hydration
//! survives (CLAUDE.md, "Never write a signal during render").
//!
//! The swipe between items lives on the `<figure>` around this. Pointer
//! events on the picture itself are allowed to bubble so a swipe across the
//! video still turns the page; the control bar and the scrubber stop them,
//! because a drag along the scrubber is a seek and nothing else.

use std::time::Duration;

use leptos::prelude::*;

use crate::components::card::short_duration;
use crate::components::icon::{
    Ico, LuLoaderCircle, LuMaximize, LuMinimize, LuPause, LuPlay, LuRotateCcw, LuRotateCw,
    LuVolume2, LuVolumeX,
};
use crate::models::Item;

/// True while a `Player` is mounted. Provided by `ItemDetail`, so its
/// arrow-key stepping can yield to the player's seeking.
#[derive(Clone, Copy)]
pub struct PlayerMounted(pub RwSignal<bool>);

/// A double-tap on either half skips this far.
const SKIP_SECONDS: f64 = 10.0;
/// The arrow keys nudge this far; J and L skip `SKIP_SECONDS`.
const NUDGE_SECONDS: f64 = 5.0;
/// How long the controls stay up after the last touch while playing.
const HIDE_AFTER_MS: u64 = 2400;
/// Two taps closer than this on the same half are a double-tap. Also how
/// long a single finger-tap waits before it is taken as one -- the price of
/// telling the two apart, and the same beat every phone video player pays.
const DOUBLE_TAP_MS: u64 = 280;
/// How long the skip flash is shown.
const FLASH_MS: u64 = 550;
/// A press that travels further than this is a swipe, not a tap.
const TAP_SLOP_PX: f64 = 12.0;
/// The speed button cycles through these.
const SPEEDS: [f64; 5] = [1.0, 1.25, 1.5, 2.0, 0.5];

fn speed_label(rate: f64) -> String {
    if rate == rate.trunc() {
        format!("{rate:.0}×")
    } else {
        format!("{rate}×")
    }
}

#[component]
pub fn Player(item: Item) -> impl IntoView {
    let video: NodeRef<leptos::html::Video> = NodeRef::new();
    let root: NodeRef<leptos::html::Div> = NodeRef::new();
    let bar: NodeRef<leptos::html::Div> = NodeRef::new();

    // Everything the controls draw from. Seeded from constants and the item
    // row, never from the element (see the module comment).
    let playing = RwSignal::new(false);
    let muted = RwSignal::new(true);
    let sound_hint = RwSignal::new(true);
    let time = RwSignal::new(0.0_f64);
    let duration = RwSignal::new(
        item.duration
            .filter(|d| d.is_finite() && *d > 0.0)
            .unwrap_or(0.0),
    );
    let buffered = RwSignal::new(0.0_f64);
    let speed_idx = RwSignal::new(0_usize);
    let shown = RwSignal::new(true);
    let scrubbing = RwSignal::new(false);
    let buffering = RwSignal::new(false);
    let fullscreen = RwSignal::new(false);
    // `Some(forward)` while a skip flash is on screen.
    let flash = RwSignal::new(Option::<bool>::None);

    // Bookkeeping that nothing renders from.
    let hide_timer: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let tap_timer: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let flash_epoch: StoredValue<u64> = StoredValue::new(0);
    // Last tap on the picture: when (ms) and which half.
    let last_tap: StoredValue<Option<(f64, bool)>> = StoredValue::new(None);
    // Where the current press on the picture began.
    let press_at: StoredValue<Option<(f64, f64)>> = StoredValue::new(None);

    let mounted = use_context::<PlayerMounted>();

    let poster = item.still_url().to_string();
    let src = item.orig_url.clone();
    let title = item.title.clone();

    // -- helpers -------------------------------------------------------------

    let cancel_hide = move || {
        if let Some(h) = hide_timer.get_value() {
            h.clear();
        }
        hide_timer.set_value(None);
    };
    // Show the controls, and if the clip is playing, arrange for them to go.
    let touch = move || {
        shown.set(true);
        cancel_hide();
        if playing.get_untracked() && !scrubbing.get_untracked() {
            let h = set_timeout_with_handle(
                move || {
                    if playing.get_untracked() && !scrubbing.get_untracked() {
                        shown.set(false);
                    }
                },
                Duration::from_millis(HIDE_AFTER_MS),
            )
            .ok();
            hide_timer.set_value(h);
        }
    };

    let toggle_play = move || {
        let Some(v) = video.get_untracked() else {
            return;
        };
        if v.paused() {
            dom::play(&v);
        } else {
            v.pause().ok();
        }
        touch();
    };

    let seek_to = move |t: f64| {
        let Some(v) = video.get_untracked() else {
            return;
        };
        // The element's own length when it knows it, the row's until then.
        // Stop a hair short of the end: with `loop` on, landing exactly on
        // it wraps to the start, and "skip ahead" turning into "start over"
        // is the wrong surprise.
        let d = match v.duration() {
            d if d.is_finite() && d > 0.0 => d,
            _ => duration.get_untracked(),
        };
        let t = if d > 0.0 {
            t.clamp(0.0, (d - 0.1).max(0.0))
        } else {
            t.max(0.0)
        };
        v.set_current_time(t);
        time.set(t);
        touch();
    };

    let skip = move |by: f64| {
        let now = video
            .get_untracked()
            .map(|v| v.current_time())
            .unwrap_or(0.0);
        seek_to(now + by);
        let mine = flash_epoch.get_value().wrapping_add(1);
        flash_epoch.set_value(mine);
        flash.set(Some(by > 0.0));
        let _ = set_timeout_with_handle(
            move || {
                if flash_epoch.get_value() == mine {
                    flash.set(None);
                }
            },
            Duration::from_millis(FLASH_MS),
        );
    };

    let toggle_mute = move || {
        let Some(v) = video.get_untracked() else {
            return;
        };
        let now_muted = !v.muted();
        v.set_muted(now_muted);
        muted.set(now_muted);
        if !now_muted {
            sound_hint.set(false);
        }
        touch();
    };

    let cycle_speed = move || {
        let next = (speed_idx.get_untracked() + 1) % SPEEDS.len();
        speed_idx.set(next);
        if let Some(v) = video.get_untracked() {
            v.set_playback_rate(SPEEDS[next]);
        }
        touch();
    };

    let toggle_fullscreen = move || {
        let (Some(r), Some(v)) = (root.get_untracked(), video.get_untracked()) else {
            return;
        };
        dom::toggle_fullscreen(&r, &v);
        touch();
    };

    // -- mount ---------------------------------------------------------------

    // Start playback the deterministic way. Autoplay muted is allowed
    // everywhere; `play()` may still be refused (data saver, a policy we
    // have not met) and then the big play glyph is simply showing the truth.
    Effect::new(move |_| {
        let Some(v) = video.get() else { return };
        v.set_muted(true);
        v.set_playback_rate(SPEEDS[speed_idx.get_untracked()]);
        // The element has been loading since the HTML arrived, well before
        // this wasm attached its listeners, so `loadedmetadata` (and on a
        // fast connection `progress`) may already be behind us. Read what it
        // knows now rather than wait for events that have fired.
        let d = v.duration();
        if d.is_finite() && d > 0.0 {
            duration.set(d);
        }
        buffered.set(dom::buffered_end(&v));
        time.set(v.current_time());
        dom::play(&v);
        if let Some(m) = mounted {
            m.0.set(true);
        }
    });
    on_cleanup(move || {
        cancel_hide();
        if let Some(h) = tap_timer.get_value() {
            h.clear();
        }
        if let Some(m) = mounted {
            m.0.set(false);
        }
    });

    // A hidden tab does not get to keep playing: a clip looping muted behind
    // another tab is battery and bandwidth for nobody. Paused on hide, resumed
    // on return only if it was this that paused it -- a clip the visitor
    // paused stays paused.
    let paused_by_tab: StoredValue<bool> = StoredValue::new(false);
    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::visibilitychange, move |_| {
            let Some(v) = video.get_untracked() else {
                return;
            };
            if document().hidden() {
                if !v.paused() {
                    paused_by_tab.set_value(true);
                    let _ = v.pause();
                }
            } else if paused_by_tab.get_value() {
                paused_by_tab.set_value(false);
                dom::play(&v);
            }
        });
        on_cleanup(move || handle.remove());
    });

    // Keys, for as long as the player is on the page.
    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::keydown, move |ev| {
            if ev.alt_key() || ev.ctrl_key() || ev.meta_key() {
                return;
            }
            if dom::target_is_interactive(&ev) {
                return;
            }
            let key = ev.key();
            let handled = match key.as_str() {
                " " | "k" | "K" => {
                    toggle_play();
                    true
                }
                "ArrowLeft" => {
                    skip(-NUDGE_SECONDS);
                    true
                }
                "ArrowRight" => {
                    skip(NUDGE_SECONDS);
                    true
                }
                "j" | "J" => {
                    skip(-SKIP_SECONDS);
                    true
                }
                "l" | "L" => {
                    skip(SKIP_SECONDS);
                    true
                }
                "m" | "M" => {
                    toggle_mute();
                    true
                }
                "f" | "F" => {
                    toggle_fullscreen();
                    true
                }
                _ => false,
            };
            if handled {
                ev.prevent_default();
            }
        });
        on_cleanup(move || handle.remove());
    });

    // -- element events ------------------------------------------------------

    let on_loadedmetadata = move |_| {
        if let Some(v) = video.get_untracked() {
            let d = v.duration();
            if d.is_finite() && d > 0.0 {
                duration.set(d);
            }
        }
    };
    let on_timeupdate = move |_| {
        if scrubbing.get_untracked() {
            return;
        }
        if let Some(v) = video.get_untracked() {
            time.set(v.current_time());
        }
    };
    let on_progress = move |_| {
        if let Some(v) = video.get_untracked() {
            buffered.set(dom::buffered_end(&v));
        }
    };
    let on_play = move |_| {
        playing.set(true);
        buffering.set(false);
        touch();
    };
    let on_pause = move |_| {
        playing.set(false);
        shown.set(true);
        cancel_hide();
    };
    let on_waiting = move |_| buffering.set(true);
    let on_playing = move |_| buffering.set(false);
    let on_volumechange = move |_| {
        if let Some(v) = video.get_untracked() {
            muted.set(v.muted());
        }
    };
    let on_fullscreenchange = move |_| {
        fullscreen.set(dom::is_fullscreen());
        touch();
    };

    // -- the picture: tap, double-tap ----------------------------------------

    let on_surface_down = move |ev: leptos::ev::PointerEvent| {
        press_at.set_value(Some((f64::from(ev.client_x()), f64::from(ev.client_y()))));
        shown.set(true);
        cancel_hide();
    };
    let on_surface_up = move |ev: leptos::ev::PointerEvent| {
        let Some((x0, y0)) = press_at.get_value() else {
            return;
        };
        press_at.set_value(None);
        let (x, y) = (f64::from(ev.client_x()), f64::from(ev.client_y()));
        if (x - x0).abs() > TAP_SLOP_PX || (y - y0).abs() > TAP_SLOP_PX {
            // A swipe; the figure around us is handling it.
            touch();
            return;
        }
        let right = dom::is_right_half(&ev, x);
        let now = dom::now_ms();
        let is_double = last_tap
            .get_value()
            .is_some_and(|(t, r)| r == right && now - t < DOUBLE_TAP_MS as f64);
        if is_double {
            last_tap.set_value(None);
            if let Some(h) = tap_timer.get_value() {
                // The first tap's pending toggle is cancelled: two taps are a
                // skip, not a pause and a resume.
                h.clear();
                tap_timer.set_value(None);
            } else {
                // A mouse toggled on the first click already; put it back.
                toggle_play();
            }
            skip(if right { SKIP_SECONDS } else { -SKIP_SECONDS });
            return;
        }
        last_tap.set_value(Some((now, right)));
        if ev.pointer_type() == "touch" {
            let h = set_timeout_with_handle(
                move || {
                    tap_timer.set_value(None);
                    toggle_play();
                },
                Duration::from_millis(DOUBLE_TAP_MS),
            )
            .ok();
            tap_timer.set_value(h);
        } else {
            toggle_play();
        }
    };
    let on_surface_move = move |ev: leptos::ev::PointerEvent| {
        // A mouse gliding over the picture is a reason to show the controls;
        // a finger dragging is a swipe and gets nothing extra.
        if ev.pointer_type() != "touch" {
            touch();
        }
    };

    // -- the scrubber ----------------------------------------------------------

    let scrub_to = move |ev: &leptos::ev::PointerEvent| {
        let Some(b) = bar.get_untracked() else { return };
        let frac = dom::fraction_along(&b, f64::from(ev.client_x()));
        seek_to(frac * duration.get_untracked());
    };
    let on_bar_down = move |ev: leptos::ev::PointerEvent| {
        ev.stop_propagation();
        ev.prevent_default();
        if let Some(b) = bar.get_untracked() {
            let _ = b.set_pointer_capture(ev.pointer_id());
        }
        scrubbing.set(true);
        scrub_to(&ev);
    };
    let on_bar_move = move |ev: leptos::ev::PointerEvent| {
        if !scrubbing.get_untracked() {
            return;
        }
        ev.stop_propagation();
        scrub_to(&ev);
    };
    let on_bar_up = move |ev: leptos::ev::PointerEvent| {
        if !scrubbing.get_untracked() {
            return;
        }
        ev.stop_propagation();
        scrub_to(&ev);
        scrubbing.set(false);
        touch();
    };
    // The control bar as a whole swallows pointerdown so a press on a button
    // never starts the figure's swipe.
    let on_controls_down = move |ev: leptos::ev::PointerEvent| ev.stop_propagation();

    // -- classes: whole strings per state (CLAUDE.md, Tailwind) ---------------

    let chrome_class = move || {
        if shown.get() || !playing.get() || scrubbing.get() {
            "absolute inset-x-0 bottom-0 z-20 flex flex-col gap-1 bg-gradient-to-t from-black/85 via-black/45 to-transparent px-2 pb-1.5 pt-8 transition-opacity duration-200 ease-out opacity-100"
        } else {
            "absolute inset-x-0 bottom-0 z-20 flex flex-col gap-1 bg-gradient-to-t from-black/85 via-black/45 to-transparent px-2 pb-1.5 pt-8 transition-opacity duration-300 ease-out opacity-0 pointer-events-none"
        }
    };
    let cursor_class = move || {
        if shown.get() || !playing.get() {
            "absolute inset-0 z-10 cursor-pointer"
        } else {
            "absolute inset-0 z-10 cursor-none"
        }
    };
    let center_class = move || {
        if !playing.get() && !buffering.get() {
            "pointer-events-none absolute left-1/2 top-1/2 z-10 flex h-[4.5rem] w-[4.5rem] -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full bg-black/60 text-white shadow-lg backdrop-blur-sm transition-[opacity,transform] duration-200 ease-out opacity-100 scale-100"
        } else {
            "pointer-events-none absolute left-1/2 top-1/2 z-10 flex h-[4.5rem] w-[4.5rem] -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full bg-black/60 text-white shadow-lg backdrop-blur-sm transition-[opacity,transform] duration-200 ease-out opacity-0 scale-75"
        }
    };
    let hint_class = move || {
        if muted.get() && sound_hint.get() && playing.get() {
            "absolute left-2 top-2 z-20 inline-flex min-h-9 items-center gap-1.5 rounded-full bg-black/65 px-3 text-[0.8125rem] font-semibold text-white backdrop-blur-sm transition-opacity duration-300 ease-out opacity-100"
        } else {
            "pointer-events-none absolute left-2 top-2 z-20 inline-flex min-h-9 items-center gap-1.5 rounded-full bg-black/65 px-3 text-[0.8125rem] font-semibold text-white backdrop-blur-sm transition-opacity duration-300 ease-out opacity-0"
        }
    };
    let flash_class = move |forward: bool| {
        let side = if forward { "right-[12%]" } else { "left-[12%]" };
        let on = flash.get() == Some(forward);
        format!(
            "pointer-events-none absolute top-1/2 z-10 flex -translate-y-1/2 flex-col items-center gap-1 rounded-full bg-black/55 px-4 py-3 text-white backdrop-blur-sm transition-[opacity,transform] duration-200 ease-out {side} {}",
            if on { "opacity-100 scale-100" } else { "opacity-0 scale-90" }
        )
    };
    let played_pct = move || {
        let d = duration.get();
        if d > 0.0 {
            format!("{:.3}%", (time.get() / d * 100.0).clamp(0.0, 100.0))
        } else {
            "0%".to_string()
        }
    };
    let buffered_pct = move || {
        let d = duration.get();
        if d > 0.0 {
            format!("{:.3}%", (buffered.get() / d * 100.0).clamp(0.0, 100.0))
        } else {
            "0%".to_string()
        }
    };
    let clock = move || {
        format!(
            "{} / {}",
            short_duration(time.get()),
            short_duration(duration.get())
        )
    };

    view! {
        // The root is what goes fullscreen, so the controls come along. In
        // fullscreen the UA pins it to the viewport; the caps that size it in
        // the figure are lifted so it can take the whole screen.
        <div
            node_ref=root
            class="group/player relative h-full w-full select-none overflow-hidden rounded bg-black [&:fullscreen]:rounded-none [&:fullscreen]:max-h-none"
            on:fullscreenchange=on_fullscreenchange
        >
            // Bare: no `controls`, no `autoplay`. `muted` and `loop` are set
            // as attributes too so the server HTML describes the same element
            // the client will drive, and `preload="metadata"` with the poster
            // means the page paints before a byte of video arrives.
            <video
                node_ref=video
                class="absolute inset-0 h-full w-full object-contain"
                src=src
                poster=poster
                muted
                loop
                playsinline
                preload="metadata"
                aria-label=title
                on:loadedmetadata=on_loadedmetadata
                on:timeupdate=on_timeupdate
                on:progress=on_progress
                on:play=on_play
                on:pause=on_pause
                on:waiting=on_waiting
                on:playing=on_playing
                on:volumechange=on_volumechange
            />

            // The tap surface. Under the controls, over the video. Its
            // pointer events bubble on purpose (see the module comment).
            <div
                class=cursor_class
                on:pointerdown=on_surface_down
                on:pointerup=on_surface_up
                on:pointermove=on_surface_move
                role="button"
                aria-label="Play or pause"
                tabindex="-1"
            />

            // Paused: a big play glyph. Buffering: a spinner in its place.
            <div class=center_class>
                <Ico icon=LuPlay size=34 class="translate-x-0.5"/>
            </div>
            {move || {
                buffering.get().then(|| view! {
                    <div class="pointer-events-none absolute left-1/2 top-1/2 z-10 -translate-x-1/2 -translate-y-1/2 text-white/90">
                        <Ico icon=LuLoaderCircle size=40 class="animate-spin"/>
                    </div>
                })
            }}

            // Skip flashes, one per half.
            <div class=move || flash_class(false)>
                <Ico icon=LuRotateCcw size=22/>
                <span class="text-[0.75rem] font-semibold tabular-nums">"10s"</span>
            </div>
            <div class=move || flash_class(true)>
                <Ico icon=LuRotateCw size=22/>
                <span class="text-[0.75rem] font-semibold tabular-nums">"10s"</span>
            </div>

            // "Tap for sound", until the first unmute.
            <button
                type="button"
                class=hint_class
                on:pointerdown=on_controls_down
                on:click=move |_| toggle_mute()
            >
                <Ico icon=LuVolumeX size=16/>
                "Tap for sound"
            </button>

            // The control bar.
            <div class=chrome_class on:pointerdown=on_controls_down>
                // Scrubber. The touch target is the whole 28px-tall strip; the
                // drawn track is the thin bar in the middle, thickening on
                // hover and while dragging.
                <div
                    node_ref=bar
                    class="group/bar relative flex h-7 w-full cursor-pointer items-center touch-none"
                    role="slider"
                    aria-label="Seek"
                    aria-valuemin="0"
                    aria-valuemax=move || format!("{:.0}", duration.get())
                    aria-valuenow=move || format!("{:.0}", time.get())
                    aria-valuetext=move || short_duration(time.get())
                    tabindex="0"
                    on:pointerdown=on_bar_down
                    on:pointermove=on_bar_move
                    on:pointerup=on_bar_up
                    on:pointercancel=on_bar_up
                >
                    <div class="relative h-1 w-full overflow-hidden rounded-full bg-white/25 transition-[height] duration-150 ease-out group-hover/bar:h-1.5">
                        <div class="absolute inset-y-0 left-0 rounded-full bg-white/35" style:width=buffered_pct/>
                        <div class="absolute inset-y-0 left-0 rounded-full bg-accent" style:width=played_pct/>
                    </div>
                    // The knob, sitting on the played edge.
                    <div
                        class="pointer-events-none absolute top-1/2 h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-accent shadow transition-transform duration-150 ease-out scale-0 group-hover/bar:scale-100 group-active/bar:scale-100"
                        style:left=played_pct
                    />
                </div>

                <div class="flex items-center gap-0.5 text-white">
                    <button
                        type="button"
                        class="inline-flex h-10 w-10 items-center justify-center rounded text-white transition-colors hover:bg-white/15"
                        aria-label=move || if playing.get() { "Pause" } else { "Play" }
                        on:click=move |_| toggle_play()
                    >
                        {move || {
                            if playing.get() {
                                view! { <Ico icon=LuPause size=22/> }.into_any()
                            } else {
                                view! { <Ico icon=LuPlay size=22/> }.into_any()
                            }
                        }}
                    </button>
                    <button
                        type="button"
                        class="inline-flex h-10 w-10 items-center justify-center rounded text-white transition-colors hover:bg-white/15"
                        aria-label=move || if muted.get() { "Unmute" } else { "Mute" }
                        on:click=move |_| toggle_mute()
                    >
                        {move || {
                            if muted.get() {
                                view! { <Ico icon=LuVolumeX size=20/> }.into_any()
                            } else {
                                view! { <Ico icon=LuVolume2 size=20/> }.into_any()
                            }
                        }}
                    </button>
                    <span class="px-1.5 text-[0.8125rem] font-medium tabular-nums text-white/90">
                        {clock}
                    </span>
                    <span class="flex-1"/>
                    <button
                        type="button"
                        class="inline-flex h-10 min-w-10 items-center justify-center rounded px-2 text-[0.8125rem] font-semibold tabular-nums text-white transition-colors hover:bg-white/15"
                        aria-label="Playback speed"
                        title="Playback speed"
                        on:click=move |_| cycle_speed()
                    >
                        {move || speed_label(SPEEDS[speed_idx.get()])}
                    </button>
                    <button
                        type="button"
                        class="inline-flex h-10 w-10 items-center justify-center rounded text-white transition-colors hover:bg-white/15"
                        aria-label=move || if fullscreen.get() { "Exit fullscreen" } else { "Fullscreen" }
                        on:click=move |_| toggle_fullscreen()
                    >
                        {move || {
                            if fullscreen.get() {
                                view! { <Ico icon=LuMinimize size=20/> }.into_any()
                            } else {
                                view! { <Ico icon=LuMaximize size=20/> }.into_any()
                            }
                        }}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// The handful of things that need a browser, behind one seam. The
/// server build gets the same names as no-ops so the component body is one
/// piece of code: handlers are compiled there too, they just never run.
#[cfg(feature = "hydrate")]
mod dom {
    use leptos::wasm_bindgen::JsCast;
    use leptos::web_sys;

    pub fn play(v: &web_sys::HtmlVideoElement) {
        // The promise's rejection is the autoplay policy saying no; the
        // `pause` state it leaves behind is already what the UI shows. It is
        // awaited only so the rejection is handled rather than logged as
        // unhandled on every clip a browser declines.
        if let Ok(p) = v.play() {
            leptos::task::spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(p).await;
            });
        }
    }

    pub fn buffered_end(v: &web_sys::HtmlVideoElement) -> f64 {
        let ranges = v.buffered();
        let n = ranges.length();
        if n == 0 {
            return 0.0;
        }
        // The range the playhead is in, else the last one.
        let t = v.current_time();
        for i in 0..n {
            let (s, e) = (ranges.start(i).unwrap_or(0.0), ranges.end(i).unwrap_or(0.0));
            if s <= t && t <= e {
                return e;
            }
        }
        ranges.end(n - 1).unwrap_or(0.0)
    }

    pub fn now_ms() -> f64 {
        web_sys::js_sys::Date::now()
    }

    pub fn is_right_half(ev: &web_sys::PointerEvent, client_x: f64) -> bool {
        let Some(el) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return false;
        };
        let r = el.get_bounding_client_rect();
        client_x - r.left() > r.width() / 2.0
    }

    pub fn fraction_along(bar: &web_sys::HtmlDivElement, client_x: f64) -> f64 {
        let r = bar.get_bounding_client_rect();
        if r.width() <= 0.0 {
            return 0.0;
        }
        ((client_x - r.left()) / r.width()).clamp(0.0, 1.0)
    }

    pub fn is_fullscreen() -> bool {
        leptos::prelude::document().fullscreen_element().is_some()
    }

    pub fn toggle_fullscreen(root: &web_sys::HtmlDivElement, video: &web_sys::HtmlVideoElement) {
        let doc = leptos::prelude::document();
        if doc.fullscreen_element().is_some() {
            doc.exit_fullscreen();
            return;
        }
        if root.request_fullscreen().is_ok() {
            return;
        }
        // iPhone Safari has no element fullscreen at all, only the video's
        // own `webkitEnterFullscreen`, which hands the clip to the system
        // player. Better than nothing, and the only thing there is.
        if let Ok(f) = web_sys::js_sys::Reflect::get(video, &"webkitEnterFullscreen".into()) {
            if let Ok(f) = f.dyn_into::<web_sys::js_sys::Function>() {
                let _ = f.call0(video);
            }
        }
    }

    /// Whether the key was typed into something that should keep it: a
    /// field, a button (Enter and Space belong to it), a link.
    pub fn target_is_interactive(ev: &web_sys::KeyboardEvent) -> bool {
        let Some(el) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return false;
        };
        matches!(
            el.tag_name().as_str(),
            "INPUT" | "TEXTAREA" | "SELECT" | "BUTTON" | "A"
        ) || el.get_attribute("contenteditable").is_some()
    }
}

#[cfg(not(feature = "hydrate"))]
mod dom {
    use leptos::web_sys;

    pub fn play(_v: &web_sys::HtmlVideoElement) {}
    pub fn buffered_end(_v: &web_sys::HtmlVideoElement) -> f64 {
        0.0
    }
    pub fn now_ms() -> f64 {
        0.0
    }
    pub fn is_right_half(_ev: &web_sys::PointerEvent, _client_x: f64) -> bool {
        false
    }
    pub fn fraction_along(_bar: &web_sys::HtmlDivElement, _client_x: f64) -> f64 {
        0.0
    }
    pub fn is_fullscreen() -> bool {
        false
    }
    pub fn toggle_fullscreen(_root: &web_sys::HtmlDivElement, _video: &web_sys::HtmlVideoElement) {}
    pub fn target_is_interactive(_ev: &web_sys::KeyboardEvent) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_read_like_a_player_menu() {
        assert_eq!(speed_label(1.0), "1×");
        assert_eq!(speed_label(1.25), "1.25×");
        assert_eq!(speed_label(2.0), "2×");
        assert_eq!(speed_label(0.5), "0.5×");
    }

    /// The cycle starts at normal speed and returns to it; nothing in the
    /// list is a speed a meme is unwatchable at.
    #[test]
    fn speed_cycle_starts_and_ends_at_normal() {
        assert_eq!(SPEEDS[0], 1.0);
        assert!(SPEEDS.iter().all(|s| (0.5..=2.0).contains(s)));
        assert_eq!(SPEEDS.iter().filter(|s| **s == 1.0).count(), 1);
    }
}
