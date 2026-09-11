use crate::app::SitePreview;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};

use crate::api::leaderboard;
use crate::components::density::{stagger, RowSkeleton};
use crate::components::empty::{EmptyState, ErrorState};
use crate::components::icon::LuTrophy;
use crate::seo::absolute;

/// The rank number's colour, as three whole class strings.
///
/// A podium rather than a single winner: first place in the full accent, second
/// and third in the muted one, everyone else quiet. The middle arm is the point
/// of the change -- with two arms, ranks 2 and 3 were styled identically to
/// rank 40, so the top of the list had no shape to it.
///
/// Whole strings, not a toggled utility, because the Tailwind scanner reads
/// these `.rs` sources literally.
fn rank_class(rank: usize) -> &'static str {
    match rank {
        1 => "w-5 flex-none text-right text-[0.8125rem] font-semibold tabular-nums text-accent",
        2 | 3 => {
            "w-5 flex-none text-right text-[0.8125rem] font-semibold tabular-nums text-accent-muted"
        }
        _ => "w-5 flex-none text-right text-[0.8125rem] tabular-nums text-ink-3",
    }
}

/// The row's own surface. First place gets a faint accent wash and hairline so
/// the top of the list is a warm band instead of one more grey row; the colour
/// is never the only signal, since the rank number and its weight already say
/// the same thing.
fn row_class(rank: usize) -> &'static str {
    if rank == 1 {
        "flex items-center gap-3 rounded border border-accent-line bg-accent-veil px-3.5 py-2.5"
    } else {
        "flex items-center gap-3 rounded border border-line bg-surface px-3.5 py-2.5"
    }
}

