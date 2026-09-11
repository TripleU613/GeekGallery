//! The flavor: everything that makes one deployment of this codebase *a site*
//! rather than *the* site.
//!
//! One image serves any number of galleries. What differs between them -- the
//! name, what one item is called, the colours, the like button's verb and
//! glyph, the logo, the legal contact, the sister sites in the footer -- is
//! read from the environment at startup on the server, serialised into the
//! HTML as a JSON blob, and read back by the wasm bundle before it hydrates.
//! Both sides therefore render from the identical value, which is the one
//! property hydration cannot do without (see CLAUDE.md: nothing that differs
//! between server and client may seed initial state).
//!
//! Every field has a default, so a bare `cargo leptos watch` with no
//! `SITE_*` variables set runs as a plain grey gallery called "geekgallery".
//! `FLAVORS.md` lists every variable with its default; the two functions that
//! read them (`Flavor::from_env` and `Theme::from_env`) are the source of
//! truth that file is checked against.
//!
//! Nothing in here is a secret. The whole struct is public in every page.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The glyph on the like button. A closed set rather than an arbitrary icon
/// name: the wasm bundle only carries the icons it references, so a name
/// that is not in this list could not be drawn anyway.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LikeIcon {
    #[default]
    Heart,
    Mic,
    Droplet,
    Star,
    ThumbsUp,
    Flame,
    Zap,
    Laugh,
}

impl LikeIcon {
    /// The `SITE_LIKE_ICON` spelling. Unknown values fall back to the heart
    /// rather than refusing to start: a typo in a colour or an icon name is
    /// not worth an outage.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "mic" | "microphone" => Self::Mic,
            "droplet" | "drop" | "tear" => Self::Droplet,
            "star" => Self::Star,
            "thumbs_up" | "thumbsup" | "thumb" => Self::ThumbsUp,
            "flame" | "fire" => Self::Flame,
            "zap" | "bolt" | "lightning" => Self::Zap,
            "laugh" | "lol" => Self::Laugh,
            _ => Self::Heart,
        }
    }
}

/// The like button's words. Each may use `{noun}` and `{nouns}`, filled by
/// [`Flavor::fill`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Like {
    pub icon: LikeIcon,
    /// The action, unliked state: "Give this {noun} a mic".
    pub verb: String,
    /// The action, liked state: "Take the mic back".
    pub undo: String,
    /// The gallery sort chip: "Most mics".
    pub sort_label: String,
}

impl Default for Like {
    fn default() -> Self {
        Self {
            icon: LikeIcon::Heart,
            verb: "Like this {noun}".into(),
            undo: "Unlike".into(),
            sort_label: "Most liked".into(),
        }
    }
}

/// The palette, as `#rrggbb` (or `#rgb`) strings. These become CSS custom
/// properties in the document `<head>` (see [`Theme::css`]) and the Tailwind
/// utilities read those, so `bg-surface` is the same class in every flavor
/// and only the variable behind it changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub bg: String,
    pub surface: String,
    pub surface_raised: String,
    pub surface_hover: String,
    pub line: String,
    pub line_strong: String,
    pub ink: String,
    pub ink_2: String,
    pub ink_3: String,
    pub accent: String,
    pub accent_hover: String,
    pub accent_active: String,
    pub accent_muted: String,
    /// Text on a filled accent button.
    pub accent_ink: String,
    pub danger: String,
    pub ok: String,
}

impl Default for Theme {
    /// Cool near-black neutrals with a periwinkle accent -- the palette the
    /// codebase was tuned on, every token measured against the 4.5:1 AA floor.
    fn default() -> Self {
        Self {
            bg: "#0a0b0f".into(),
            surface: "#10121a".into(),
            surface_raised: "#171a25".into(),
            surface_hover: "#1e2230".into(),
            line: "#2b3042".into(),
            line_strong: "#3d4358".into(),
            ink: "#f2f4f8".into(),
            ink_2: "#aab0c0".into(),
            ink_3: "#7f8698".into(),
            accent: "#9aa4ff".into(),
            accent_hover: "#b0b8ff".into(),
            accent_active: "#7a84f5".into(),
            accent_muted: "#8089d6".into(),
            accent_ink: "#0a0b0f".into(),
            danger: "#ff6b7a".into(),
            ok: "#4ee3a0".into(),
        }
    }
}

