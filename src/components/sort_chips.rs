//! The gallery's sort control, and the shared state behind it.
//!
//! "Item of the day" is an option here rather than a `Sort` variant. It is not an
//! ordering -- it selects a single featured item from a different query
//! (`db::item_of_the_day`) -- so folding it into `Sort` would have meant a fake
//! ordering in the enum, the wire format and the SQL. `GalleryView` keeps that
//! distinction in the UI layer, where it belongs, and leaves `Sort` untouched.
//!
//! The four options used to be four pills, every one of them on screen at all
//! times. That row owned the full width of a phone, stopped fitting somewhere
//! around 360px and scrolled sideways from there -- directly beneath a 56px bar,
//! so two bands of chrome ate the top of the viewport before a single item
//! appeared. It is one button now: it names the current ordering and expands to
//! the five choices on demand. What that costs is discovery -- nobody finds "Item
//! of the day" without opening the menu -- which is exactly why the menu spells
//! every option out in full rather than keeping the abbreviations the strip
//! needed to fit.
//!
//! The file name lags the component on purpose: `SortMenu` lives in
//! `sort_chips.rs` because renaming the module would mean edits to `mod.rs` and
//! `app.rs` for no behaviour change at all.

use leptos::prelude::*;

use crate::components::icon::{Ico, LuCheck, LuChevronDown};
use crate::models::Sort;

/// What the gallery is currently showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GalleryView {
    /// The full gallery in some order.
    Sort(Sort),
    /// Just today's featured item.
    ItemOfDay,
}

impl Default for GalleryView {
    fn default() -> Self {
        Self::Sort(Sort::default())
    }
}

impl GalleryView {
    /// The ordering to query with. `ItemOfDay` has no ordering of its own, so it
    /// reports the default -- callers switch on the view before using this.
    pub fn sort(&self) -> Sort {
        match self {
            Self::Sort(s) => *s,
            Self::ItemOfDay => Sort::default(),
        }
    }
}

/// Shared gallery view. Provided once by `App` so the sort control and `Gallery`
/// read one source of truth.
#[derive(Clone, Copy)]
pub struct SortCtx {
    pub view: ReadSignal<GalleryView>,
    pub set_view: WriteSignal<GalleryView>,
}

/// Reads the view context. Panics only if used outside `App`, which is a wiring
/// mistake rather than a runtime condition.
pub fn use_sort() -> SortCtx {
    use_context::<SortCtx>().expect("SortCtx provided by App")
}

/// Every option, in menu order. One array drives both the trigger's label and
/// the menu's items, so the name of the current ordering cannot drift from the
/// name of the row that selects it.
///
/// Full wording, not the old abbreviations. Those existed because five labels
/// were competing for one 320px row and "Item of the day" alone was as wide as
/// the other three together; with one label visible at a time that constraint is
/// gone. It matters that the wording is real visible text rather than an
/// `aria-label`: the accessible name of every control here is its own text, so
/// there is nothing for a voice-control user to say that the screen does not
/// show (WCAG 2.5.3).
const OPTIONS: [GalleryView; 5] = [
    GalleryView::Sort(Sort::Newest),
    GalleryView::Sort(Sort::MostLiked),
    GalleryView::Sort(Sort::Clips),
    GalleryView::Sort(Sort::Az),
    GalleryView::ItemOfDay,
];

/// The `?view=` query value for a view, and back. The gallery reads the URL on
/// load so `/?view=clips` is a linkable, indexable page of its own, and writes
/// it on change so the back button and a shared link mean what they show.
pub fn view_param(v: GalleryView) -> &'static str {
    match v {
        GalleryView::Sort(s) => s.as_str(),
        GalleryView::ItemOfDay => "day",
    }
}

pub fn view_from_param(s: &str) -> GalleryView {
    match s {
        "day" => GalleryView::ItemOfDay,
        other => GalleryView::Sort(Sort::from_str_or_default(other)),
    }
}

/// The label for a view. Falls back to the first option rather than panicking:
/// every `GalleryView` is in `OPTIONS` by construction, and a panic reached from
/// the render path would take the whole wasm module down to report a typo.
fn label_for(v: GalleryView) -> String {
    let f = crate::flavor::get();
    match v {
        GalleryView::Sort(Sort::Newest) => "Newest".into(),
        GalleryView::Sort(Sort::MostLiked) => f.fill(&f.like.sort_label),
        GalleryView::Sort(Sort::Clips) => "Clips only".into(),
        GalleryView::Sort(Sort::Az) => "A\u{2013}Z".into(),
        GalleryView::ItemOfDay => format!("{} of the day", f.noun_title()),
    }
}