#[component]
pub fn Leaderboard() -> impl IntoView {
    let entries = Resource::new_blocking(|| (), |_| leaderboard());

    view! {
        <Title text=format!("Leaderboard — {}", crate::flavor::get().name)/>
        <SitePreview/>
        <Meta
            name="description"
            content=format!("Top contributors to {}.", crate::flavor::get().name)
        />
        <Link rel="canonical" href=absolute("/leaderboard")/>

        // No ranking explainer. The list itself communicates the ranking, and
        // the eligibility paragraph that used to sit here was pure narration.
        <h1 class="m-0 mb-4 text-[1.375rem] font-bold tracking-tight lg:mb-6 lg:text-[1.75rem]">"Leaderboard"</h1>

        // A placeholder shaped like the list, rather than a word where the list
        // will be: "tallying…" is one line tall, so the page grew by six rows
        // the moment the tally arrived.
        <Suspense fallback=|| view! { <RowSkeleton count=6/> }>
            {move || {
                entries
                    .get()
                    .map(|res| match res {
                        Err(_) => {
                            view! {
                                <ErrorState
                                    message="Couldn't tally the leaderboard."
                                    retry_href="/leaderboard"
                                />
                            }
                                .into_any()
                        }
                        // An empty leaderboard is an invitation: the way to get
                        // on it is to upload, and this state used to be a
                        // sentence with nowhere to go.
                        Ok(rows) if rows.is_empty() => {
                            view! {
                                <EmptyState
                                    icon=LuTrophy
                                    // Not "no contributors yet", which reads as
                                    // an empty collection: this board only ranks
                                    // people who upload signed in.
                                    message="All contributions were anonymous…"
                                    action_href="/upload"
                                    action_label="Be the first"
                                />
                            }
                                .into_any()
                        }
                        Ok(rows) => {
                            // `.enumerate()` over a plain iterator rather than
                            // <For>: the rank is the row's position, and <For>'s
                            // `let:entry` hands over the item without one. The
                            // list is a snapshot from a resolved Resource -- the
                            // whole branch re-renders when it changes -- so
                            // there is no keyed reconciliation to preserve here.
                            view! {
                                <ol class="m-0 grid list-none gap-2 p-0">
                                    {rows
                                        .into_iter()
                                        .enumerate()
                                        .map(|(i, entry)| {
                                            let rank = i + 1;
                                            // Every interpolated piece is
                                            // itself a whole literal, so the
                                            // scanner still sees each one in
                                            // full somewhere in the source.
                                            let li_class = format!(
                                                "{} {} {}",
                                                row_class(rank),
                                                "animate-rise-in",
                                                stagger(i),
                                            );
                                            // Contributors who signed in without
                                            // a Google picture had no avatar at
                                            // all, so their name started 32px
                                            // left of everyone else's and the
                                            // column zig-zagged. Same initial
                                            // fallback the header account button
                                            // uses.
                                            let initial = entry
                                                .display_name
                                                .chars()
                                                .next()
                                                .map(|c| c.to_uppercase().to_string())
                                                .unwrap_or_else(|| "?".into());
                                            let unit = if entry.upload_count == 1 { crate::flavor::get().noun.clone() } else { crate::flavor::get().nouns.clone() };
                                            view! {
                                                <li class=li_class>
                                                    <span class=rank_class(rank)>{rank}</span>
                                                    {entry
                                                        .avatar_url
                                                        .clone()
                                                        .map(|src| {
                                                            view! {
                                                                <img class="h-8 w-8 flex-none rounded-full object-cover" src=src alt=""/>
                                                            }
                                                                .into_any()
                                                        })
                                                        .unwrap_or_else(|| {
                                                            view! {
                                                                <span class="flex h-8 w-8 flex-none items-center justify-center rounded-full bg-surface-raised text-[0.8rem] font-semibold text-ink-2">
                                                                    {initial.clone()}
                                                                </span>
                                                            }
                                                                .into_any()
                                                        })}
                                                    <span class="min-w-0 flex-1 truncate">
                                                        {entry.display_name.clone()}
                                                    </span>
                                                    // The bare number said
                                                    // nothing about what was
                                                    // being counted; the unit
                                                    // stays quiet so the figure
                                                    // still reads first.
                                                    <span class="flex-none text-[0.8125rem] text-ink-3">
                                                        <span class="font-semibold tabular-nums text-accent">
                                                            {entry.upload_count}
                                                        </span>
                                                        {format!(" {unit}")}
                                                    </span>
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ol>
                            }
                                .into_any()
                        }
                    })
            }}
        </Suspense>
    }
}

#[cfg(test)]
mod tests {
    use super::{rank_class, row_class};

    /// Three tiers, three distinct strings. The middle tier is the one worth
    /// asserting: it was added because ranks 2 and 3 previously rendered
    /// identically to rank 40.
    #[test]
    fn the_podium_has_three_distinct_tiers() {
        let first = rank_class(1);
        let second = rank_class(2);
        let rest = rank_class(4);
        assert_eq!(rank_class(3), second, "3rd shares 2nd's tier");
        assert_ne!(first, second);
        assert_ne!(second, rest);
        assert_ne!(first, rest);
        for r in [5, 10, 999] {
            assert_eq!(rank_class(r), rest, "rank {r} should be the quiet tier");
        }
    }

    /// Colour is never the only signal: the top three are also the only ranks
    /// set in a heavier weight, so the ordering survives being read in
    /// greyscale or by someone who cannot separate the two yellows.
    #[test]
    fn the_podium_is_marked_by_weight_as_well_as_hue() {
        assert!(rank_class(1).contains("font-semibold"));
        assert!(rank_class(2).contains("font-semibold"));
        assert!(rank_class(3).contains("font-semibold"));
        assert!(!rank_class(4).contains("font-semibold"));
    }

    /// Only the winner's row gets the warm band; everything else is the plain
    /// surface. A second highlighted row would make the band mean nothing.
    #[test]
    fn only_first_place_gets_the_warm_row() {
        let winner = row_class(1);
        let plain = row_class(2);
        assert_ne!(winner, plain);
        for r in [2, 3, 4, 50] {
            assert_eq!(row_class(r), plain);
        }
    }

    /// Both variants must describe the same box, or the list visibly steps in
    /// and out at the first row. Only the two paint properties may differ.
    #[test]
    fn both_row_variants_keep_identical_geometry() {
        let geometry = |s: &str| {
            s.split_whitespace()
                .filter(|c| !c.starts_with("border-") || *c == "border")
                .filter(|c| !c.starts_with("bg-"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(geometry(row_class(1)), geometry(row_class(2)));
    }
}
