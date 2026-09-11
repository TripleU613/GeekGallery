use serde::{Deserialize, Serialize};

/// What the stored original is. Decides how a item is drawn (an `<img>` or a
/// `<video>`), what the download is called, which OG tags a page emits and what
/// the sitemap says about it.
///
/// Shared with the wasm bundle, so no ssr-only type may appear here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// A still image. Re-encoded to PNG on the way in.
    #[default]
    Image,
    /// An animated GIF, stored byte-for-byte so it keeps animating.
    Gif,
    /// mp4 or webm, stored as uploaded.
    Video,
}

impl MediaKind {
    /// The database representation. Kept as a plain string column rather than
    /// an enforced CHECK, so a new kind is a code change and not a migration.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Gif => "gif",
            MediaKind::Video => "video",
        }
    }

    /// Anything unrecognised is an image: every row written before the column
    /// existed is one, and drawing an unknown kind as an `<img>` fails soft.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "gif" => MediaKind::Gif,
            "video" => MediaKind::Video,
            _ => MediaKind::Image,
        }
    }

    /// Whether this plays rather than sits still. GIFs count: the card badge
    /// and the "clips" framing on the site are about motion, not codecs.
    pub fn is_animated(self) -> bool {
        !matches!(self, MediaKind::Image)
    }

    /// The label a card badge shows, or nothing for a still image.
    pub fn badge(self) -> Option<&'static str> {
        match self {
            MediaKind::Image => None,
            MediaKind::Gif => Some("GIF"),
            MediaKind::Video => Some("VIDEO"),
        }
    }
}

/// One item: a meme, a GIF or a clip. Shared verbatim between the server and the
/// wasm bundle, so this struct must not name any ssr-only type (no chrono, no
/// reqwest) — timestamps travel as RFC3339 strings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    /// URL-safe form of the title, unique across items. What `/item/:slug`
    /// matches on; falls back to the id for a item whose title had nothing
    /// sluggable in it. Always prefer this over `id` when building a link.
    pub slug: String,
    pub title: String,
    pub kind: MediaKind,
    /// Public URLs, not filesystem paths. `orig_url` is the media file itself
    /// -- a PNG, a GIF, an mp4 or a webm, by `kind`.
    pub orig_url: String,
    /// Always a JPEG, always a still, whatever the original is.
    pub thumb_url: String,
    /// A full-size still for a video, for `og:image` and `<video poster>`.
    /// `None` for anything that is already a picture.
    pub poster_url: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Seconds, videos only.
    pub duration: Option<f64>,
    /// Where a link-import came from (the tweet, the reel, the watch page).
    /// `None` for a file upload. Defaulted so a row from before the column
    /// still deserialises.
    #[serde(default)]
    pub source_url: Option<String>,
    pub created_at: String,
    pub is_public: bool,
    pub reports: i64,
    pub likes: i64,
    /// Whether the current visitor has already liked this one. Populated per
    /// request from their session; not a property of the item.
    pub liked_by_me: bool,
    /// `None` for an anonymous upload — still the common case, since logging
    /// in is additive, not required.
    pub uploader: Option<Uploader>,
}

impl Item {
    /// Whether the title is the one `upload_route::clean_title` invents for a
    /// item posted without one. Those stay stored -- `<title>`, `og:title`, the
    /// sitemap and a download's filename all need *some* words -- but the
    /// gallery and the item page render them as a quiet label rather than a
    /// headline, because a wall of bold "untitled item" captions was the
    /// loudest thing on the site.
    pub fn is_untitled(&self) -> bool {
        let t = self.title.trim().to_ascii_lowercase();
        matches!(
            t.as_str(),
            "" | "untitled" | "untitled clip" | "untitled gif"
        ) || t == format!("untitled {}", crate::flavor::get().noun)
    }

    /// The picture that stands for this item wherever a raster is required:
    /// link previews, the sitemap, the embed card's `<img>` fallback.
    pub fn still_url(&self) -> &str {
        self.poster_url.as_deref().unwrap_or(&self.orig_url)
    }