/// Whole class strings per state, the rule everywhere in this codebase: the
/// Tailwind scanner reads these `.rs` files as raw text and never sees a class
/// assembled at runtime.
///
/// These are the pill geometry and colours from `style/tailwind.css` written out
/// in full rather than reached through the shared primitive, so the control
/// looks continuous with the strip it replaces while owning none of it. Layering
/// a utility on top of that primitive would put two rules of equal specificity
/// on the same property and let stylesheet order pick the winner -- the precise
/// class of bug the SCSS deletion existed to remove.
fn trigger_class(open: bool) -> &'static str {
    if open {
        "inline-flex min-h-9 flex-none items-center gap-1.5 whitespace-nowrap rounded-full border border-accent-border bg-accent-soft px-3.5 text-[0.8125rem] font-semibold text-accent transition-colors"
    } else {
        "inline-flex min-h-9 flex-none items-center gap-1.5 whitespace-nowrap rounded-full border border-line bg-transparent px-3.5 text-[0.8125rem] text-ink-2 transition-colors hover:border-line-strong hover:text-ink"
    }
}

/// A menu row, matching the sign-out row in `app.rs`'s account menu so the two
/// popovers in this app read as one system.
///
/// The selected string deliberately has no hover text colour: the accent is what
/// says "this is the current ordering", and it has to survive the pointer being
/// over it.
fn item_class(selected: bool) -> &'static str {
    if selected {
        "flex w-full items-center gap-2 whitespace-nowrap px-3 py-2 text-left text-[0.85rem] font-semibold text-accent transition-colors hover:bg-surface-hover"
    } else {
        "flex w-full items-center gap-2 whitespace-nowrap px-3 py-2 text-left text-[0.85rem] text-ink-2 transition-colors hover:bg-surface-hover hover:text-ink"
    }
}

/// Two whole strings again, for the same reason: a rotation utility toggled
/// against its own zero value is two rules setting `transform` at equal
/// specificity.
fn chevron_class(open: bool) -> &'static str {
    if open {
        "inline-flex rotate-180 transition-transform"
    } else {
        "inline-flex transition-transform"
    }
}

