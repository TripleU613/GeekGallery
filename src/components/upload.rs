use crate::app::SitePreview;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;

use crate::components::icon::{Ico, LuCircleAlert, LuCloudUpload, LuX};
use crate::components::share::ShareButton;
use crate::models::{Item, MediaKind, Step, UploadResult};
use crate::seo::absolute;

// `Progress` is deserialized only inside the poll loop, which is hydrate-only.
// The signal below holds a bare `Step`, so nothing in the view names this type
// any more -- left in the shared import it becomes an unused import under
// `--features ssr` and `clippy -D warnings` fails the whole gate.
#[cfg(feature = "hydrate")]
use crate::models::Progress;

/// How often the browser asks the server where the upload has got to. Fast
/// enough that each step is visibly acknowledged, slow enough that a minute of
/// processing is ~75 requests rather than thousands.
#[cfg(feature = "hydrate")]
const POLL_MS: u32 = 800;

/// Deliberately hand-restated copies of `storage::MAX_IMAGE_BYTES` and
/// `storage::MAX_VIDEO_BYTES`, not references: `crate::storage` is
/// `#[cfg(feature = "ssr")]` and cannot be named from a component that also
/// compiles to wasm. If those constants ever move, these and the size line in
/// the prompt drift in silence.
///
/// Worth the duplication because the alternative is the worst outcome this page
/// has: a 70 MB file uploads in full, takes minutes over a phone connection,
/// and only then fails on the server.
#[cfg(feature = "hydrate")]
const MAX_IMAGE_BYTES: f64 = 12.0 * 1024.0 * 1024.0;
#[cfg(feature = "hydrate")]
const MAX_VIDEO_BYTES: f64 = 60.0 * 1024.0 * 1024.0;

/// Mirrors the `accept` attribute on the input. It exists twice because
/// `accept` filters the file picker and does precisely nothing for a dropped
/// file -- a drop is the only reason this list is checked in Rust at all.
#[cfg(feature = "hydrate")]
const ACCEPTED: [&str; 7] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "video/mp4",
    "video/webm",
    "video/quicktime",
];

/// The `accept` attribute, as one string. The picker filters on it; the check
/// above is what actually decides.
const ACCEPT_ATTR: &str =
    "image/png,image/jpeg,image/webp,image/gif,video/mp4,video/webm,video/quicktime";

/// Where in a clip the cover frame is looked for, as fractions of its length.
/// Never the first frame: on a phone recording it is black or blurred more
/// often than not, and a meme clip's opens on a title card as often as not.
/// Each candidate is drawn small and scored for contrast; the liveliest wins
/// and becomes the default the uploader can then override with the slider.
#[cfg(feature = "hydrate")]
const COVER_CANDIDATES: [f64; 4] = [0.12, 0.3, 0.5, 0.72];

/// The fallback when a clip is too short or too broken to score.
#[cfg(feature = "hydrate")]
const POSTER_AT_SECONDS: f64 = 0.8;

/// The longest edge of the captured poster. The server bounds stills at 2048
/// and thumbnails at 480; drawing more than this is a bigger JPEG for nothing.
#[cfg(feature = "hydrate")]
const POSTER_MAX_EDGE: f64 = 1280.0;

/// The box every outcome is drawn in. A `const` rather than four copies of the
/// same literal; the Tailwind scanner reads this file as raw text, so a `const`
/// is exactly as visible to it as a class attribute would be.
const OUTCOME_SHELL: &str =
    "mt-4 flex flex-col items-center gap-3 rounded-lg border border-line bg-surface p-4 text-center";

/// The four steps the server reports, collapsed to the three facts a visitor can
/// do anything with: the file is leaving their device, someone has it, it is
/// being put away. Reciting "Fingerprinting" at them is the app talking about
/// itself.
fn phase(step: Step, importing: bool) -> &'static str {
    match step {
        Step::Receiving if importing => "Fetching",
        Step::Receiving => "Uploading",
        Step::Fingerprinting | Step::Cropping => "Working",
        Step::Storing => "Saving",
    }
}

/// Where a step sits on the bar: `(start, ceiling, tau)`, as fractions of one.
///
/// Within a band the fill is `start + (ceiling - start) * t / (t + tau)`, where
/// `t` is the number of polls seen since this step began. Three properties make
/// that honest, and each of them is easy to void by accident later:
///
/// - **Monotonic.** Every band's ceiling is the next band's floor, and the
///   hyperbolic term never reaches 1, so the bar can never walk backwards.
///   Reordering the pipeline in `upload_route.rs` without reordering this
///   function breaks it.
/// - **Always moving.** `t` grows on every poll even when the server reports the
///   same step for a while, so the bar never looks stuck.
/// - **Never full before `Done`.** The last ceiling is 0.99 and is approached,
///   never reached. A bar that completes and then waits is the classic lie.
///
/// The weights: receiving is most of the wait for a 60MB clip (the POST itself
/// is not polled, so this band only covers the tail), fingerprinting is a hash
/// of the whole file, and storing is the push to R2 -- which for a video is the
/// slow half. `tau` is in polls, not seconds, because `POLL_MS` is hydrate-only
/// and this function compiles under both feature sets.
fn band(step: Step) -> (f64, f64, f64) {
    match step {
        Step::Receiving => (0.00, 0.10, 4.0),
        Step::Fingerprinting => (0.10, 0.35, 4.0),
        Step::Cropping => (0.35, 0.45, 2.0),
        Step::Storing => (0.45, 0.99, 8.0),
    }
}