impl Theme {
    /// The palette from `THEME_*`, each token defaulting to the built-in.
    ///
    /// Only the accent and the background usually need setting: the three
    /// accent variants are derived from the accent when absent (lighter,
    /// darker, dimmer), so a flavor can be one line of colour and still get
    /// hover and pressed states that belong to it.
    #[cfg(feature = "ssr")]
    pub fn from_env() -> Self {
        let d = Self::default();
        let get = |k: &str, fallback: &str| {
            env_non_empty(k)
                .filter(|v| parse_hex(v).is_some())
                .unwrap_or_else(|| fallback.to_string())
        };
        let accent = get("THEME_ACCENT", &d.accent);
        let derived = |k: &str, fallback: &str, f: fn([u8; 3]) -> [u8; 3]| {
            if let Some(v) = env_non_empty(k).filter(|v| parse_hex(v).is_some()) {
                v
            } else if accent == d.accent {
                fallback.to_string()
            } else {
                hex(f(parse_hex(&accent).unwrap_or([0; 3])))
            }
        };
        Self {
            bg: get("THEME_BG", &d.bg),
            surface: get("THEME_SURFACE", &d.surface),
            surface_raised: get("THEME_SURFACE_RAISED", &d.surface_raised),
            surface_hover: get("THEME_SURFACE_HOVER", &d.surface_hover),
            line: get("THEME_LINE", &d.line),
            line_strong: get("THEME_LINE_STRONG", &d.line_strong),
            ink: get("THEME_INK", &d.ink),
            ink_2: get("THEME_INK_2", &d.ink_2),
            ink_3: get("THEME_INK_3", &d.ink_3),
            accent_hover: derived("THEME_ACCENT_HOVER", &d.accent_hover, |c| {
                mix(c, [255; 3], 0.18)
            }),
            accent_active: derived("THEME_ACCENT_ACTIVE", &d.accent_active, |c| {
                mix(c, [0; 3], 0.14)
            }),
            accent_muted: derived("THEME_ACCENT_MUTED", &d.accent_muted, |c| {
                mix(c, [0; 3], 0.20)
            }),
            accent_ink: get("THEME_ACCENT_INK", &d.accent_ink),
            accent,
            danger: get("THEME_DANGER", &d.danger),
            ok: get("THEME_OK", &d.ok),
        }
    }

    /// The custom properties Tailwind's utilities read, as one `:root` rule.
    ///
    /// Channels, not colours (`10 11 15` rather than `#0a0b0f`), because the
    /// utilities are written `rgb(var(--c-bg) / <alpha>)` so that `bg-bg/50`
    /// and friends keep working. A token that is not a parseable hex is
    /// skipped rather than emitted broken, and the Tailwind config carries the
    /// default palette as each variable's fallback.
    pub fn css(&self) -> String {
        let mut out = String::from(":root{");
        for (name, value) in self.tokens() {
            if let Some([r, g, b]) = parse_hex(value) {
                out.push_str(&format!("--c-{name}:{r} {g} {b};"));
            }
        }
        out.push('}');
        out
    }

    /// Every token with its CSS variable name.
    pub fn tokens(&self) -> [(&'static str, &str); 16] {
        [
            ("bg", &self.bg),
            ("surface", &self.surface),
            ("surface-raised", &self.surface_raised),
            ("surface-hover", &self.surface_hover),
            ("line", &self.line),
            ("line-strong", &self.line_strong),
            ("ink", &self.ink),
            ("ink-2", &self.ink_2),
            ("ink-3", &self.ink_3),
            ("accent", &self.accent),
            ("accent-hover", &self.accent_hover),
            ("accent-active", &self.accent_active),
            ("accent-muted", &self.accent_muted),
            ("accent-ink", &self.accent_ink),
            ("danger", &self.danger),
            ("ok", &self.ok),
        ]
    }
}

/// `#rgb` or `#rrggbb` to channels. Anything else is `None`.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(s, 16).ok()?;
    match s.len() {
        6 => Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]),
        3 => {
            let c = |n: u32| ((n & 0xf) * 17) as u8;
            Some([c(v >> 8), c(v >> 4), c(v)])
        }
        _ => None,
    }
}

#[cfg(feature = "ssr")]
fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `a` moved `t` of the way towards `b`, per channel.
#[cfg(feature = "ssr")]
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// A footer link to another gallery run by the same people.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SisterSite {
    pub name: String,
    pub url: String,
}

