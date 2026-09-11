use leptos::prelude::*;

use crate::components::icon::{Ico, LuFlag};

use crate::api::{report_item, ReportOutcome};
use crate::components::sign_in::SignInLink;
use crate::models::ReportReason;

/// The report flow: closed by default, opens into a reason picker + optional
/// message on click, rather than a single button that reports with no detail.
///
/// Reporting requires an account (`api::report_item`), checked on submit rather
/// than on open. Checking on open would mean a `current_user` request on every
/// detail page render to answer a question most visitors never ask, and would
/// duplicate the one the header already makes; the cost of checking late is that
/// a signed-out visitor loses an optional free-text message across the sign-in
/// redirect, having already chosen a radio button.
#[component]
pub fn ReportForm(
    item_id: String,
    /// Class for the closed trigger, so a caller whose action bar needs a 44px
    /// touch target can hand one down. Defaults to the same `.icon-btn` every
    /// existing call site was getting, so passing nothing changes nothing.
    #[prop(into, default = "icon-btn".to_string())]
    class: String,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let (reason, set_reason) = signal(ReportReason::OffTopic);
    let message_input: NodeRef<leptos::html::Textarea> = NodeRef::new();
    // `StoredValue`, because the branch that renders the trigger is a `Fn`
    // closure and has to be able to produce this string more than once.
    let trigger_class = StoredValue::new(class);

    let report = Action::new(move |(reason, message): &(ReportReason, String)| {
        let id = item_id.clone();
        let reason = reason.as_str().to_string();
        let message = message.clone();
        async move { report_item(id, reason, Some(message)).await }
    });
    let reported = report.value();

    view! {
        {move || match reported.get() {
            // Nothing submitted yet.
            None => {
                view! {
                    <Show
                        when=move || open.get()
                        fallback=move || {
                            view! {
                                <button
                                    class=trigger_class.get_value()
                                    on:click=move |_| set_open.set(true)
                                    aria-label="Report"
                                    title="Report"
                                >
                                    <Ico icon=LuFlag size=17/>
                                </button>
                            }
                        }
                    >
                        <div class="mt-2.5 grid gap-2.5 rounded border border-line bg-surface p-3.5">
                            <fieldset class="m-0 grid gap-1.5 border-0 p-0">
                                <legend>"what's wrong with it?"</legend>
                                {ReportReason::all()
                                    .into_iter()
                                    .map(|r| {
                                        view! {
                                            <label class="flex items-center gap-2 text-[0.9rem]">
                                                <input
                                                    type="radio"
                                                    name="report-reason"
                                                    checked=move || reason.get() == r
                                                    on:change=move |_| set_reason.set(r)
                                                />
                                                {r.label()}
                                            </label>
                                        }
                                    })
                                    .collect_view()}
                            </fieldset>
                            <textarea
                                node_ref=message_input
                                class="field min-h-[60px] resize-y"
                                placeholder="anything else? (optional)"
                                maxlength="500"
                            ></textarea>
                            <div class="flex items-center gap-2.5">
                                <button
                                    class="btn-quiet"
                                    disabled=move || report.pending().get()
                                    on:click=move |_| {
                                        let message = message_input
                                            .get()
                                            .map(|t| t.value())
                                            .unwrap_or_default();
                                        report.dispatch((reason.get(), message));
                                    }
                                >
                                    {move || {
                                        if report.pending().get() { "Sending…" } else { "Send report" }
                                    }}
                                </button>
                                // btn-quiet, like every other secondary control.
                                // This was a bare unstyled <button> -- no height,
                                // no padding, no border -- next to a real one, so
                                // the way out of the form was the hardest thing in
                                // it to hit. The same flaw was fixed in the admin
                                // row's cancel; this was the other instance.
                                <button class="btn-quiet" on:click=move |_| set_open.set(false)>
                                    "Cancel"
                                </button>
                            </div>
                        </div>
                    </Show>
                }
                    .into_any()
            }
            Some(Ok(ReportOutcome::Recorded)) => {
                view! { <p class="text-[0.9rem] text-ok">"Reported. Someone will look."</p> }
                    .into_any()
            }
            // Nothing was written, so this must not read as done. A link, not a
            // sentence, because a visitor who has just filled the form in is
            // one click from being able to send it.
            Some(Ok(ReportOutcome::SignInRequired)) => {
                view! {
                    <SignInLink attr:class="text-[0.9rem] text-accent underline underline-offset-2 hover:text-accent-hover">
                        "Sign in to report this"
                    </SignInLink>
                }
                    .into_any()
            }
            // Previously indistinguishable from success: the old `when` was
            // `reported.get().is_none()`, which is false for an `Err` too, so a
            // report that never reached D1 still said "Flagged. Someone will
            // look." and then nobody looked.
            Some(Err(e)) => {
                leptos::logging::error!("report failed: {e}");
                view! { <p class="text-[0.9rem] text-danger">"Didn't send. Try again."</p> }
                    .into_any()
            }
        }}
    }
}