/// The free upload page.
///
/// Posts multipart directly to `/api/upload` rather than through a server
/// function: server fns would have to base64 the file through a serde payload,
/// inflating it by a third for no benefit.
///
/// No sign-in anywhere on this page, on purpose. `upload_route::upload` falls
/// back to an anonymous uploader when the session cookie is missing, so an
/// account only ever adds attribution -- it is never a gate.
#[component]
pub fn Upload() -> impl IntoView {
    let (preview, set_preview) = signal(Option::<String>::None);
    // What was picked, by container, so the preview can be a <video> or an
    // <img>. Decided from the MIME the browser reports, the same list the
    // server would sniff -- a wrong guess here only mis-draws the preview, the
    // server still decides from the bytes.
    let (kind, set_kind) = signal(MediaKind::Image);
    let (filename, set_filename) = signal(Option::<String>::None);
    let (size_text, set_size_text) = signal(Option::<String>::None);
    let (busy, set_busy) = signal(false);
    let (result, set_result) = signal(Option::<UploadResult>::None);
    // Where the pipeline is, as last reported, and how many polls have come back
    // saying so. Together they are the whole input to the progress bar. A `Step`
    // rather than a `Progress` because the three terminal states already move
    // into `result` -- keeping them here too would mean two places to look for
    // "did this finish".
    let (step, set_step) = signal(Option::<Step>::None);
    let (polls, set_polls) = signal(0u32);
    // Whether the preview image has decoded. Drives its fade-in: the element
    // mounts transparent and this flips on the `load` event, which for an object
    // URL is always at least a task after mount, so the transparent style has
    // been computed by then and the transition actually runs. An event handler
    // is not render, so writing a signal here is not the hydration-killing kind.
    let (loaded, set_loaded) = signal(false);
    // Whether a file is currently being dragged over the drop zone. Purely
    // visual, but without it there is no feedback that the page will accept it.
    let (dragging, set_dragging) = signal(false);
    // For a clip: how long it is, and which moment is the cover. The preview
    // sits paused on the cover, so what the uploader sees is exactly the
    // poster the server will get. Both are written from element events and the
    // slider, never during render.
    let (clip_len, set_clip_len) = signal(0.0_f64);
    let (cover_at, set_cover_at) = signal(0.0_f64);
    // Whether the running job started from a pasted link rather than a file:
    // the first phase is then "Fetching", not "Uploading".
    let (importing, set_importing) = signal(false);

    let file_input: NodeRef<leptos::html::Input> = NodeRef::new();
    let title_input: NodeRef<leptos::html::Input> = NodeRef::new();
    let link_input: NodeRef<leptos::html::Input> = NodeRef::new();

    // The captcha (Cloudflare Turnstile). `captcha_on` is whether the server
    // has a site key at all -- off locally, on in production -- and
    // `captcha_token` is the answer the widget produced, spent by the server
    // on the next submit and cleared here right after. The widget id is what
    // `turnstile.reset` wants; a `StoredValue` because it is read from inside
    // async blocks that would otherwise have to own a `String`.
    let (captcha_on, set_captcha_on) = signal(false);
    let (captcha_token, set_captcha_token) = signal(Option::<String>::None);
    let captcha_box: NodeRef<leptos::html::Div> = NodeRef::new();
    let captcha_widget: StoredValue<Option<String>> = StoredValue::new(None);
    // Read only from hydrate-only blocks; the server build sees them unused.
    #[cfg(not(feature = "hydrate"))]
    let _ = (
        set_captcha_on,
        captcha_token,
        set_captcha_token,
        captcha_widget,
    );
    // Spend the token: the widget shows a fresh box and the next submit has
    // to earn a new one. Called after every answer from the server, whatever
    // it said, because the server has already used the token by then.
    #[cfg(feature = "hydrate")]
    let reset_captcha = move || {
        set_captcha_token.set(None);
        if let Some(id) = captcha_widget.get_value() {
            turnstile::reset(&id);
        }
    };

    // Client-only: ask the server whether the gate is on, and if so pull in
    // Turnstile's script and render the widget into its box. The script is
    // appended from here rather than emitted in the HTML so a site without a
    // key never loads it. Everything Turnstile-specific is in `turnstile`,
    // below.
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            leptos::task::spawn_local(async move {
                let Ok(Some(key)) = crate::api::captcha_site_key().await else {
                    return;
                };
                set_captcha_on.set(true);
                turnstile::load_script();
                // Give the node a tick to exist, then wait for the script.
                for _ in 0..150 {
                    gloo_timers::future::TimeoutFuture::new(100).await;
                    let Some(el) = captcha_box.get_untracked() else {
                        continue;
                    };
                    if !turnstile::ready() {
                        continue;
                    }
                    let el: &web_sys::Element = &el;
                    captcha_widget.set_value(turnstile::render(el, &key, set_captcha_token));
                    return;
                }
                leptos::logging::warn!("turnstile script never became ready");
            });
        }
    });
    // The preview <video>, when there is one. Submit reads the frame, the
    // dimensions and the duration straight off it -- the element has already
    // decoded the clip to show it, so there is nothing to decode twice.
    let video_preview: NodeRef<leptos::html::Video> = NodeRef::new();

    // Every setter above is written only from the hydrate-only handlers below,
    // so the server build sees them as unused. The form is inert until wasm
    // takes over, which is expected — not a missing code path.
    #[cfg(not(feature = "hydrate"))]
    let _ = (
        set_preview,
        set_kind,
        set_filename,
        set_size_text,
        set_busy,
        set_result,
        set_step,
        set_polls,
        set_loaded,
        set_dragging,
        set_clip_len,
        set_importing,
        title_input,
        link_input,
        video_preview,
    );

    // Back to "nothing picked", from either the Remove button or a file this
    // page refuses. Clearing the input matters in both cases: a drop assigns the
    // FileList before anything is validated, so a rejected drop would otherwise
    // leave the zone showing the previous image while the input still held the
    // bad file, and submit reads the input.
    //
    // `set_value("")` is the specified way to empty a file input; `set_files`
    // with an empty list is not reliably honoured.
    #[cfg(feature = "hydrate")]
    let clear_picked = move || {
        if let Some(old) = preview.get_untracked() {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
        set_preview.set(None);
        set_filename.set(None);
        set_size_text.set(None);
        set_loaded.set(false);
        if let Some(input) = file_input.get() {
            input.set_value("");
        }
    };

    // Shared by the file picker and by a drop, so both routes produce the same
    // preview and the same cleared-out previous result. Only compiled for the
    // browser: `web_sys::File` is a hydrate-only dependency, and there is no
    // file picker on the server.
    #[cfg(feature = "hydrate")]
    let show_preview = move |file: web_sys::File| {
        // File is a Blob subclass in the DOM; web-sys models that as AsRef,
        // so no cast is needed.
        let blob: &web_sys::Blob = file.as_ref();

        // Fails open on an empty type. Several platforms hand over a `File` with
        // no MIME type at all -- a drop out of some file managers, and anything
        // with an extension the OS does not recognise -- and refusing those
        // would turn away valid files with nothing the visitor could act on.
        // The server decodes by sniffing the bytes anyway, so this check is a
        // courtesy, not the boundary.
        let mime = blob.type_();
        if !mime.is_empty() && !ACCEPTED.contains(&mime.as_str()) {
            clear_picked();
            set_result.set(Some(UploadResult::Error {
                message: "That needs to be a PNG, JPG, WEBP, GIF, MP4 or WEBM.".into(),
            }));
            return;
        }
        let picked_kind = if mime.starts_with("video/") {
            MediaKind::Video
        } else if mime == "image/gif" {
            MediaKind::Gif
        } else {
            MediaKind::Image
        };
        let limit = if picked_kind == MediaKind::Video {
            MAX_VIDEO_BYTES
        } else {
            MAX_IMAGE_BYTES
        };
        if blob.size() > limit {
            clear_picked();
            set_result.set(Some(UploadResult::Error {
                message: if picked_kind == MediaKind::Video {
                    "That clip is over 60 MB.".into()
                } else {
                    "That file is over 12 MB.".into()
                },
            }));
            return;
        }
        set_kind.set(picked_kind);

        // Revoke the URL this one replaces, not the one just created: every
        // picked file used to leak an object URL for the life of the page, and
        // revoking the new one instead simply blanks the preview.
        if let Some(old) = preview.get_untracked() {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
        // Before `set_preview`, so the freshly mounted image is built from a
        // `loaded` that is already false and has something to transition from.
        set_loaded.set(false);
        if let Ok(url) = web_sys::Url::create_object_url_with_blob(blob) {
            set_preview.set(Some(url));
        }
        set_filename.set(Some(file.name()));
        let bytes = blob.size();
        set_size_text.set(Some(if bytes >= 1024.0 * 1024.0 {
            format!("{:.1} MB", bytes / (1024.0 * 1024.0))
        } else {
            format!("{:.0} KB", (bytes / 1024.0).max(1.0))
        }));
        set_result.set(None);
        set_step.set(None);
        set_polls.set(0);
    };

    let on_file_change = move |_| {
        #[cfg(feature = "hydrate")]
        {
            if let Some(file) = file_input
                .get()
                .and_then(|i| i.files())
                .and_then(|f| f.get(0))
            {
                show_preview(file);
            }
        }
    };

    let remove = move |_ev: leptos::ev::MouseEvent| {
        #[cfg(feature = "hydrate")]
        {
            clear_picked();
            set_result.set(None);
        }
    };

    // Drag and drop. The handlers sit on the wrapper around the drop zone, not
    // on the label itself, because the Remove button overlays the zone as a
    // sibling of that label: a file dropped on that corner would land on an
    // element with no preventDefault on dragover, and the browser would navigate
    // to the image instead -- exactly the failure the comment below was written
    // for. The wrapper is the one element every drop is guaranteed to reach.
    //
    // dragover MUST preventDefault on every event, not just once: the browser's
    // default for a dragged file is "navigate to it", and it re-checks on each
    // dragover. Without it the drop silently opens the image instead, which is
    // exactly how this was broken.
    let on_drag_over = move |ev: leptos::ev::DragEvent| {
        ev.prevent_default();
        set_dragging.set(true);
    };
    let on_drag_leave = move |ev: leptos::ev::DragEvent| {
        ev.prevent_default();
        set_dragging.set(false);
    };
    let on_drop = move |ev: leptos::ev::DragEvent| {
        ev.prevent_default();
        set_dragging.set(false);
        #[cfg(feature = "hydrate")]
        {
            // The file input is disabled while an upload runs, which stops the
            // click route, but a drop is delivered to the wrapper regardless.
            // Swapping the file out from under a running job would leave the
            // preview and the job describing two different images.
            if busy.get_untracked() {
                return;
            }
            let Some(dt) = ev.data_transfer() else { return };
            let Some(files) = dt.files() else { return };
            let Some(file) = files.get(0) else { return };
            // Assign the dropped FileList onto the hidden input, rather than
            // keeping the File in a signal: submit reads the input, so this
            // keeps one source of truth and means a drop and a click are
            // indistinguishable from there on.
            if let Some(input) = file_input.get() {
                input.set_files(Some(&files));
            }
            show_preview(file);
        }
    };

    // A item from a link. Same job registry, same poll, same outcomes as a
    // file -- only the first phase differs, and the server does the fetching.
    #[cfg(feature = "hydrate")]
    let start_import = move |url: String| {
        if busy.get_untracked() {
            return;
        }
        if captcha_on.get_untracked() && captcha_token.get_untracked().is_none() {
            set_result.set(Some(UploadResult::Error {
                message: "Tick the box to show you're human, then try again.".into(),
            }));
            return;
        }
        let captcha = captcha_token.get_untracked().unwrap_or_default();
        let title = title_input.get().map(|t| t.value()).unwrap_or_default();
        set_busy.set(true);
        set_result.set(None);
        set_importing.set(true);
        set_step.set(Some(Step::Receiving));
        set_polls.set(0);
        leptos::task::spawn_local(async move {
            let body = format!(
                r#"{{"url":"{}","title":"{}","captcha":"{}"}}"#,
                crate::seo::json_escape(&url),
                crate::seo::json_escape(&title),
                crate::seo::json_escape(&captcha)
            );
            let sent = match gloo_net::http::Request::post("/api/import")
                .header("content-type", "application/json")
                .body(body)
            {
                Ok(req) => req.send().await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let queued = match sent {
                Ok(resp) => {
                    resp.json::<UploadResult>()
                        .await
                        .unwrap_or_else(|e| UploadResult::Error {
                            message: format!("unexpected reply from the server: {e}"),
                        })
                }
                Err(e) => UploadResult::Error {
                    message: format!("could not reach the server: {e}"),
                },
            };
            reset_captcha();
            let UploadResult::Queued { job } = queued else {
                set_busy.set(false);
                set_step.set(None);
                set_importing.set(false);
                set_result.set(Some(queued));
                return;
            };
            follow(job, set_step, set_polls, set_busy, set_result).await;
            set_importing.set(false);
        });
    };

    let import_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        #[cfg(feature = "hydrate")]
        {
            let url = link_input
                .get()
                .map(|i| i.value())
                .unwrap_or_default()
                .trim()
                .to_string();
            if url.is_empty() {
                return;
            }
            start_import(url);
        }
    };

    // Paste anywhere on the page: an image from the clipboard becomes the
    // picked file, a link becomes an import. Not while typing a title, where
    // a paste is a paste. Registered in an `Effect` so it exists only on the
    // client and goes away with the page.
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            let handle = window_event_listener(leptos::ev::paste, move |ev| {
                if busy.get_untracked() {
                    return;
                }
                let target: Option<web_sys::HtmlInputElement> = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                let in_title = target.as_ref().is_some_and(|i| i.id() == "item-title");
                let in_link = target.as_ref().is_some_and(|i| i.id() == "import-link");
                if in_title {
                    return;
                }
                let Some(dt) = ev.clipboard_data() else {
                    return;
                };
                if let Some(files) = dt.files() {
                    if let Some(file) = files.get(0) {
                        ev.prevent_default();
                        if let Some(input) = file_input.get() {
                            input.set_files(Some(&files));
                        }
                        show_preview(file);
                        return;
                    }
                }
                let text = dt.get_data("text").unwrap_or_default().trim().to_string();
                if text.starts_with("https://") || text.starts_with("http://") {
                    if !in_link {
                        ev.prevent_default();
                    }
                    if let Some(i) = link_input.get() {
                        i.set_value(&text);
                    }
                    start_import(text);
                }
            });
            on_cleanup(move || handle.remove());
        }
    });

    // Whether the last outcome was a published item: the forms give way to
    // the success card, and come back from it.
    let succeeded = move || matches!(result.get(), Some(UploadResult::Ok { .. }));
    let upload_another = Callback::new(move |_: ()| {
        #[cfg(feature = "hydrate")]
        {
            clear_picked();
            set_importing.set(false);
            if let Some(i) = link_input.get() {
                i.set_value("");
            }
            if let Some(t) = title_input.get() {
                t.set_value("");
            }
            reset_captcha();
        }
        set_result.set(None);
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        #[cfg(feature = "hydrate")]
        {
            let Some(input) = file_input.get() else {
                return;
            };
            let Some(file) = input.files().and_then(|f| f.get(0)) else {
                set_result.set(Some(UploadResult::Error {
                    message: "Pick a file first.".into(),
                }));
                return;
            };

            if captcha_on.get_untracked() && captcha_token.get_untracked().is_none() {
                set_result.set(Some(UploadResult::Error {
                    message: "Tick the box to show you're human, then try again.".into(),
                }));
                return;
            }
            let captcha = captcha_token.get_untracked().unwrap_or_default();

            let title = title_input.get().map(|t| t.value()).unwrap_or_default();
            // Taken now, on the main thread, before anything async: the node
            // is only meaningful while the preview is mounted.
            let video_el = video_preview.get();

            set_busy.set(true);
            set_result.set(None);
            set_step.set(Some(Step::Receiving));
            set_polls.set(0);

            leptos::task::spawn_local(async move {
                let form = web_sys::FormData::new().expect("FormData unavailable");
                let _ = form.append_with_blob_and_filename("file", &file, &file.name());
                let _ = form.append_with_str("title", &title);
                let _ = form.append_with_str("captcha", &captcha);

                // For a clip: a frame, the frame size and the length, all read
                // off the preview element the visitor has been looking at. The
                // server cannot open a video, so this is the only place a
                // poster can come from. A failure here sends nothing extra and
                // the server paints a placeholder -- the upload still goes.
                if let Some(video) = video_el {
                    let _ = form.append_with_str("width", &video.video_width().to_string());
                    let _ = form.append_with_str("height", &video.video_height().to_string());
                    let d = video.duration();
                    if d.is_finite() && d > 0.0 {
                        let _ = form.append_with_str("duration", &format!("{d:.3}"));
                    }
                    if let Some(poster) = capture_poster(&video, cover_at.get_untracked()).await {
                        let _ = form.append_with_blob_and_filename("poster", &poster, "poster.jpg");
                    }
                }

                let queued =
                    match gloo_net::http::Request::post("/api/upload")
                        .body(form)
                        .expect("FormData is a valid body")
                        .send()
                        .await
                    {
                        Ok(resp) => resp.json::<UploadResult>().await.unwrap_or_else(|e| {
                            UploadResult::Error {
                                message: format!("unexpected reply from the server: {e}"),
                            }
                        }),
                        Err(e) => UploadResult::Error {
                            message: format!("could not reach the server: {e}"),
                        },
                    };
                reset_captcha();

                // Anything but a job id is already final -- a malformed request,
                // or the server refusing before it started work.
                let UploadResult::Queued { job } = queued else {
                    set_busy.set(false);
                    set_step.set(None);
                    set_result.set(Some(queued));
                    return;
                };

                follow(job, set_step, set_polls, set_busy, set_result).await;
            });
        }
    };

    // The bar's fill, already in percent and already floored. One closure so the
    // width and the announced value cannot disagree.
    //
    // The floor is 2%: a zero-width bar is indistinguishable from no bar at all,
    // and "we have started" is true from the moment the POST leaves.
    let shown = move || {
        let raw = match step.get() {
            Some(s) => {
                let (start, ceiling, tau) = band(s);
                let t = polls.get() as f64;
                start + (ceiling - start) * t / (t + tau)
            }
            None => 0.0,
        };
        (raw * 100.0).max(2.0)
    };

    // Three whole class strings, never a shared base with a state layered on
    // top. The version this replaces put `bg-surface` in the base and appended
    // the drag tint in the branch: both landed, both were single-class selectors
    // at equal specificity, and the later rule in the stylesheet won -- so the
    // drag-hover tint had never once rendered, and it looked fine because the
    // border still changed. The base below holds no background, no border colour
    // and no border style, which is the whole fix.
    let zone = move || {
        let base = "relative grid min-h-[260px] min-w-0 cursor-pointer grid-cols-[minmax(0,1fr)] place-items-center overflow-hidden rounded-lg border-2 p-5 text-center transition-colors duration-200 ease-out has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent lg:min-h-[320px]";
        let state = if dragging.get() {
            "border-solid border-accent bg-accent-soft"
        } else if preview.get().is_some() {
            "border-solid border-line-strong bg-surface"
        } else {
            "border-dashed border-line bg-surface hover:border-accent-border has-[:focus-visible]:border-accent"
        };
        format!("{base} {state}")
    };

    view! {
        <Title text=format!("Upload {}: picture, GIF or clip — {}", crate::flavor::get().a_noun(), crate::flavor::get().name)/>
        <SitePreview/>
        <Meta
            name="description"
            content=format!("Add {} to {}: a picture, GIF or video clip. Free, no account needed, up to 60 MB for video.", crate::flavor::get().a_noun(), crate::flavor::get().name)
        />
        <Link rel="canonical" href=absolute("/upload")/>

        <section class="mx-auto max-w-[620px] pt-5">
            // Just the task name. The paragraph that used to sit here narrated
            // the service ("Free, no account. Everything is checked before it
            // goes live…") -- none of which a visitor needs in order to pick a
            // file, so it is gone rather than reworded.
            <h1 class="m-0 mb-4 text-[1.375rem] font-bold tracking-tight lg:mb-6 lg:text-[1.75rem]">"Contribute"</h1>

            // `grid-cols-[minmax(0,1fr)]`, not a bare `grid`: an implicit track
            // is sized by its widest item's min-content, and the drop zone's
            // min-content came to 388px at a 393px viewport. That widened the
            // single track and dragged the file input and submit button out
            // with it, so the whole form scrolled sideways on a phone. The
            // explicit `minmax(0, ...)` lets the track shrink below min-content.
            // Both forms live inside one wrapper that hides on success -- hidden,
            // not unmounted, so the file input and the preview keep their state
            // and "Upload another" brings them straight back.
            <div class=move || if succeeded() { "hidden" } else { "contents" }>
            // A link first, because that is what most people have on a phone:
            // a share sheet, not a file. YouTube, Instagram, X, Facebook, a
            // bare image or clip URL -- the server fetches it at original
            // quality and it runs through the same pipeline as a file.
            // Pasting a link (or an image) anywhere on the page does the same
            // without touching this field.
            <form on:submit=import_submit aria-busy=move || busy.get().to_string() class="mb-3.5 grid grid-cols-[minmax(0,1fr)_auto] gap-2">
                <input
                    class="field"
                    id="import-link"
                    node_ref=link_input
                    type="url"
                    inputmode="url"
                    autocomplete="off"
                    spellcheck="false"
                    placeholder="Paste a link: YouTube, Instagram, X, Facebook, or an image URL"
                    aria-label="Link to import"
                    disabled=move || busy.get()
                />
                <button type="submit" class="btn" disabled=move || busy.get()>
                    "Fetch"
                </button>
            </form>

            // The captcha, between the two forms because one answer serves
            // either: Fetch above it, Upload below it. Hidden until the server
            // says there is a site key, and sized to the widget so the page
            // does not jump when it renders. Turnstile paints its own frame in
            // here; nothing is styled from this side.
            <div
                node_ref=captcha_box
                class=move || if captcha_on.get() { "mb-3.5 min-h-[65px]" } else { "hidden" }
                aria-live="polite"
            ></div>

            <form on:submit=submit aria-busy=move || busy.get().to_string() class="grid grid-cols-[minmax(0,1fr)] gap-3.5">
                // The wrapper, not the label, owns the drag handlers -- see
                // `on_drag_over`. It is also the positioning context for the
                // three things that sit over the zone: the Remove button, the
                // busy overlay, and the preview itself.
                <div
                    class="relative"
                    on:dragover=on_drag_over
                    on:dragenter=on_drag_over
                    on:dragleave=on_drag_leave
                    on:drop=on_drop
                >
                    // `has-[:focus-visible]` puts the focus ring on the drop
                    // zone, because the input it belongs to is visually hidden
                    // inside it -- without it a keyboard user tabbing here would
                    // see nothing at all change. `overflow-hidden` clips the
                    // preview and the filename strip to the rounded border; it
                    // does not clip the element's own outline, so the focus ring
                    // still draws in full.
                    //
                    // The height is pinned rather than grown into. Before this,
                    // picking a file took the zone from 260px to as much as
                    // 420px and shoved the title field, the button and every
                    // result box down the page by around 160px, which is the
                    // single most jarring thing this page did.
                    <label class=zone>
                        // `sr-only`, not a styled-down native control: a file
                        // input's width comes from its own "Choose file / no file
                        // selected" chrome, which is 344px in Chrome and refuses to
                        // shrink -- `max-width` does not clamp it, so it pushed the
                        // page sideways at 360px and narrower. The label around it
                        // already renders the whole drop zone, including the
                        // "Choose a file" prompt the native button duplicated.
                        // Hidden this way it stays focusable and screen-reader
                        // reachable, unlike `display: none`.
                        //
                        // Disabled while a job runs so the zone cannot open a
                        // picker mid-upload. Nothing is submitted natively -- the
                        // multipart body is assembled by hand -- so disabling it
                        // costs nothing at submit time.
                        <input
                            class="sr-only"
                            type="file"
                            accept=ACCEPT_ATTR
                            disabled=move || busy.get()
                            node_ref=file_input
                            on:change=on_file_change
                        />
                        // Permanently mounted and faded, never unmounted. Two
                        // reasons, both real: this text is the label's content
                        // and therefore the sr-only input's only accessible
                        // name, so swapping it out for the preview left a
                        // nameless control; and an element that stays put is the
                        // only thing a CSS transition can animate, which is the
                        // whole of the motion budget here (no custom keyframes
                        // exist in this project's config).
                        //
                        // `pointer-events-none` is load-bearing rather than
                        // tidy: dragleave bubbles up from children, so dragging
                        // across this text used to fire dragleave on the zone
                        // and flicker the highlight off -- masked only by the
                        // next dragover setting it straight back. Making every
                        // inner layer transparent to hit-testing removes the
                        // flicker outright and lets clicks fall through to the
                        // label.
                        <span class=move || {
                            if preview.get().is_some() {
                                "pointer-events-none grid gap-1 text-center text-ink-2 transition-opacity duration-200 ease-out opacity-0"
                            } else {
                                "pointer-events-none grid gap-1 text-center text-ink-2 transition-opacity duration-200 ease-out opacity-100"
                            }
                        }>
                            <span class="mx-auto inline-flex text-ink-3">
                                <Ico icon=LuCloudUpload size=26/>
                            </span>
                            <strong>
                                // "Drop a file" is an instruction a
                                // phone cannot follow -- there is
                                // nothing to drag with -- so touch gets
                                // the prompt that matches what it can
                                // actually do.
                                //
                                // Chosen by `pointer:fine` rather than a
                                // width breakpoint, because the question
                                // is whether this device has a pointer,
                                // not how wide it is: a 1400px touch
                                // screen still cannot drag, and a 700px
                                // window with a mouse still can. Both
                                // strings are rendered and CSS hides
                                // one, so nothing branches on the
                                // viewport in Rust -- the server has no
                                // window, and disagreeing with the
                                // client there is what killed the wasm
                                // module once already.
                                {move || {
                                    if dragging.get() {
                                        view! { <span>"Drop it"</span> }.into_any()
                                    } else {
                                        view! {
                                            <span>
                                                <span class="[@media(pointer:fine)]:hidden">
                                                    "Choose a file"
                                                </span>
                                                <span class="hidden [@media(pointer:fine)]:inline">
                                                    "Drop a file, or choose one"
                                                </span>
                                            </span>
                                        }
                                            .into_any()
                                    }
                                }}
                            </strong>
                            // Kept: the format/size line prevents a
                            // failed upload, and these values mirror
                            // MAX_UPLOAD_BYTES and the accept list
                            // rather than being restated by hand.
                            <small>"PNG, JPG, WEBP, GIF · 12 MB — MP4, WEBM · 60 MB"</small>
                        </span>

                        // A mapping closure, not a `<Show>` reading the signal
                        // back out with `unwrap_or_default`: that form can emit
                        // `src=""` on the frame it unmounts, which some browsers
                        // resolve as a fresh request for the current document.
                        // Reachable now that Remove can put `preview` back to
                        // None. This closure captures a real String and cannot
                        // produce an empty src.
                        //
                        // Absolutely positioned because an in-flow image is what
                        // used to resize the zone. The class is its own reactive
                        // closure so only the attribute updates on load -- if
                        // the opacity were read in the outer closure the whole
                        // element would be rebuilt on every load event, which
                        // both kills the fade and re-fires `load` forever.
                        {move || {
                            let cls = move || {
                                if loaded.get() {
                                    "pointer-events-none absolute inset-0 h-full w-full rounded object-contain p-3 transition-opacity duration-300 ease-out opacity-100"
                                } else {
                                    "pointer-events-none absolute inset-0 h-full w-full rounded object-contain p-3 transition-opacity duration-300 ease-out opacity-0"
                                }
                            };
                            preview
                                .get()
                                .map(|url| {
                                    if kind.get() == MediaKind::Video {
                                        // Paused on the cover frame, not
                                        // playing: the preview *is* the
                                        // poster, so the uploader can see what
                                        // the grid and every unfurl will show.
                                        // `loadeddata` rather than `load`,
                                        // which a <video> never fires; once
                                        // the first frame is decodable the
                                        // cover is chosen (see `auto_cover`).
                                        view! {
                                            // `autoplay muted loop`: not to
                                            // show motion, but because iPhone
                                            // Safari decodes nothing for a
                                            // paused <video> until it has
                                            // played, and a canvas draw of an
                                            // undecoded clip is a black
                                            // frame -- which is what five of
                                            // the first eight clips shipped
                                            // with. The cover pick pauses it
                                            // on the chosen frame.
                                            <video
                                                class=cls
                                                node_ref=video_preview
                                                src=url
                                                autoplay
                                                muted
                                                loop
                                                playsinline
                                                preload="auto"
                                                on:loadedmetadata=move |_| {
                                                    if let Some(v) = video_preview.get_untracked() {
                                                        let d = v.duration();
                                                        if d.is_finite() && d > 0.0 {
                                                            set_clip_len.set(d);
                                                        }
                                                    }
                                                }
                                                on:loadeddata=move |_| {
                                                    set_loaded.set(true);
                                                    #[cfg(feature = "hydrate")]
                                                    if let Some(v) = video_preview.get_untracked() {
                                                        leptos::task::spawn_local(async move {
                                                            let at = auto_cover(&v).await;
                                                            let _ = v.pause();
                                                            set_cover_at.set(at);
                                                        });
                                                    }
                                                }
                                            />
                                        }
                                            .into_any()
                                    } else {
                                        view! {
                                            <img
                                                class=cls
                                                src=url
                                                alt=""
                                                on:load=move |_| set_loaded.set(true)
                                            />
                                        }
                                            .into_any()
                                    }
                                })
                        }}

                        // The filename, pinned inside the zone rather than sat
                        // in the form below it -- it was the second thing that
                        // shifted the whole page when a file was picked. Same
                        // gradient treatment the gallery cards already use, so
                        // it stays readable over a white photograph. Hidden
                        // while busy: it shares this edge with the progress bar
                        // and the two never need to be seen at once.
                        {move || {
                            (preview.get().is_some() && !busy.get())
                                .then(|| {
                                    view! {
                                        <span class="pointer-events-none absolute inset-x-0 bottom-0 flex items-center gap-2 bg-gradient-to-t from-black/90 via-black/60 to-transparent px-3 pb-2.5 pt-10 text-left text-[0.8125rem] text-ink-2">
                                            <span class="min-w-0 truncate">
                                                {move || filename.get().unwrap_or_default()}
                                            </span>
                                            <span class="flex-none tabular-nums text-ink-3">
                                                {move || size_text.get().unwrap_or_default()}
                                            </span>
                                        </span>
                                    }
                                })
                        }}
                    </label>

                    // There was no way to un-pick a file at all before this.
                    //
                    // Not the `.icon-btn` primitive: that sets a transparent
                    // border, a transparent background, a muted text colour and
                    // a surface hover, and this control sits on top of an
                    // arbitrary photograph, so it needs all four different.
                    // Adding them as utilities would put four equal-specificity
                    // pairs in play and hand the result to stylesheet order.
                    // One standalone string instead, with the same on-image
                    // treatment the small like button uses.
                    {move || {
                        (preview.get().is_some() && !busy.get())
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="absolute right-2 top-2 inline-flex h-9 w-9 items-center justify-center rounded-full border border-white/25 bg-black/55 text-ink backdrop-blur-sm transition-colors hover:border-danger hover:text-danger"
                                        aria-label="Remove"
                                        on:click=remove
                                    >
                                        <Ico icon=LuX size=16/>
                                    </button>
                                }
                            })
                    }}

                    // The busy overlay. Mounted from first paint and faded by an
                    // opacity swap rather than mounted on demand, for two
                    // reasons: a scrim that pops in reads as a glitch, and the
                    // live region inside it has to already exist when its text
                    // arrives -- a region inserted at the same moment as its
                    // contents announces nothing at all.
                    <div class=move || {
                        if busy.get() {
                            "pointer-events-none absolute inset-0 overflow-hidden rounded-lg bg-black/60 opacity-100 backdrop-blur-sm transition-opacity duration-300 ease-out"
                        } else {
                            "pointer-events-none absolute inset-0 overflow-hidden rounded-lg bg-black/60 opacity-0 backdrop-blur-sm transition-opacity duration-300 ease-out"
                        }
                    }>
                        // One word, and it is empty when idle so nothing is
                        // announced until there is something to announce. The
                        // whole of the narration this page used to do: six named
                        // pipeline stages, each with its own row and its own
                        // tick. It changes three times in fifty seconds, which
                        // is about the rate a spoken announcement can be useful
                        // at -- and it is exactly why the percentage is kept out
                        // of this element and left on the bar as an attribute.
                        <p
                            class="m-0 flex h-full w-full items-center justify-center text-[0.9375rem] font-semibold text-ink"
                            aria-live="polite"
                        >
                            {move || step.get().map(|s| phase(s, importing.get())).unwrap_or_default()}
                        </p>

                        // The one thing the progress display could not say:
                        // `upload_route::upload` drains the multipart, spawns the
                        // pipeline with `tokio::spawn` and answers 202, so the
                        // work is not attached to this tab at all. People sat
                        // through fifty seconds of a scrim not knowing that.
                        //
                        // Not shown during `Receiving`, which is the one phase
                        // where it would be a lie: the body is still on its way
                        // up and no job exists yet, so closing the tab there
                        // really does cancel everything. `step` is the only
                        // signal that can tell those apart without touching the
                        // poll loop -- the server's first report is
                        // `Fingerprinting`, so anything past `Receiving` is proof
                        // the bytes landed and a spawned task owns them now.
                        //
                        // The second clause is the honest half and has to stay:
                        // nothing notifies anybody. The result exists only in
                        // this page's poll loop, and `jobs::RETAIN` drops it
                        // after five minutes, so a visitor who leaves has to go
                        // and find their item in the gallery.
                        //
                        // Deliberately outside the live region above rather than
                        // a second line inside it: it is reassurance that does
                        // not change, and in there it would be re-announced
                        // every time the phase word did.
                        //
                        // The gradient is the filename strip's, for the same
                        // reason: this edge sits over whatever image was picked,
                        // and 13px `ink-2` on `black/60` alone measures 2.8:1
                        // over a white photograph -- well under the 4.5:1 the
                        // palette is built to. The two never collide, because
                        // the filename strip is hidden exactly while this shows.
                        // The bar is painted after it, so it lands on the darkest
                        // part of the gradient rather than on the photograph.
                        <Show when=move || {
                            busy.get() && step.get().is_some_and(|s| s != Step::Receiving)
                        }>
                            <p class="m-0 absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/90 via-black/60 to-transparent px-4 pb-3 pt-10 text-center text-[0.8125rem] text-ink-2">
                                "Closing the tab won't stop it — you just won't see the link when it's done."
                            </p>
                        </Show>

                        // The bar is the only computed visual on this page and
                        // its width is an inline style, never a class: a
                        // `w-[37%]` assembled at runtime is invisible to the
                        // Tailwind scanner and would generate no rule at all.
                        //
                        // The 700ms transition against an 800ms poll makes the
                        // fill glide continuously instead of stepping once a
                        // second. The target is always the honest number; the
                        // glide is interpolation toward it, never past it. The
                        // global reduced-motion rule already turns this into a
                        // snap, so there are no motion variants layered here.
                        <Show when=move || busy.get()>
                            <div
                                role="progressbar"
                                aria-label="Upload progress"
                                aria-valuemin="0"
                                aria-valuemax="100"
                                aria-valuenow=move || (shown().round() as i32).to_string()
                                class="absolute inset-x-0 bottom-0 h-1 overflow-hidden bg-white/15"
                            >
                                <div
                                    class="h-full bg-accent transition-[width] duration-700 ease-out"
                                    style:width=move || format!("{:.1}%", shown())
                                />
                            </div>
                        </Show>
                    </div>
                </div>

                // A refusal or a failure lands right here, under the thing it
                // is about, not below the button at the far end of the form.
                {move || {
                    result
                        .get()
                        .and_then(|r| match r {
                            UploadResult::Rejected { reason } => Some(reason),
                            UploadResult::Error { message } => Some(message),
                            _ => None,
                        })
                        .map(|line| {
                            view! {
                                <Outcome
                                    icon=LuCircleAlert
                                    tone="inline-flex h-9 w-9 items-center justify-center rounded-full bg-surface-raised text-danger"
                                    headline=line
                                />
                            }
                        })
                }}

                // The cover picker, clips only. The slider seeks the paused
                // preview, so dragging it is scrubbing through the clip and
                // wherever it stops is the poster. The value is a plain range
                // input styled by `accent-color`: the native control already
                // does touch, keyboard and a11y right, and a bespoke one would
                // have to earn all three back.
                {move || {
                    (kind.get() == MediaKind::Video && preview.get().is_some() && !busy.get())
                        .then(|| {
                            view! {
                                <label class="flex items-center gap-3 rounded-lg border border-line bg-surface px-3 py-2 text-[0.8125rem] text-ink-2">
                                    <span class="flex-none font-semibold text-ink">"Cover"</span>
                                    <input
                                        type="range"
                                        class="min-w-0 flex-1 accent-accent"
                                        min="0"
                                        max=move || format!("{:.2}", clip_len.get().max(0.05))
                                        step="0.05"
                                        prop:value=move || format!("{:.2}", cover_at.get())
                                        aria-label="Cover frame"
                                        on:input=move |ev| {
                                            let Ok(t) = event_target_value(&ev).parse::<f64>() else {
                                                return;
                                            };
                                            set_cover_at.set(t);
                                            if let Some(v) = video_preview.get_untracked() {
                                                let _ = v.pause();
                                                v.set_current_time(t);
                                            }
                                        }
                                    />
                                    <span class="flex-none tabular-nums">
                                        {move || crate::components::card::short_duration(cover_at.get())}
                                    </span>
                                </label>
                            }
                        })
                }}

                // Labelled, not just placeholder'd: a placeholder stops being an
                // accessible name the moment anything is typed into the field.
                // No utilities on top of `.field` -- it already owns the width,
                // border, background, padding, text size and placeholder colour.
                <input
                    class="field"
                    id="item-title"
                    type="text"
                    placeholder=format!("Name this {}", crate::flavor::get().noun)
                    aria-label=format!("Name this {}", crate::flavor::get().noun)
                    maxlength="80"
                    node_ref=title_input
                />

                // The label stays "Upload" while a job runs. The overlay above
                // is already saying what is happening, and two controls
                // narrating the same state is precisely the blabber this page
                // was cut down to remove. `.btn` carries the disabled styling.
                <button class="btn" type="submit" disabled=move || busy.get()>
                    "Upload"
                </button>
            </form>

            </div>

            // The success card takes the forms' place: what went up, where it
            // is, and the way back. See `Success`.
            {move || {
                match result.get() {
                    Some(UploadResult::Ok { item }) => {
                        view! { <Success item=*item again=upload_another/> }.into_any()
                    }
                    _ => ().into_any(),
                }
            }}
        </section>
    }
}

