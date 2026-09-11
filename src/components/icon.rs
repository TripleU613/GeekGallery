//! Lucide line icons, wrapped so call sites don't repeat sizing boilerplate.
//!
//! `leptos_icons` renders `icondata` SVG data as a real `<svg>` element in
//! Rust -- no JavaScript icon package, and no Unicode/emoji standing in for an
//! icon. Only the Lucide set is pulled in (`icondata_lu`), and unused icon
//! consts are dead-code-eliminated, so the wasm only carries what's referenced.
//!
//! `currentColor` is inherited by default, so an icon takes the colour of
//! whatever control contains it -- which is what makes a single `.icon-btn`
//! style work for every icon button.

use leptos::prelude::*;
use leptos_icons::Icon;

/// Re-exported so call sites say `icon::LuHeart` rather than reaching for
/// `icondata_lu` directly and coupling every component to the icon set.
pub use icondata_lu::{
    LuArrowLeft, LuCheck, LuChevronDown, LuCircleAlert, LuCirclePlus, LuClapperboard,
    LuCloudUpload, LuCodeXml, LuDownload, LuDroplet, LuEllipsis, LuFlag, LuFlame,
    LuGalleryThumbnails, LuGithub, LuGrid2x2, LuHeart, LuImage, LuLaugh, LuLayoutGrid, LuLink,
    LuLoaderCircle, LuLogOut, LuMaximize, LuMenu, LuMic, LuMinimize, LuPause, LuPlay, LuRefreshCw,
    LuRotateCcw, LuRotateCw, LuSearch, LuShare2, LuStar, LuThumbsUp, LuTrash2, LuTrophy,
    LuUserRound, LuVolume2, LuVolumeX, LuX, LuZap,
};

/// The like button's glyph for a flavor. The only place the closed set in
/// `flavor::LikeIcon` meets real icon data, so adding a glyph is one arm here
/// and one variant there.
pub fn like_icon(which: crate::flavor::LikeIcon) -> icondata_core::Icon {
    use crate::flavor::LikeIcon::*;
    match which {
        Heart => LuHeart,
        Mic => LuMic,
        Droplet => LuDroplet,
        Star => LuStar,
        ThumbsUp => LuThumbsUp,
        Flame => LuFlame,
        Zap => LuZap,
        Laugh => LuLaugh,
    }
}

/// Default icon size in px. The plan's 16-22px band; 18 reads correctly next
/// to 13-14px navigation text and 14-16px control text.
pub const SIZE: u32 = 18;

/// An icon at a given pixel size, inheriting `currentColor`.
///
/// `aria-hidden` is unconditional: an icon here is always either decorative
/// beside a text label, or inside a control that carries its own `aria-label`.
/// Announcing the glyph too would just duplicate the accessible name.
#[component]
pub fn Ico(
    icon: icondata_core::Icon,
    #[prop(optional)] size: Option<u32>,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let px = format!("{}px", size.unwrap_or(SIZE));
    view! {
        <span class=move || format!("ico {}", class.clone().unwrap_or_default()) aria-hidden="true">
            <Icon icon=icon width=px.clone() height=px/>
        </span>
    }
}