/// One deployment's identity. See the module docs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flavor {
    /// The site's name as written everywhere: `<title>`, `og:site_name`, the
    /// logo's alt text, the embed card's brand line. Case is kept as given.
    pub name: String,
    /// What one item is called, lower case: "pic", "meme", "frog".
    pub noun: String,
    /// Its plural.
    pub nouns: String,
    /// The subject the items are of, for copy like "a {subject} clip" and
    /// "memes of {subject}". Empty means the copy drops that phrase.
    pub subject: String,
    /// One line under the name: the about page's subtitle, the `llms.txt`
    /// blockquote's opening.
    pub tagline: String,
    /// The home page's `<meta name="description">`, and the manifest's.
    pub description: String,
    /// Paragraphs for the top of the about page, before the generic sections
    /// on what belongs, uploads and moderation.
    pub about: Vec<String>,
    /// Other spellings people search for, for the site's JSON-LD
    /// `alternateName`.
    pub alternate_names: Vec<String>,
    /// `https://example.com`, no trailing slash. Every absolute URL the
    /// site emits starts with this.
    pub origin: String,
    /// The DMCA agent.
    pub contact_email: String,
    /// Where the code lives, for the header's GitHub button. Empty hides it.
    pub repo_url: String,
    pub sister_sites: Vec<SisterSite>,
    pub like: Like,
    pub theme: Theme,
    /// Where the brand art is served from, no trailing slash: `logo.png`,
    /// `logo-large.png`, `favicon-32.png`, `favicon-192.png`,
    /// `favicon-512.png` and `apple-touch-icon.png` are appended to it. The
    /// default `/brand` is this repo's own placeholder art under `public/`;
    /// a real flavor points it at a folder on its media bucket.
    pub assets_base: String,
    /// A cache-busting suffix on the asset URLs, bumped when the art changes
    /// under the same names.
    pub assets_version: String,
    /// The header logo's intrinsic size, so the bar does not reflow while it
    /// loads.
    pub logo_width: u32,
    pub logo_height: u32,
    /// Whether `logo.png` is a wordmark that already spells the name. When it
    /// is not (the default: a square mark), the header prints the name beside
    /// it in text.
    pub logo_has_name: bool,
}

impl Default for Flavor {
    fn default() -> Self {
        Self {
            name: "geekgallery".into(),
            noun: "item".into(),
            nouns: "items".into(),
            subject: String::new(),
            tagline: "A gallery for one joke, kept properly.".into(),
            description: "A free, community-run gallery: pictures, GIFs and clips, \
                          open to anonymous uploads, found by caption and by picture."
                .into(),
            about: vec![],
            alternate_names: vec![],
            origin: "http://127.0.0.1:3100".into(),
            contact_email: String::new(),
            repo_url: "https://github.com/TripleU613/GeekGallery".into(),
            sister_sites: vec![],
            like: Like::default(),
            theme: Theme::default(),
            assets_base: "/brand".into(),
            assets_version: String::new(),
            logo_width: 96,
            logo_height: 96,
            logo_has_name: false,
        }
    }
}