/// One result box. Four near-identical blocks lived here before, and the risk
/// with that is not the duplication itself but that they drifted.
///
/// `tone` is the whole class string for the icon medallion, passed as a literal
/// from each call site rather than crossed with a colour modifier here: the
/// scanner reads complete literals, and a base class plus a colour utility is
/// the equal-specificity coin flip this codebase keeps out of its stylesheet.
///
/// No `children` prop. Nothing else in this project uses one, and every outcome
/// is the same three optional pieces.
#[component]
fn Outcome(
    icon: icondata_core::Icon,
    tone: &'static str,
    #[prop(into)] headline: String,
    /// Second line. Only the held-item outcome has one, and it is the reason
    /// that outcome cannot share copy with a refusal.
    #[prop(optional, into)]
    note: Option<String>,
) -> impl IntoView {
    view! {
        <div class=OUTCOME_SHELL>
            <span class=tone>
                <Ico icon=icon size=18/>
            </span>
            <p class="m-0 text-[0.9375rem] font-semibold text-ink">{headline}</p>
            {note.map(|n| view! { <p class="m-0 text-[0.85rem] text-ink-2">{n}</p> })}
        </div>
    }
}

/// The card that replaces the form once a item is published.
///
/// Before this, "Uploaded" was one line in a grey box under the submit button,
/// below the drop zone that still showed the file, the title field, and the
/// button -- three things that had just stopped mattering, above the one that
/// had started to. Now the forms hide and this stands where they were: the tick
/// lands and draws itself, the item's own still confirms what went up, and the
/// three things to do next are the only controls on the page.
///
/// The still is `thumb_url`: for a clip that is the poster frame at grid size,
/// for a GIF a frame, for a still the still. All three are JPEG and already
/// cached by the time this renders, since the server wrote them moments ago.
#[component]
fn Success(item: Item, again: Callback<()>) -> impl IntoView {
    let page = crate::flavor::get().item_path(&item.slug);
    let what = match item.kind {
        MediaKind::Video => "Your clip is live and already in the grid.".to_string(),
        MediaKind::Gif => "Your GIF is live and already in the grid.".to_string(),
        MediaKind::Image => format!(
            "Your {} is live and already in the grid.",
            crate::flavor::get().noun
        ),
    };
    // One binding per use inside the macro (the card.rs lesson).
    let title = item.title.clone();
    let alt = title.clone();
    let shown = title.clone();
    let share_title = title.clone();
    view! {
        <div class="animate-rise-in flex flex-col items-center gap-4 rounded-lg border border-line bg-surface px-5 py-7 text-center">
            <span class="animate-pop inline-flex h-16 w-16 items-center justify-center rounded-full bg-ok/15 text-ok">
                <svg
                    viewBox="0 0 24 24"
                    class="h-8 w-8"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                >
                    <path class="animate-draw" pathLength="1" stroke-dasharray="1" d="M5 12.5l4.5 4.5L19 7"/>
                </svg>
            </span>
            <div>
                <p class="m-0 text-[1.25rem] font-bold tracking-tight text-ink">"It's up."</p>
                <p class="m-0 mt-1 text-[0.875rem] text-ink-2">{what}</p>
            </div>
            <A
                href=page.clone()
                attr:class="block w-full max-w-[260px] overflow-hidden rounded-lg border border-line bg-surface-raised transition-colors hover:border-accent-line"
            >
                <img
                    src=item.thumb_url.clone()
                    alt=alt
                    class="block aspect-square w-full object-cover"
                    decoding="async"
                />
            </A>
            <p class="m-0 max-w-[320px] truncate text-[0.9375rem] font-semibold text-ink">{shown}</p>
            <div class="flex flex-wrap items-center justify-center gap-2">
                <A href=page.clone() attr:class="btn">{format!("View {}", crate::flavor::get().noun)}</A>
                <ShareButton url=absolute(&page) title=share_title/>
                <button type="button" class="btn-quiet" on:click=move |_| again.run(())>
                    "Upload another"
                </button>
            </div>
        </div>
    }
}