/// The sort control: one button that expands to the five views.
///
/// `Sort`, its `as_str` wire values and the server function signatures are all
/// unchanged -- this is display only.
#[component]
pub fn SortMenu(#[prop(optional, into)] class: Option<String>) -> impl IntoView {
    let SortCtx { view, set_view } = use_sort();

    // Collapsed on the server and on the first hydration pass, which is the only
    // value it can start at: the server has no viewport, no storage and no prior
    // session to seed this from, and a disagreement between the two renders does
    // not degrade into a visual glitch -- it kills the wasm module and the whole
    // page with it.
    let (open, set_open) = signal(false);

    // Which row should hold focus. Tracked as an index rather than read back off
    // the document, because asking for `document.activeElement` means naming
    // `web_sys`, and that crate does not exist in the ssr build.
    let (focus, set_focus) = signal(Option::<usize>::None);

    let trigger: NodeRef<leptos::html::Button> = NodeRef::new();
    // Five separate calls, NOT `[NodeRef::new(); 5]`. `NodeRef` is `Copy`, so the
    // array-repeat form evaluates the expression once and hands the same signal
    // to all five rows -- every arrow key would then focus the same button. It
    // compiles, and it fails silently.
    let items: [NodeRef<leptos::html::Button>; 5] = [
        NodeRef::new(),
        NodeRef::new(),
        NodeRef::new(),
        NodeRef::new(),
        NodeRef::new(),
    ];

    // Moves real focus to whatever `focus` points at. An `Effect`, not a call
    // inside the click handler: setting `open` does not mount the panel
    // synchronously, so focusing from there would target an element that does
    // not exist yet and quietly do nothing. Reading the `NodeRef` here
    // subscribes to it, so this re-runs the instant the button mounts. Effects
    // are client-only by construction, which is also what keeps it out of the
    // server render.
    Effect::new(move |_| {
        let Some(i) = focus.get() else { return };
        let Some(el) = items[i].get() else { return };
        let _ = el.focus();
    });

    // Closing unmounts whatever was focused, so focus would otherwise land on
    // `<body>` and a keyboard user would be back at the top of the document.
    // This draws no focus ring after a pointer click: the global rule in
    // `style/tailwind.css` is `:focus-visible`, and programmatic focus following
    // a mouse event is not that.
    let close_and_refocus = move || {
        set_open.set(false);
        set_focus.set(None);
        if let Some(b) = trigger.get() {
            let _ = b.focus();
        }
    };

    // One handler on the wrapper rather than one per control: keydown bubbles
    // from the trigger and from every row, so this covers the whole widget with
    // no document listener to add, remove and leak -- the same reasoning the
    // account menu gives for its click-outside layer.
    let on_key = move |ev: leptos::ev::KeyboardEvent| {
        let last = OPTIONS.len() - 1;
        match ev.key().as_str() {
            // Guarded on `open` so a collapsed control never swallows these keys
            // from the rest of the page.
            "Escape" if open.get() => {
                ev.prevent_default();
                close_and_refocus();
            }
            k @ ("ArrowDown" | "ArrowUp") => {
                // Otherwise the page scrolls underneath the open menu.
                ev.prevent_default();
                let down = k == "ArrowDown";
                if open.get() {
                    // `focus` is None when the menu was opened by click or by
                    // Enter, which leaves focus on the trigger. Treating that as
                    // index 0 and stepping from it would skip the first row
                    // entirely on the first press.
                    let next = match focus.get() {
                        None => {
                            if down {
                                0
                            } else {
                                last
                            }
                        }
                        Some(cur) if down => {
                            if cur == last {
                                0
                            } else {
                                cur + 1
                            }
                        }
                        Some(cur) => {
                            if cur == 0 {
                                last
                            } else {
                                cur - 1
                            }
                        }
                    };
                    set_focus.set(Some(next));
                } else {
                    set_open.set(true);
                    set_focus.set(Some(if down { 0 } else { last }));
                }
            }
            "Home" if open.get() => {
                ev.prevent_default();
                set_focus.set(Some(0));
            }
            "End" if open.get() => {
                ev.prevent_default();
                set_focus.set(Some(last));
            }
            _ => {}
        }
    };

    // `relative` so the panel hangs off this control rather than off whatever
    // ancestor happens to be positioned. Nothing here may ever gain
    // `overflow-x`, `transform`, `filter` or `contain`: the first clips the
    // panel, the rest re-anchor the fixed layer below to this box instead of to
    // the viewport.
    let cls = format!("relative flex-none {}", class.unwrap_or_default());

    view! {
        <div class=cls on:keydown=on_key>
            // No aria-label: the accessible name is assembled from real text
            // below, so the visible label is always a substring of it. No
            // aria-controls either -- collapsed, this renders no panel, so the
            // IDREF would dangle, and having no id means the control can be
            // mounted twice without colliding.
            <button
                type="button"
                node_ref=trigger
                class=move || trigger_class(open.get())
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| set_open.update(|o| *o = !*o)
            >
                // Read on its own, the label is a bare word sitting next to the
                // density switch; the prefix is what says which control this is.
                // Hidden rather than printed because printing it is what pushes
                // the pill past a 320px screen.
                <span class="sr-only">"Sort by "</span>
                <span>{move || label_for(view.get())}</span>
                <span class=move || chevron_class(open.get())>
                    <Ico icon=LuChevronDown size=15/>
                </span>
            </button>

            <Show when=move || open.get()>
                // A click anywhere else closes it. A full-viewport transparent
                // layer behind the panel does that without a document listener
                // to add, remove and leak. It sits above the trigger, so a click
                // on the trigger while open closes through here and the button's
                // own toggle never fires -- that single-click-closes behaviour is
                // deliberate, not a bug to "fix" into a double toggle.
                //
                // preventDefault on mousedown keeps the click from blurring
                // whatever is focused before the handler runs.
                <div
                    class="fixed inset-0 z-40"
                    on:mousedown=move |ev: leptos::ev::MouseEvent| ev.prevent_default()
                    on:click=move |_| close_and_refocus()
                />
                <div
                    class="absolute left-0 top-full z-50 mt-1 min-w-full overflow-hidden rounded-lg border border-line bg-surface-raised py-1 shadow-lg"
                    role="menu"
                    aria-label="Sort by"
                >
                    {OPTIONS
                        .iter()
                        .enumerate()
                        .map(|(i, target)| {
                            let target = *target;
                            let label = label_for(target);
                            view! {
                                // menuitemradio + aria-checked, not aria-pressed:
                                // this is a single-select group, and the choice is
                                // carried by weight, colour and a tick, so it is
                                // never signalled by hue alone.
                                <button
                                    type="button"
                                    node_ref=items[i]
                                    role="menuitemradio"
                                    aria-checked=move || (view.get() == target).to_string()
                                    class=move || item_class(view.get() == target)
                                    // Keeps the tracked index honest when focus
                                    // arrives by Tab or by pointer rather than
                                    // from the arrow keys above.
                                    on:focus=move |_| set_focus.set(Some(i))
                                    on:click=move |_| {
                                        set_view.set(target);
                                        close_and_refocus();
                                    }
                                >
                                    // Fixed-width leading slot so all five labels
                                    // share one left edge whether or not the tick
                                    // is drawn for that row.
                                    <span class="inline-flex w-4 flex-none justify-center">
                                        {move || {
                                            (view.get() == target)
                                                .then(|| view! { <Ico icon=LuCheck size=14/> })
                                        }}
                                    </span>
                                    {label}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
            </Show>
        </div>
    }
}