#[cfg(feature = "ssr")]
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl Flavor {
    /// Read the flavor from `SITE_*` and `THEME_*`. Missing values take the
    /// defaults above; nothing here can fail.
    #[cfg(feature = "ssr")]
    pub fn from_env() -> Self {
        let d = Self::default();
        let get =
            |k: &str, fallback: &str| env_non_empty(k).unwrap_or_else(|| fallback.to_string());
        let list = |k: &str| -> Vec<String> {
            env_non_empty(k)
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let noun = get("SITE_NOUN", &d.noun);
        let nouns = env_non_empty("SITE_NOUN_PLURAL").unwrap_or_else(|| format!("{noun}s"));
        let sister_sites = list("SITE_SISTER_SITES")
            .into_iter()
            .filter_map(|entry| {
                let (name, url) = entry.split_once('=')?;
                let (name, url) = (name.trim(), url.trim());
                (!name.is_empty() && url.starts_with("http")).then(|| SisterSite {
                    name: name.to_string(),
                    url: url.to_string(),
                })
            })
            .collect();
        let dl = Like::default();
        Self {
            name: get("SITE_NAME", &d.name),
            subject: get("SITE_SUBJECT", &d.subject),
            tagline: get("SITE_TAGLINE", &d.tagline),
            description: get("SITE_DESCRIPTION", &d.description),
            // Paragraph breaks travel as a literal backslash-n: the deploy
            // blob is one KEY=VALUE per line, so a value cannot contain a
            // real newline.
            about: env_non_empty("SITE_ABOUT")
                .map(|v| {
                    v.split("\\n")
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            alternate_names: list("SITE_ALTERNATE_NAMES"),
            origin: get("SITE_ORIGIN", &d.origin)
                .trim_end_matches('/')
                .to_string(),
            contact_email: get("SITE_CONTACT_EMAIL", &d.contact_email),
            repo_url: get("SITE_REPO_URL", &d.repo_url),
            sister_sites,
            like: Like {
                icon: env_non_empty("SITE_LIKE_ICON")
                    .map(|v| LikeIcon::parse(&v))
                    .unwrap_or_default(),
                verb: get("SITE_LIKE_VERB", &dl.verb),
                undo: get("SITE_LIKE_UNDO", &dl.undo),
                sort_label: get("SITE_LIKE_SORT_LABEL", &dl.sort_label),
            },
            theme: Theme::from_env(),
            assets_base: get("SITE_ASSETS_BASE", &d.assets_base)
                .trim_end_matches('/')
                .to_string(),
            assets_version: get("SITE_ASSETS_VERSION", &d.assets_version),
            logo_width: env_non_empty("SITE_LOGO_WIDTH")
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.logo_width),
            logo_height: env_non_empty("SITE_LOGO_HEIGHT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.logo_height),
            logo_has_name: env_non_empty("SITE_LOGO_HAS_NAME")
                .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes")),
            noun,
            nouns,
        }
    }

    /// `{noun}`, `{nouns}`, `{name}` and `{subject}` in a template, filled.
    pub fn fill(&self, template: &str) -> String {
        template
            .replace("{noun}", &self.noun)
            .replace("{nouns}", &self.nouns)
            .replace("{name}", &self.name)
            .replace("{subject}", &self.subject)
    }

    /// "a pic" / "an item": the noun with its article.
    pub fn a_noun(&self) -> String {
        format!("{} {}", article(&self.noun), self.noun)
    }

    /// The noun with its first letter upper-cased, for the start of a
    /// sentence or a chip label ("Pic of the day").
    pub fn noun_title(&self) -> String {
        capitalize(&self.noun)
    }

    /// "a Frog clip" / "a clip", for descriptions: the subject when there is
    /// one, otherwise just the kind.
    pub fn of_subject(&self, kind: &str) -> String {
        if self.subject.is_empty() {
            format!("{} {kind}", article(kind))
        } else {
            format!("{} {} {kind}", article(&self.subject), self.subject)
        }
    }

    /// One of the brand files, as a URL: `asset("logo.png")`.
    pub fn asset(&self, file: &str) -> String {
        if self.assets_version.is_empty() {
            format!("{}/{file}", self.assets_base)
        } else {
            format!("{}/{file}?v={}", self.assets_base, self.assets_version)
        }
    }

    /// `/pic/<slug>`: the path of one item's page.
    pub fn item_path(&self, slug: &str) -> String {
        format!("/{}/{slug}", self.noun)
    }

    /// `/pic/`: what every item page path starts with.
    pub fn item_prefix(&self) -> String {
        format!("/{}/", self.noun)
    }

    /// The whole thing as the JSON the shell embeds and `hydrate` reads back.
    /// `<` is escaped so the blob can never close its own `<script>`.
    pub fn to_embedded_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| "{}".into())
            .replace('<', "\\u003c")
    }
}

/// "a" or "an" for a word, by its first letter. Good enough for nouns a
/// gallery is named after; "an hour" is not one of them.
fn article(word: &str) -> &'static str {
    match word.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The `KEY=VALUE` lines of a deploy blob, as pairs.
///
/// Deliberately not a dotenv parser: a flavor's values are prose -- a tagline
/// with a `+` in it, an about paragraph with quotes, commas and `--` -- and
/// dotenv grammars decide for themselves where an unquoted value ends. Here
/// a value is everything after the first `=` to the end of the line, trimmed,
/// with one pair of matching quotes removed if the whole value is wrapped in
/// them. Blank lines and lines starting with `#` are skipped, and so is a
/// line with no `=` or an empty key.
pub fn parse_blob(blob: &str) -> Vec<(String, String)> {
    blob.lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let k = k.trim();
            if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let v = v.trim();
            let v = match (v.chars().next(), v.chars().last()) {
                (Some('"'), Some('"')) | (Some('\''), Some('\'')) if v.len() >= 2 => {
                    &v[1..v.len() - 1]
                }
                _ => v,
            };
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

static FLAVOR: OnceLock<Flavor> = OnceLock::new();

/// Install the flavor. Once, before anything renders: on the server from
/// `Flavor::from_env()` in `main`, on the client from the embedded JSON in
/// `hydrate()`. A second call is ignored, which is what a test that sets one
/// up twice wants.
pub fn set(flavor: Flavor) {
    let _ = FLAVOR.set(flavor);
}

/// The flavor. Defaults if nothing was installed, so a unit test or a
/// component rendered in isolation never panics over configuration.
pub fn get() -> &'static Flavor {
    FLAVOR.get_or_init(Flavor::default)
}

/// The noun as a `&'static str`, for the router: `StaticSegment` wants one,
/// and the flavor lives as long as the process.
pub fn noun_static() -> &'static str {
    &get().noun
}