/// Poll a job to its end, writing each step into the page's signals. Shared by
/// the file upload and the link import: past the POST they are the same job.
#[cfg(feature = "hydrate")]
async fn follow(
    job: String,
    set_step: WriteSignal<Option<Step>>,
    set_polls: WriteSignal<u32>,
    set_busy: WriteSignal<bool>,
    set_result: WriteSignal<Option<UploadResult>>,
) {
    // How long the server has been saying the same thing, counted in
    // polls. Two plain locals rather than reading the signals back:
    // the loop is the only writer, so it already knows.
    let mut current = Step::Receiving;
    let mut seen = 0u32;

    // Poll until the job reaches a terminal state. A failed request
    // mid-poll is not terminal -- the server may just have been busy
    // -- so it waits and asks again rather than reporting failure.
    // A job the server has forgotten (a restart drops every job in
    // flight, they live in memory) comes back as Failed, so the
    // browser is told rather than left polling a ghost.
    loop {
        gloo_timers::future::TimeoutFuture::new(POLL_MS).await;

        let fetched = gloo_net::http::Request::get(&format!("/api/upload/status/{job}"))
            .send()
            .await;

        let Ok(resp) = fetched else { continue };
        let Ok(p) = resp.json::<Progress>().await else {
            continue;
        };

        match p {
            Progress::Running { step: s } => {
                if s == current {
                    seen += 1;
                } else {
                    current = s;
                    seen = 0;
                }
                set_step.set(Some(s));
                set_polls.set(seen);
            }
            Progress::Done { item } => {
                set_busy.set(false);
                set_step.set(None);
                set_result.set(Some(UploadResult::Ok { item }));
                return;
            }
            Progress::Rejected { reason } => {
                set_busy.set(false);
                set_step.set(None);
                set_result.set(Some(UploadResult::Rejected { reason }));
                return;
            }
            Progress::Failed { message } => {
                set_busy.set(false);
                set_step.set(None);
                set_result.set(Some(UploadResult::Error { message }));
                return;
            }
        }
    }
}

