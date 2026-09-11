use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;

use crate::api::{admin_delete_item, admin_flagged_items, admin_set_public};
use crate::components::empty::{EmptyState, ErrorState};
use crate::components::icon::{Ico, LuCheck, LuCircleAlert, LuUserRound};
use crate::components::sign_in::SignInLink;
use crate::models::{AdminQueue, FlaggedItem};

/// A destructive action, written out in full rather than as `btn-quiet` plus
/// overrides.
///
/// This replaced `.btn-quiet` plus two important-flagged hover colours, where
/// the flags were load-bearing: the primitive already sets a hover border and a
/// hover text colour, so without them the two rules sat at equal specificity and
/// the winner came down to their order in the generated stylesheet. That is the
/// precise failure mode this project deleted its stylesheet to escape, and
/// forcing it treats the symptom. Same geometry, its own hover colours, nothing
/// to fight.
///
/// The old class names are deliberately not written out anywhere above: the
/// Tailwind scanner reads these `.rs` files as plain text and does not skip
/// comments, so quoting them verbatim kept emitting the very rules this removed.
const BTN_DANGER: &str = "inline-flex min-h-9 items-center gap-2 rounded border border-line \
     bg-transparent px-3 text-[0.85rem] text-ink-2 transition-colors hover:border-danger \
     hover:bg-danger/10 hover:text-danger disabled:cursor-not-allowed disabled:opacity-50";

/// The report queue. Gated server-side in every server fn it calls
/// (`api::require_admin`) — this component hiding itself from a non-admin is
/// a courtesy, not the actual access control.
#[component]
pub fn Admin() -> impl IntoView {
    let flagged = Resource::new_blocking(|| (), |_| admin_flagged_items());
    // Bumped after any action to force a refetch, since the three admin
    // actions below are plain Actions, not tied to this Resource directly.
    let (refresh, set_refresh) = signal(0u32);
    Effect::new(move |_| {
        refresh.get();
        flagged.refetch();
    });

    view! {
        <Title text=format!("admin — {}", crate::flavor::get().name)/>
        // Nothing above the gate: the heading lives inside the one arm that is
        // actually allowed to see it, so a signed-out visitor gets the sign-in
        // prompt and nothing else.

        <Suspense fallback=|| view! { <p class="py-14 text-center text-ink-2">"loading queue…"</p> }>
            {move || {
                flagged
                    .get()
                    .map(|res| match res {
                        // The shared states, not two more hand-rolled ones.
                        // Besides the inconsistency, the old error branch
                        // rendered `{e.to_string()}` straight into the page --
                        // the exact thing `ErrorState` was written to stop, and
                        // the raw server-fn wording is no more useful to an
                        // admin than to anyone else. The real error is still
                        // logged server-side.
                        Err(_) => {
                            view! {
                                <ErrorState
                                    message="Could not load the queue."
                                    retry_href="/admin"
                                />
                            }
                                .into_any()
                        }
                        // Not signed in at all: offer the way in rather than a
                        // refusal. `rel="external"` because /auth/google/login is an
                        // Axum route and leptos_router would otherwise intercept the
                        // click and render its own 404.
                        Ok(AdminQueue::SignInRequired) => {
                            view! {
                                <section class="flex min-h-[46vh] flex-col items-center justify-center gap-3 text-center">
                                    <span class="text-ink-3"><Ico icon=LuUserRound size=26/></span>
                                    <h2 class="m-0 text-[1.1rem] font-semibold">"Sign in to see this"</h2>
                                    <p class="m-0 max-w-[34ch] text-[0.9rem] text-ink-2">
                                        "The report queue is only visible to admins."
                                    </p>
                                    <SignInLink attr:class="btn">"Sign in"</SignInLink>
                                </section>
                            }
                                .into_any()
                        }
                        // Signed in, but not an admin. Says so plainly: this is a
                        // decision about the account, not a fault, and the previous
                        // page rendered "admin access required" in red as though
                        // something had gone wrong.
                        Ok(AdminQueue::Denied) => {
                            view! {
                                <section class="flex min-h-[46vh] flex-col items-center justify-center gap-3 text-center">
                                    <span class="text-ink-3"><Ico icon=LuCircleAlert size=26/></span>
                                    <h2 class="m-0 text-[1.1rem] font-semibold">"You don't have access to this"</h2>
                                    <p class="m-0 max-w-[38ch] text-[0.9rem] text-ink-2">
                                        "This account isn't an admin, so the report queue isn't yours to see. Nothing is broken."
                                    </p>
                                    <A href="/" attr:class="btn">"Back to the collection"</A>
                                </section>
                            }
                                .into_any()
                        }
                        Ok(AdminQueue::Queue(rows)) => {
                            // Built before the wrapper rather than with a
                            // `<Show>`: Show's children have to be `Fn`, and the
                            // `For` below consumes `rows`, so a Show here fails
                            // to compile for reasons that have nothing to do
                            // with the empty state.
                            let queue = if rows.is_empty() {
                                view! { <EmptyState icon=LuCheck message="Nothing flagged right now."/> }
                                    .into_any()
                            } else {
                                view! {
                                    <div class="grid gap-3.5">
                                        <For each=move || rows.clone() key=|f| f.item.id.clone() let:flagged>
                                            <AdminRow flagged=flagged on_change=move || set_refresh.update(|n| *n += 1)/>
                                        </For>
                                    </div>
                                }
                                    .into_any()
                            };
                            view! {
                                // Sentence case, like "Leaderboard" and "Search".
                                <h1 class="m-0 mb-4 text-[1.375rem] font-bold tracking-tight lg:mb-6 lg:text-[1.75rem]">
                                    "Report queue"
                                </h1>
                                {queue}
                            }
                                .into_any()
                        }
                    })
            }}
        </Suspense>
    }
}