    /// The MIME type of the original, as stored.
    ///
    /// The container is recorded in the object key's extension and read back
    /// off the URL here rather than stored in a fourth column: a still is PNG
    /// unless `storage` kept it as the JPEG it arrived as, and a video keeps
    /// the container it arrived in.
    pub fn mime(&self) -> &'static str {
        match self.kind {
            MediaKind::Image => {
                if self.orig_url.ends_with(".jpg") {
                    "image/jpeg"
                } else {
                    "image/png"
                }
            }
            MediaKind::Gif => "image/gif",
            MediaKind::Video => {
                if self.orig_url.ends_with(".webm") {
                    "video/webm"
                } else {
                    "video/mp4"
                }
            }
        }
    }

    /// The file extension the download should carry.
    pub fn extension(&self) -> &'static str {
        match self.mime() {
            "image/gif" => "gif",
            "image/jpeg" => "jpg",
            "video/webm" => "webm",
            "video/mp4" => "mp4",
            _ => "png",
        }
    }
}

/// Where an upload is. Ordered as the pipeline actually runs.
// Hash so the upload page can key a `<For>` over `Step::ALL` by the step
// itself, which is already a unique, stable identity -- no index needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Bytes arriving from the browser.
    Receiving,
    /// Sniff the container, decode what can be decoded, SHA-256 it, then the
    /// duplicate lookup.
    Fingerprinting,
    /// Bound the image, build the thumbnail (and the poster, for a video).
    Cropping,
    /// Invisible provenance watermark, re-encode, upload to R2.
    Storing,
}

impl Step {
    /// Shown verbatim in the UI. Present tense, because it names what is
    /// happening right now rather than a stage in an abstract pipeline.
    pub fn label(self) -> &'static str {
        match self {
            Step::Receiving => "Uploading",
            Step::Fingerprinting => "Fingerprinting",
            Step::Cropping => "Preparing",
            Step::Storing => "Saving",
        }
    }

    /// Every step, in order, so the UI can draw the whole list up front and
    /// light each one as it completes instead of having rows appear one by one.
    pub const ALL: [Step; 4] = [
        Step::Receiving,
        Step::Fingerprinting,
        Step::Cropping,
        Step::Storing,
    ];
}

/// A job's public state, exactly as the browser polls it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Progress {
    /// Still going. `step` is the one currently running.
    Running { step: Step },
    /// Finished. The item is published.
    Done { item: Box<Item> },
    /// Refused, with a line fit to show a visitor.
    Rejected { reason: String },
    /// Broke. Also what a client gets for an id that no longer exists.
    Failed { message: String },
}

/// A page of items plus the cursor needed to ask for the next one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemPage {
    pub items: Vec<Item>,
    /// `created_at` of the last row, or `None` when the gallery is exhausted.
    pub next_cursor: Option<String>,
}

pub const PAGE_SIZE: i64 = 24;

/// Gallery ordering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sort {
    #[default]
    Newest,
    MostLiked,
    /// A-Z by title.
    Az,
    /// Videos and GIFs only, newest first. Not a separate route: it is the same
    /// keyset pagination with one more WHERE clause, and the menu is where a
    /// visitor already goes to change what the grid shows.
    Clips,
}

impl Sort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::MostLiked => "liked",
            Sort::Az => "az",
            Sort::Clips => "clips",
        }
    }

    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "liked" => Sort::MostLiked,
            "az" => Sort::Az,
            "clips" => Sort::Clips,
            _ => Sort::Newest,
        }
    }
}