/// Seek the preview to `t` and wait until that frame is drawable.
///
/// A seek is asynchronous and the canvas draws whatever frame is *current*,
/// so drawing straight after `set_current_time` gives the frame from before
/// the seek -- which, on a clip that has never played, is frame zero: the
/// black or title-card frame the whole exercise exists to avoid. So: a
/// one-shot `seeked` listener, with a timeout behind it in case a browser
/// declines to fire it (a seek to the time it is already at, a stalled
/// decoder), because a poster a beat early beats no poster.
#[cfg(feature = "hydrate")]
async fn seek_and_wait(video: &web_sys::HtmlVideoElement, t: f64) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    // readyState 2 is HAVE_CURRENT_DATA: there is a frame at the playhead.
    if (video.current_time() - t).abs() < 0.02 && video.ready_state() >= 2 {
        return;
    }
    let target = video.clone();
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let done = resolve.clone();
        let cb = Closure::once_into_js(move |_: wasm_bindgen::JsValue| {
            let _ = done.call0(&wasm_bindgen::JsValue::NULL);
        });
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_once(true);
        let _ = target.add_event_listener_with_callback_and_add_event_listener_options(
            "seeked",
            cb.unchecked_ref(),
            &opts,
        );
        let late = resolve.clone();
        set_timeout(
            move || {
                let _ = late.call0(&wasm_bindgen::JsValue::NULL);
            },
            std::time::Duration::from_millis(1500),
        );
    });
    video.set_current_time(t);
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Pick the cover: the liveliest of a few frames spread through the clip,
/// judged by how much its brightness varies. A black frame, a fade, a flat
/// title card all score near zero; a face mid-sentence scores high. The
/// clip is left seeked to the winner, so the preview shows it.
///
/// Small canvas, big effect: 48 pixels across is plenty to tell a frame
/// with something in it from one without, and four seeks plus four tiny
/// draws finish in well under a second on a phone.
#[cfg(feature = "hydrate")]
async fn auto_cover(video: &web_sys::HtmlVideoElement) -> f64 {
    use wasm_bindgen::JsCast;

    let d = video.duration();
    if !d.is_finite() || d <= 0.0 {
        return 0.0;
    }
    let fallback = POSTER_AT_SECONDS.min(d * 0.5);
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return fallback;
    };
    let Ok(Ok(canvas)) = document
        .create_element("canvas")
        .map(|c| c.dyn_into::<web_sys::HtmlCanvasElement>())
    else {
        return fallback;
    };
    let (w, h) = (48u32, 27u32);
    canvas.set_width(w);
    canvas.set_height(h);
    let Ok(Some(ctx)) = canvas.get_context("2d") else {
        return fallback;
    };
    let Ok(ctx) = ctx.dyn_into::<web_sys::CanvasRenderingContext2d>() else {
        return fallback;
    };

    let mut best = (fallback, -1.0_f64);
    for frac in COVER_CANDIDATES {
        let t = (d * frac).clamp(0.0, (d - 0.05).max(0.0));
        seek_and_wait(video, t).await;
        if ctx
            .draw_image_with_html_video_element_and_dw_and_dh(
                video,
                0.0,
                0.0,
                f64::from(w),
                f64::from(h),
            )
            .is_err()
        {
            continue;
        }
        let Ok(data) = ctx.get_image_data(0.0, 0.0, f64::from(w), f64::from(h)) else {
            continue;
        };
        let score = liveliness(&data.data());
        if score > best.1 {
            best = (t, score);
        }
    }
    seek_and_wait(video, best.0).await;
    best.0
}