#[component]
fn AdminRow(
    flagged: FlaggedItem,
    on_change: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let item = flagged.item;
    let has_reports = !flagged.reports.is_empty();
    let (confirming_delete, set_confirming_delete) = signal(false);
    let is_public = item.is_public;
    let id_for_toggle = item.id.clone();
    let id_for_delete = item.id.clone();

    let toggle = Action::new(move |()| {
        let id = id_for_toggle.clone();
        async move { admin_set_public(id, !is_public).await }
    });
    let delete = Action::new(move |()| {
        let id = id_for_delete.clone();
        async move { admin_delete_item(id).await }
    });

    // Both actions refetch the queue on success, from a plain Effect rather
    // than chaining inside the Action itself — simpler than threading the
    // parent's refetch through two separate async closures.
    Effect::new(move |_| {
        if toggle.value().get().is_some_and(|r| r.is_ok()) {
            on_change();
        }
    });
    Effect::new(move |_| {
        if delete.value().get().is_some_and(|r| r.is_ok()) {
            on_change();
        }
    });

    view! {
        <article class="flex flex-col gap-3.5 rounded-lg border border-line bg-surface p-3.5 sm:flex-row">
            <img class="h-[88px] w-[88px] flex-none rounded object-cover" src=item.thumb_url.clone() alt=""/>
            <div class="grid min-w-0 flex-1 gap-2">
                <div class="flex min-w-0 items-center gap-2.5">
                    <a
                        class="truncate font-medium transition-colors hover:text-accent"
                        href=crate::flavor::get().item_path(&item.slug)
                    >
                        {item.title.clone()}
                    </a>
                    // Tinted rather than filled, and in theme colours. The
                    // filled version needed a hand-picked foreground for each
                    // state -- a raw `#06301c` on the green and white on the
                    // red -- which are the only two hardcoded hex values left
                    // anywhere in the UI and match nothing in the palette. A
                    // translucent fill lets `ok`/`danger` be both the text and
                    // the background, so the badge stays in the system.
                    <span class=if item.is_public {
                        "flex-none rounded-full border border-ok/30 bg-ok/15 px-2 py-0.5 text-[0.72rem] font-medium text-ok"
                    } else {
                        "flex-none rounded-full border border-danger/30 bg-danger/15 px-2 py-0.5 text-[0.72rem] font-medium text-danger"
                    }>
                        {if item.is_public { "visible" } else { "hidden" }}
                    </span>
                </div>
                // A item with no reports is in this queue because an admin hid
                // it by hand -- `db::flagged_items` selects `reports > 0 OR
                // is_public = 0`. Say so, or the row reads as a bug in the queue.
                {(!has_reports)
                    .then(|| {
                        view! {
                            <p class="m-0 text-[0.85rem] text-ink-2">
                                "Hidden by an admin, not reported."
                            </p>
                        }
                    })}
                <ul class="m-0 pl-5 text-[0.85rem] text-ink-2">
                    <For
                        each={
                            let reports = flagged.reports.clone();
                            move || reports.clone()
                        }
                        key=|r| format!("{}{}", r.reason, r.created_at)
                        let:r
                    >
                        <li>
                            <strong>{r.reason.clone()}</strong>
                            {r.message.clone().map(|m| view! { <span>": " {m}</span> })}
                        </li>
                    </For>
                </ul>
                <div class="flex items-center gap-2.5">
                    <button
                        class="btn-quiet"
                        disabled=move || toggle.pending().get()
                        on:click=move |_| {
                            toggle.dispatch(());
                        }
                    >
                        {if is_public { "Hide" } else { "Unhide" }}
                    </button>
                    <Show
                        when=move || confirming_delete.get()
                        fallback=move || {
                            view! {
                                <button
                                    class=BTN_DANGER
                                    on:click=move |_| set_confirming_delete.set(true)
                                >
                                    "Delete"
                                </button>
                            }
                        }
                    >
                        <button
                            class=BTN_DANGER
                            disabled=move || delete.pending().get()
                            on:click=move |_| {
                                delete.dispatch(());
                            }
                        >
                            "Really delete? No undo"
                        </button>
                        // `btn-quiet`, like every other secondary control.
                        // Cancel was a bare unstyled <button> -- no height, no
                        // padding, no border -- sitting next to two real ones,
                        // so the safe way out of a destructive confirmation was
                        // the least clickable thing in the row.
                        <button
                            class="btn-quiet"
                            on:click=move |_| set_confirming_delete.set(false)
                        >
                            "Cancel"
                        </button>
                    </Show>
                </div>

                // Failures used to be silent: both Actions dropped their Err
                // on the floor, so a hide or a delete that the server refused
                // looked exactly like one that worked -- the row simply stayed
                // as it was. On an admin surface that is the difference between
                // "already handled" and "still live".
                {move || {
                    let failed = toggle.value().get().is_some_and(|r| r.is_err())
                        || delete.value().get().is_some_and(|r| r.is_err());
                    failed
                        .then(|| {
                            view! {
                                <p class="m-0 text-[0.8rem] text-danger" role="alert">
                                    "That didn't go through. Try again."
                                </p>
                            }
                        })
                }}
            </div>
        </article>
    }
}