/// What a visitor is allowed to see of `/admin`.
///
/// Three states rather than a `Result`, for the same reason `LikeOutcome` has
/// `SignInRequired`: "not signed in" and "signed in but not an admin" need different
/// pages -- one offers a sign-in link, the other says plainly that this account does
/// not have access -- and folded into a `ServerFnError` they could only be told apart
/// by matching on message text.
///
/// Adjacently tagged (`tag` + `content`), not internally tagged. serde cannot
/// internally-tag a newtype variant that holds a sequence -- it needs somewhere to
/// put the `state` key, and a JSON array has no keys -- and it does not say so
/// until it serializes. `tag = "state"` alone compiled, server-rendered correctly,
/// and then panicked inside `leptos_server`'s resource serializer while preparing
/// the value for hydration. Verified by `admin_queue_round_trips` below rather
/// than by reading serde's docs again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "queue", rename_all = "snake_case")]
pub enum AdminQueue {
    /// Nobody is signed in.
    SignInRequired,
    /// Signed in, but this account is not an admin.
    Denied,
    Queue(Vec<FlaggedItem>),
}

/// A signed-in user, as far as the UI is concerned.
///
/// Deliberately thin: no email, no Google subject id. Those have no reason to
/// reach the client, and keeping them out of this struct means they can never
/// leak through a server function response by accident.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub is_admin: bool,
}

/// The uploader shown on a card/detail page. Thin on purpose, same reasoning
/// as `User`: no id, no email, nothing beyond what's actually displayed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Uploader {
    pub display_name: String,
    pub avatar_url: Option<String>,
}

/// One row of `/leaderboard`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeaderboardEntry {
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub upload_count: i64,
}

/// Why a item was reported. A plain string on the wire (see `reports.reason`'s
/// comment for why it isn't an enforced DB constraint), but typed here so the
/// UI can't submit something the report queue doesn't know how to label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportReason {
    OffTopic,
    Spam,
    Porn,
    Stolen,
    /// Harassment, threats, gore, anything that is not a meme but an attack.
    Harmful,
}

impl ReportReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReportReason::OffTopic => "off_topic",
            ReportReason::Spam => "spam",
            ReportReason::Porn => "porn",
            ReportReason::Stolen => "stolen",
            ReportReason::Harmful => "harmful",
        }
    }

    pub fn label(&self) -> String {
        match self {
            ReportReason::OffTopic => format!("not {} / off topic", crate::flavor::get().a_noun()),
            ReportReason::Spam => "spam".into(),
            ReportReason::Porn => "porn / NSFW".into(),
            ReportReason::Stolen => "stolen / copyright (DMCA)".into(),
            ReportReason::Harmful => "harassment, threats or gore".into(),
        }
    }

    pub fn all() -> [ReportReason; 5] {
        [
            ReportReason::OffTopic,
            ReportReason::Spam,
            ReportReason::Porn,
            ReportReason::Stolen,
            ReportReason::Harmful,
        ]
    }

    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "spam" => ReportReason::Spam,
            "porn" => ReportReason::Porn,
            "stolen" => ReportReason::Stolen,
            "harmful" => ReportReason::Harmful,
            _ => ReportReason::OffTopic,
        }
    }
}

/// One report against a item, as shown in the admin queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportDetail {
    pub reason: String,
    pub message: Option<String>,
    pub created_at: String,
}

/// A item with at least one report, plus every report against it — the admin
/// queue's unit of review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlaggedItem {
    pub item: Item,
    pub reports: Vec<ReportDetail>,
}