/// Whether the video's current frame draws as a flat black: the check that
/// keeps an undecoded frame from becoming a cover. Small canvas, one draw.
#[cfg(feature = "hydrate")]
fn frame_is_flat(video: &web_sys::HtmlVideoElement) -> bool {
    use wasm_bindgen::JsCast;
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return false;
    };
    let Ok(Ok(canvas)) = document
        .create_element("canvas")
        .map(|c| c.dyn_into::<web_sys::HtmlCanvasElement>())
    else {
        return false;
    };
    canvas.set_width(32);
    canvas.set_height(18);
    let Ok(Some(ctx)) = canvas.get_context("2d") else {
        return false;
    };
    let Ok(ctx) = ctx.dyn_into::<web_sys::CanvasRenderingContext2d>() else {
        return false;
    };
    if ctx
        .draw_image_with_html_video_element_and_dw_and_dh(video, 0.0, 0.0, 32.0, 18.0)
        .is_err()
    {
        return false;
    }
    let Ok(data) = ctx.get_image_data(0.0, 0.0, 32.0, 18.0) else {
        return false;
    };
    let px = data.data();
    let n = (px.len() / 4).max(1) as f64;
    let mean = px
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0)
        .sum::<f64>()
        / n;
    mean < 12.0 && liveliness(&px) < 6.0
}

