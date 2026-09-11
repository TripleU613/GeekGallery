use leptos::prelude::*;
use leptos_router::components::A;

use std::time::Duration;

use crate::components::icon::{Ico, LuPlay};
use crate::components::like::LikeButton;
use crate::models::{Item, MediaKind};

/// How long a mouse has to rest on a clip before it starts playing in the
/// tile. Long enough that sweeping across the grid fetches nothing.
const PREVIEW_DELAY_MS: u64 = 220;

/// `72.4` -> `1:12`, for the duration badge on a clip. Whole seconds, minutes
/// unpadded, hours only when there are any.
pub fn short_duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[component]
pub fn ItemCard(
    item: Item,
    /// Set on the handful of tiles that are above the fold on first paint.
    ///
    /// `loading="lazy"` on every image sounds strictly better and is not: the
    /// browser deprioritises lazy images, so the one that decides this page's
    /// LCP -- a tile in the first row -- queues behind work it should have led.
    /// The gallery marks its opening row and nothing else; everything below is
    /// lazy, which at 10k items is the only reason the grid is affordable at all.
    #[prop(default = false)]
    priority: bool,
) -> impl IntoView {
    let href = crate::flavor::get().item_path(&item.slug);
    // One binding per use. The title is needed three times -- as the link's
    // accessible name, as the image's alt text, and as the visible caption --
    // and the view macro captures each of those by move, so a `.clone()` written
    // inside the macro is already too late for the use after it.
    let title = item.title.clone();
    let untitled = item.is_untitled();
    let link_label = match item.kind {
        MediaKind::Video => format!("{title} (video)"),
        MediaKind::Gif => format!("{title} (GIF)"),
        MediaKind::Image => title.clone(),
    };
    let alt_text = title.clone();
    let byline = match &item.uploader {
        Some(u) => format!("by {}", u.display_name),
        None => "anonymous".to_string(),
    };
    // The second caption line: who, and for a clip how long. The length used
    // to be a badge of its own in the corner; three overlays on one tile was
    // one too many, and next to the byline it reads as a fact about the clip
    // rather than a sticker on it.
    let byline = match (item.kind, item.duration) {
        (MediaKind::Video, Some(d)) if d > 0.0 => format!("{} · {byline}", short_duration(d)),
        _ => byline,
    };
    let is_video = item.kind == MediaKind::Video;
    let is_gif = item.kind == MediaKind::Gif;

    // Hover-to-play, for a mouse only. The tile is a JPEG frame until a
    // pointer that can hover has rested on it for `PREVIEW_DELAY_MS`; then the
    // clip itself is loaded into a <video> over the frame, muted and looping,
    // and dropped again when the pointer leaves. Fingers never trigger it (a
    // tap navigates anyway), and R2 charges nothing for egress, so the only
    // cost is the visitor's own bandwidth on a clip they are pointing at.
    //
    // `preview_src` is `None` on the server and stays so until the hover:
    // the <video> is not in the HTML at all, so hydration has nothing to
    // disagree about.
    let (preview_src, set_preview_src) = signal(Option::<String>::None);
    let (preview_live, set_preview_live) = signal(false);
    let preview_timer: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let clip_src = StoredValue::new(item.orig_url.clone());
    let on_enter = move |ev: leptos::ev::PointerEvent| {
        if !is_video || ev.pointer_type() != "mouse" {
            return;
        }
        if let Some(h) = preview_timer.get_value() {
            h.clear();
        }
        let h = set_timeout_with_handle(
            move || {
                preview_timer.set_value(None);
                set_preview_src.set(Some(clip_src.get_value()));
            },
            Duration::from_millis(PREVIEW_DELAY_MS),
        )
        .ok();
        preview_timer.set_value(h);
    };
    let on_leave = move |_: leptos::ev::PointerEvent| {
        if let Some(h) = preview_timer.get_value() {
            h.clear();
            preview_timer.set_value(None);
        }
        set_preview_live.set(false);
        set_preview_src.set(None);
    };
    // A tab that goes to the background drops its preview: nothing is looking.
    if is_video {
        Effect::new(move |_| {
            let handle = window_event_listener(leptos::ev::visibilitychange, move |_| {
                if document().hidden() {
                    set_preview_live.set(false);
                    set_preview_src.set(None);
                }
            });
            on_cleanup(move || handle.remove());
        });
    }
    let preview_class = move || {
        if preview_live.get() {
            "pointer-events-none absolute inset-0 block h-full w-full object-cover transition-opacity duration-200 ease-out opacity-100"
        } else {
            "pointer-events-none absolute inset-0 block h-full w-full object-cover transition-opacity duration-200 ease-out opacity-0"
        }
    };

    view! {
        // Square, and image-only.
        //
        // Square because a grid of mixed aspect ratios is masonry, and masonry
        // at 24 tiles a page reflows on every image load. The thumbnail is
        // centre-cropped by `object-cover` into the tile; the item's own shape
        // is shown whole on its page.
        //
        // Image-only because a caption block underneath every tile is a strip
        // of chrome, and twelve of them in a grid read as a table of labels
        // rather than a wall of items. The text sits over the image instead,
        // where it costs no layout at all.
        //
        // `content-visibility:auto` is what makes a very long grid affordable.
        // It lets the browser skip layout, style and paint for every tile that
        // is off screen, and pick the work back up as one scrolls in. It needs
        // no intrinsic-size hint here because `aspect-square` already fixes
        // the box's height from its grid-assigned width, so a skipped tile
        // still reserves exactly the right space and the scrollbar does not
        // jump.
        //
        // The tile deliberately has NO entrance animation. An element at
        // opacity 0 is not a valid LCP candidate in Chrome, and under
        // `content-visibility:auto` an off-screen subtree may never start its
        // animation and sit at its 0% frame forever. Do not add `will-change:
        // transform` to the image either: it forces layer promotion on every
        // tile in the grid, which is precisely the cost `content-visibility` is
        // here to avoid paying.
        <div
            class="card group relative aspect-square [content-visibility:auto] has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent"
            on:pointerenter=on_enter
            on:pointerleave=on_leave
        >
            // The anchor covers the whole tile. `aria-label` carries the title,
            // because the link's only content is an image and the caption that
            // names it lives outside the anchor.
            <A
                href=href
                attr:class="absolute inset-0 block focus-visible:outline-none"
                attr:aria-label=link_label
            >
                <img
                    src=item.thumb_url.clone()
                    alt=alt_text
                    loading=if priority { "eager" } else { "lazy" }
                    fetchpriority=if priority { "high" } else { "auto" }
                    decoding="async"
                    class="absolute inset-0 block h-full w-full object-cover transition-transform duration-300 ease-out group-hover:scale-[1.03]"
                />
                // The hover preview, only ever present mid-hover on a clip.
                // Faded in on `playing`, so the frame never flashes to black
                // while the first bytes arrive.
                {move || {
                    preview_src
                        .get()
                        .map(|src| {
                            view! {
                                <video
                                    class=preview_class
                                    src=src
                                    muted
                                    loop
                                    playsinline
                                    autoplay
                                    preload="auto"
                                    aria-hidden="true"
                                    tabindex="-1"
                                    on:playing=move |_| set_preview_live.set(true)
                                />
                            }
                        })
                }}
            </A>

            // What a tap does, top-left, always visible: a play disc for a
            // clip, "GIF" for a GIF, nothing for a still. One small mark in a
            // corner the caption never reaches, rather than a badge across the
            // bottom competing with the title.
            {is_video
                .then(|| {
                    view! {
                        <span class="pointer-events-none absolute left-2 top-2 inline-flex h-7 w-7 items-center justify-center rounded-full bg-black/55 text-white shadow backdrop-blur-sm">
                            <Ico icon=LuPlay size=13 class="translate-x-px"/>
                        </span>
                    }
                })}
            {is_gif
                .then(|| {
                    view! {
                        <span class="pointer-events-none absolute left-2 top-2 inline-flex h-6 items-center rounded-full bg-black/55 px-2 text-[0.625rem] font-bold tracking-wide text-white shadow backdrop-blur-sm">
                            "GIF"
                        </span>
                    }
                })}

            // The caption, revealed on hover.
            //
            // `[@media(hover:none)]:opacity-100` is the part that matters: a
            // touch device never fires hover, so keyed on hover alone the title
            // and the like button would be permanently invisible on every phone.
            // There, the caption simply stays up. The media query neutralises
            // the transform as well as the opacity: cover only the opacity and
            // every phone gets a permanently visible caption sitting 4px below
            // where it belongs.
            //
            // `group-focus-within` keeps it reachable by keyboard, since tabbing
            // to the link or the like button never triggers hover either.
            //
            // pointer-events-none so the gradient does not intercept clicks
            // meant for the link underneath; the like button switches them back
            // on for itself alone.
            <span class="pointer-events-auto absolute right-2 top-2 translate-y-0.5 opacity-0 transition-[opacity,transform] duration-300 ease-out group-hover:translate-y-0 group-hover:opacity-100 group-focus-within:translate-y-0 group-focus-within:opacity-100 [@media(hover:none)]:translate-y-0 [@media(hover:none)]:opacity-100">
                <LikeButton
                    id=item.id.clone()
                    initial_count=item.likes
                    initial_liked=item.liked_by_me
                    small=true
                />
            </span>

            // No accent colour in the caption text, deliberately. This block
            // sits over an arbitrary picture, and the black scrim is the only
            // reason the title is guaranteed legible at all. The tile's colour
            // comes from the `.card` hover edge and the like button, both of
            // which sit on backgrounds whose colour is known.
            //
            // Nothing else sits along the bottom edge now, so the caption has
            // the whole width; the kind mark is top-left and the like button
            // top-right.
            <div class="pointer-events-none absolute inset-x-0 bottom-0 translate-y-1 rounded-b-lg bg-gradient-to-t from-black/90 via-black/55 to-transparent px-2.5 pb-2.5 pt-10 opacity-0 transition-[opacity,transform] duration-300 ease-out group-hover:translate-y-0 group-hover:opacity-100 group-focus-within:translate-y-0 group-focus-within:opacity-100 [@media(hover:none)]:translate-y-0 [@media(hover:none)]:opacity-100">
                // An invented title ("untitled clip") is a fact about the
                // upload, not a name, so it is set like one: small, dim,
                // italic. The bold treatment is reserved for words a person
                // chose.
                {if untitled {
                    view! {
                        <span class="block truncate text-[0.75rem] font-normal italic leading-snug text-ink-3">
                            "untitled"
                        </span>
                    }
                    .into_any()
                } else {
                    view! {
                        <span class="block truncate text-[0.8125rem] font-semibold leading-snug text-ink">
                            {title}
                        </span>
                    }
                    .into_any()
                }}
                <span class="block truncate text-[0.6875rem] tabular-nums leading-normal text-ink-2">
                    {byline}
                </span>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::short_duration;

    #[test]
    fn durations_read_like_a_player() {
        assert_eq!(short_duration(0.0), "0:00");
        assert_eq!(short_duration(7.4), "0:07");
        assert_eq!(short_duration(72.6), "1:13");
        assert_eq!(short_duration(3599.0), "59:59");
        assert_eq!(short_duration(3661.0), "1:01:01");
    }
}