/// Reply from `POST /api/upload`.
///
/// Lives here rather than beside the handler so the wasm side deserializes the
/// exact type the server serializes — two hand-matched shapes would drift.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UploadResult {
    /// Accepted for processing. The response carries a job id to poll rather
    /// than the finished item: a 60MB video takes a while to hash and push to
    /// R2, and a form that holds the connection for that looks frozen.
    Queued {
        job: String,
    },
    /// Boxed, because this variant is several hundred bytes and the others are
    /// a `String`; clippy's `large_enum_variant` is right that every `Queued`
    /// would otherwise carry the empty space. serde sees through the `Box`.
    Ok {
        item: Box<Item>,
    },
    /// Refused before anything was stored: an exact duplicate, or a file that
    /// is not one of the accepted formats.
    Rejected {
        reason: String,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Item {
        Item {
            id: "id".into(),
            slug: "slug".into(),
            title: "a item".into(),
            kind: MediaKind::Video,
            orig_url: "https://example.invalid/v.mp4".into(),
            thumb_url: "https://example.invalid/t.jpg".into(),
            poster_url: Some("https://example.invalid/p.jpg".into()),
            width: 1280,
            height: 720,
            duration: Some(12.5),
            source_url: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            is_public: false,
            reports: 3,
            likes: 0,
            liked_by_me: false,
            uploader: None,
        }
    }

    /// The invented titles read as untitled; anything a person typed does not,
    /// including a title that merely starts with the word.
    #[test]
    fn invented_titles_are_untitled_and_chosen_ones_are_not() {
        let with = |t: &str| {
            let mut k = sample();
            k.title = t.into();
            k.is_untitled()
        };
        for t in [
            "",
            "  ",
            "untitled",
            "untitled item",
            "Untitled Clip",
            "untitled gif",
        ] {
            assert!(with(t), "{t:?} should read as untitled");
        }
        for t in [
            "an item",
            "Untitled Masterpiece",
            "item untitled",
            "untitled thing",
        ] {
            assert!(!with(t), "{t:?} is a chosen title");
        }
    }

    /// Every variant has to survive a round trip through JSON, because that is
    /// what a server fn does with it and a failure there is not a compile error.
    #[test]
    fn admin_queue_round_trips() {
        let flagged = FlaggedItem {
            item: sample(),
            reports: vec![ReportDetail {
                reason: "off_topic".into(),
                message: Some("not a item".into()),
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        };
        for value in [
            AdminQueue::SignInRequired,
            AdminQueue::Denied,
            AdminQueue::Queue(vec![]),
            AdminQueue::Queue(vec![flagged]),
        ] {
            let json = serde_json::to_string(&value).expect("serializes");
            let back: AdminQueue = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(value, back, "round trip changed the value: {json}");
        }
    }

    /// A row written before `kind` existed deserializes as an image, and the
    /// wire form of each kind is stable -- it is what the public API emits.
    #[test]
    fn media_kind_wire_format_is_stable() {
        for (k, s) in [
            (MediaKind::Image, "image"),
            (MediaKind::Gif, "gif"),
            (MediaKind::Video, "video"),
        ] {
            assert_eq!(k.as_str(), s);
            assert_eq!(MediaKind::from_str_or_default(s), k);
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{s}\""));
        }
        assert_eq!(MediaKind::from_str_or_default("hologram"), MediaKind::Image);
    }

    /// The MIME type and extension follow the stored URL for video, since two
    /// containers share one `kind`.
    #[test]
    fn mime_and_extension_follow_the_container() {
        let mut k = sample();
        assert_eq!(k.mime(), "video/mp4");
        assert_eq!(k.extension(), "mp4");
        k.orig_url = "https://example.invalid/v.webm".into();
        assert_eq!(k.mime(), "video/webm");
        assert_eq!(k.extension(), "webm");
        k.kind = MediaKind::Gif;
        assert_eq!(k.mime(), "image/gif");
        k.kind = MediaKind::Image;
        k.orig_url = "https://example.invalid/i.png".into();
        assert_eq!(k.mime(), "image/png");
        assert_eq!(k.extension(), "png");
        k.orig_url = "https://example.invalid/i.jpg".into();
        assert_eq!(k.mime(), "image/jpeg");
        assert_eq!(k.extension(), "jpg");
    }

    /// The still stands in for the video everywhere a raster is needed, and is
    /// the original itself for anything that already is one.
    #[test]
    fn still_url_prefers_the_poster() {
        let mut k = sample();
        assert_eq!(k.still_url(), "https://example.invalid/p.jpg");
        k.poster_url = None;
        assert_eq!(k.still_url(), "https://example.invalid/v.mp4");
    }
}