/// Contrast of an RGBA buffer: the standard deviation of luma, halved for a
/// frame that is dark overall so a black frame with one bright pixel does
/// not win. Pure, so it is testable without a browser.
#[cfg(any(feature = "hydrate", test))]
fn liveliness(rgba: &[u8]) -> f64 {
    let n = rgba.len() / 4;
    if n == 0 {
        return 0.0;
    }
    let lumas =
        rgba.as_chunks::<4>().0.iter().map(|p| {
            0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
        });
    let mean = lumas.clone().sum::<f64>() / n as f64;
    let var = lumas.map(|l| (l - mean) * (l - mean)).sum::<f64>() / n as f64;
    let sd = var.sqrt();
    if mean < 28.0 {
        sd * 0.5
    } else {
        sd
    }
}

/// Draw the frame at `at` to a canvas and hand back a JPEG.
///
/// `None` for any failure -- no 2D context, a codec the canvas cannot read,
/// `toBlob` refusing -- and the server paints a placeholder. Never an error to
/// the visitor: the clip is the upload, the poster is a courtesy.
#[cfg(feature = "hydrate")]
async fn capture_poster(video: &web_sys::HtmlVideoElement, at: f64) -> Option<web_sys::Blob> {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let (vw, vh) = (
        f64::from(video.video_width()),
        f64::from(video.video_height()),
    );
    if vw < 1.0 || vh < 1.0 {
        return None;
    }
    seek_and_wait(video, at).await;

    // A frame the phone has not decoded draws as solid black. Send nothing
    // rather than that: the server then draws its own frame from the file.
    if frame_is_flat(video) {
        return None;
    }

    let scale = (POSTER_MAX_EDGE / vw.max(vh)).min(1.0);
    let (cw, ch) = ((vw * scale).round(), (vh * scale).round());

    let document = web_sys::window()?.document()?;
    let canvas: web_sys::HtmlCanvasElement =
        document.create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(cw as u32);
    canvas.set_height(ch as u32);
    let ctx: web_sys::CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    ctx.draw_image_with_html_video_element_and_dw_and_dh(video, 0.0, 0.0, cw, ch)
        .ok()?;

    // `toBlob` is callback-based; a Promise is made of it by hand. Quality
    // high: the server re-encodes at its own setting, and a second
    // generation of loss on top of a soft first one is what made posters
    // look worse than the clip they came from.
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let cb = Closure::once_into_js(move |blob: wasm_bindgen::JsValue| {
            let _ = resolve.call1(&wasm_bindgen::JsValue::NULL, &blob);
        });
        let _ = canvas.to_blob_with_type_and_encoder_options(
            cb.unchecked_ref(),
            "image/jpeg",
            &wasm_bindgen::JsValue::from_f64(0.94),
        );
    });
    let value = wasm_bindgen_futures::JsFuture::from(promise).await.ok()?;
    value.dyn_into::<web_sys::Blob>().ok()
}