/// The id of the `<script>` the shell embeds the flavor JSON in.
pub const EMBED_ID: &str = "gg-flavor";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_both_lengths_and_refuses_the_rest() {
        assert_eq!(parse_hex("#0a0b0f"), Some([10, 11, 15]));
        assert_eq!(parse_hex("#fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex(" #9aa4ff "), Some([154, 164, 255]));
        assert_eq!(parse_hex("0a0b0f"), None);
        assert_eq!(parse_hex("#0a0b"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
    }

    #[test]
    fn theme_css_is_channels_per_token() {
        let css = Theme::default().css();
        assert!(css.starts_with(":root{--c-bg:10 11 15;"));
        assert!(css.contains("--c-accent:154 164 255;"));
        assert!(css.ends_with('}'));
    }

    #[test]
    fn templates_fill_and_articles_agree() {
        let f = Flavor {
            subject: "Frog".into(),
            ..Flavor::default()
        };
        assert_eq!(f.fill("Give this {noun} a mic"), "Give this item a mic");
        assert_eq!(f.a_noun(), "an item");
        assert_eq!(f.noun_title(), "Item");
        assert_eq!(f.of_subject("clip"), "a Frog clip");
        assert_eq!(f.item_path("x"), "/item/x");
        let plain = Flavor::default();
        assert_eq!(plain.of_subject("clip"), "a clip");
        assert_eq!(plain.a_noun(), "an item");
    }

    #[test]
    fn asset_urls_carry_the_version_only_when_set() {
        let mut f = Flavor::default();
        assert_eq!(f.asset("logo.png"), "/brand/logo.png");
        f.assets_base = "https://media.example.com/brand".into();
        f.assets_version = "3".into();
        assert_eq!(
            f.asset("logo.png"),
            "https://media.example.com/brand/logo.png?v=3"
        );
    }

    #[test]
    fn like_icon_names_are_forgiving() {
        assert_eq!(LikeIcon::parse("Mic"), LikeIcon::Mic);
        assert_eq!(LikeIcon::parse("tear"), LikeIcon::Droplet);
        assert_eq!(LikeIcon::parse("nonsense"), LikeIcon::Heart);
    }

    #[test]
    fn blobs_keep_prose_values_whole() {
        let blob = "# a comment\nSITE_TAGLINE=Frogs + memes. Every frog, collected.\n\n\
                    SITE_ABOUT=It started with \"Frog\" -- and grew.\\nSecond paragraph.\r\n\
                    QUOTED=\"  spaced  \"\nbad line\n=novalue\nEMPTY=\n";
        let pairs = parse_blob(blob);
        assert_eq!(pairs.len(), 4, "{pairs:?}");
        assert_eq!(pairs[0].1, "Frogs + memes. Every frog, collected.");
        assert_eq!(
            pairs[1].1,
            "It started with \"Frog\" -- and grew.\\nSecond paragraph."
        );
        assert_eq!(pairs[2].1, "  spaced  ");
        assert_eq!(pairs[3], ("EMPTY".to_string(), String::new()));
    }

    #[test]
    fn round_trips_through_json() {
        let f = Flavor::default();
        let json = serde_json::to_string(&f).unwrap();
        let back: Flavor = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }
}