/// The progress bar's honesty properties, asserted rather than believed.
///
/// They are all one edit away from being silently void -- reordering the
/// pipeline in `upload_route.rs`, shaving a tau, nudging a ceiling to 1.0 -- and
/// none of them fail loudly. They fail as a bar that goes backwards, or sticks,
/// or sits full while the visitor waits, which is exactly the class of thing
/// nobody notices until it is in production.
#[cfg(test)]
mod tests {
    use super::*;

    /// A flat frame scores nothing, a dark frame is handicapped, and a frame
    /// with real contrast beats both -- which is the whole basis of the
    /// cover pick.
    #[test]
    fn liveliness_prefers_contrast_and_light() {
        let flat: Vec<u8> = [128, 128, 128, 255].repeat(64);
        let black: Vec<u8> = [0, 0, 0, 255].repeat(64);
        let mut dark_spark = black.clone();
        dark_spark[..4].copy_from_slice(&[255, 255, 255, 255]);
        let mut checker = Vec::new();
        for i in 0..64 {
            checker.extend(if i % 2 == 0 {
                [20, 20, 20, 255]
            } else {
                [230, 230, 230, 255]
            });
        }
        assert_eq!(liveliness(&flat), 0.0);
        assert_eq!(liveliness(&black), 0.0);
        assert!(liveliness(&checker) > liveliness(&dark_spark));
        assert_eq!(liveliness(&[]), 0.0);
    }

    #[test]
    fn bands_tile_the_bar_without_gaps_or_overlap() {
        let mut floor = 0.0;
        for step in Step::ALL {
            let (start, ceiling, tau) = band(step);
            assert_eq!(start, floor, "{step:?} does not start where the last ended");
            assert!(ceiling > start, "{step:?} has no room to move");
            assert!(tau > 0.0, "{step:?} would divide by zero at the first poll");
            floor = ceiling;
        }
        // The last ceiling is approached, never reached. A bar that fills and
        // then waits is the classic lie, and `Done` is the only thing allowed
        // to end this.
        assert!(
            floor < 1.0,
            "the pipeline can reach 100% before it finishes"
        );
    }

    #[test]
    fn the_fill_always_moves_and_never_leaves_its_band() {
        for step in Step::ALL {
            let (start, ceiling, tau) = band(step);
            let at = |t: u32| start + (ceiling - start) * f64::from(t) / (f64::from(t) + tau);
            assert_eq!(at(0), start);
            // Strictly increasing for as long as any real job could run --
            // 10,000 polls is over two hours at 800ms, well past the point the
            // server would have given up.
            for t in 1..10_000u32 {
                assert!(at(t) > at(t - 1), "{step:?} stalled at poll {t}");
                assert!(at(t) < ceiling, "{step:?} overran its band at poll {t}");
            }
        }
    }

    #[test]
    fn every_step_reports_one_of_three_words() {
        for step in Step::ALL {
            assert!(
                ["Uploading", "Working", "Saving"].contains(&phase(step, false)),
                "{step:?} invented a fourth word"
            );
        }
        // An import's first phase is the fetch; nothing else changes.
        assert_eq!(phase(Step::Receiving, true), "Fetching");
        assert_eq!(phase(Step::Storing, true), phase(Step::Storing, false));
    }
}

/// The few calls this page makes into Turnstile's script, kept together so
/// the `js_sys::Reflect` plumbing stays out of the component. The script is
/// loaded with `render=explicit`: it defines `window.turnstile` and does
/// nothing else until `render` is called with an element and a site key.
#[cfg(feature = "hydrate")]
mod turnstile {
    use leptos::prelude::*;
    use leptos::wasm_bindgen::prelude::*;
    use leptos::wasm_bindgen::JsCast;
    use leptos::web_sys;

    const SCRIPT: &str = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";

    fn global() -> Option<JsValue> {
        let window = web_sys::window()?;
        let t = js_sys::Reflect::get(&window, &JsValue::from_str("turnstile")).ok()?;
        (!t.is_undefined() && !t.is_null()).then_some(t)
    }

    fn method(target: &JsValue, name: &str) -> Option<js_sys::Function> {
        js_sys::Reflect::get(target, &JsValue::from_str(name))
            .ok()?
            .dyn_into::<js_sys::Function>()
            .ok()
    }

    /// Append the script once. A second call finds the first tag and leaves it.
    pub fn load_script() {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if document
            .query_selector("script[data-turnstile]")
            .ok()
            .flatten()
            .is_some()
        {
            return;
        }
        let Ok(tag) = document.create_element("script") else {
            return;
        };
        let _ = tag.set_attribute("src", SCRIPT);
        let _ = tag.set_attribute("async", "");
        let _ = tag.set_attribute("defer", "");
        let _ = tag.set_attribute("data-turnstile", "");
        if let Some(head) = document.head() {
            let _ = head.append_child(&tag);
        }
    }

    pub fn ready() -> bool {
        global().is_some_and(|t| method(&t, "render").is_some())
    }

    /// Render the widget into `el`. Every outcome the widget can report --
    /// a token, an expired token, an error -- lands in `token`, so the
    /// component has one signal to read: `Some` means "a fresh answer is
    /// ready to send", `None` means "not yet, or not any more".
    pub fn render(
        el: &web_sys::Element,
        site_key: &str,
        token: WriteSignal<Option<String>>,
    ) -> Option<String> {
        let t = global()?;
        let render = method(&t, "render")?;
        let opts = js_sys::Object::new();
        let set = |k: &str, v: &JsValue| {
            let _ = js_sys::Reflect::set(&opts, &JsValue::from_str(k), v);
        };
        set("sitekey", &JsValue::from_str(site_key));
        set("theme", &JsValue::from_str("dark"));
        set("size", &JsValue::from_str("flexible"));
        set("action", &JsValue::from_str("upload"));

        let got = Closure::<dyn Fn(JsValue)>::new(move |v: JsValue| token.set(v.as_string()));
        set("callback", got.as_ref().unchecked_ref());
        got.forget();
        for name in ["expired-callback", "error-callback", "timeout-callback"] {
            let lost = Closure::<dyn Fn(JsValue)>::new(move |_: JsValue| token.set(None));
            set(name, lost.as_ref().unchecked_ref());
            lost.forget();
        }

        render.call2(&t, el, &opts).ok()?.as_string()
    }

    pub fn reset(widget_id: &str) {
        if let Some(t) = global() {
            if let Some(reset) = method(&t, "reset") {
                let _ = reset.call1(&t, &JsValue::from_str(widget_id));
            }
        }
    }
}
