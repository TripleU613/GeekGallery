//! Media intake: sniff, decode, bound, thumbnail, watermark, persist.
//!
//! Three kinds of thing come through here, and they are handled differently on
//! purpose:
//!
//! - A **still image** (PNG, JPEG, WEBP) is decoded and re-encoded to PNG from
//!   raw pixels. That strips EXIF and anything smuggled after the image data,
//!   and the PNG carries an invisible provenance watermark (see `watermark`)
//!   plus this site's own text chunks.
//! - A **GIF** is stored byte-for-byte. Re-encoding would keep one frame and
//!   throw the animation away, which for a meme is the whole file. Its first
//!   frame becomes the thumbnail.
//! - A **video** (mp4, webm) is stored byte-for-byte too. Nothing in this
//!   process decodes video and nothing in the container transcodes it -- the
//!   browser that uploaded it drew a frame onto a canvas and sent that JPEG
//!   along as the poster, and the poster is what the thumbnail and the
//!   `og:image` come from.
//!
//! Persistence sits behind the `Backend` trait so the same intake path works
//! against local disk in development and Cloudflare R2 in production. Object
//! keys are generated UUIDs plus an extension derived from the sniffed
//! container, never anything the uploader supplied, so there is no
//! path-traversal or key-injection surface either way.
//!
//! Duplicate detection (`dedupe`) happens in `upload_route` before any of this
//! writes, so a rejected upload leaves no files to clean up.

use std::path::PathBuf;
use std::sync::Arc;

use image::DynamicImage;
use uuid::Uuid;

use crate::models::MediaKind;

pub mod local;
pub mod r2;

pub const UPLOAD_ROOT: &str = "uploads";

/// A still or a GIF. Generous for a meme, small enough that a phone on a bad
/// connection finishes before giving up.
pub const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;

/// A clip. Sixty megabytes is a minute of decent 1080p from a phone camera;
/// anything longer belongs on a video host, not in a meme gallery.
pub const MAX_VIDEO_BYTES: usize = 60 * 1024 * 1024;

/// The largest body the upload route accepts at all -- the video limit, since
/// that is the bigger of the two. The per-kind limits above are applied once
/// the container is known.
pub const MAX_UPLOAD_BYTES: usize = MAX_VIDEO_BYTES;

pub const THUMB_MAX_EDGE: u32 = 480;

/// The longest edge a stored still may have. Bigger than this is downscaled:
/// a 6000px phone photo of a meme is R2 egress spent on pixels no screen has.
/// Not a square and not a fixed size -- memes carry captions along their top
/// and bottom edges, and cropping to a square was cutting the joke off.
pub const MAX_EDGE: u32 = 2048;

/// Hard cap on decoded pixel count, checked before allocating the full image.
/// Blocks decompression bombs: a few-KB PNG can claim 50000x50000.
const MAX_PIXELS: u64 = 40_000_000;

/// What the leading bytes of an upload say it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Png,
    Jpeg,
    Webp,
    Gif,
    Mp4,
    Webm,
}

impl Container {
    pub fn kind(self) -> MediaKind {
        match self {
            Container::Png | Container::Jpeg | Container::Webp => MediaKind::Image,
            Container::Gif => MediaKind::Gif,
            Container::Mp4 | Container::Webm => MediaKind::Video,
        }
    }

    /// The extension the stored *original* gets. A JPEG stays a JPEG; PNG
    /// and WebP become PNG (see `store` for why the two differ).
    pub fn stored_extension(self) -> &'static str {
        match self {
            Container::Png | Container::Webp => "png",
            Container::Jpeg => "jpg",
            Container::Gif => "gif",
            Container::Mp4 => "mp4",
            Container::Webm => "webm",
        }
    }

    pub fn stored_mime(self) -> &'static str {
        match self {
            Container::Png | Container::Webp => "image/png",
            Container::Jpeg => "image/jpeg",
            Container::Gif => "image/gif",
            Container::Mp4 => "video/mp4",
            Container::Webm => "video/webm",
        }
    }
}

/// Identify an upload by its magic bytes, never by its declared type or its
/// filename -- both are whatever the browser or the uploader said.
///
/// `None` is "not something this site stores", which covers everything from
/// a PDF to a renamed executable.
pub fn sniff(bytes: &[u8]) -> Option<Container> {
    if bytes.len() < 12 {
        return None;
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Container::Png);
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(Container::Jpeg);
    }
    if bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(Container::Webp);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(Container::Gif);
    }
    // ISO base media: a size then `ftyp`. Covers mp4, m4v and QuickTime .mov,
    // all of which browsers play through the same `<video>` path when the codecs
    // inside are ones they know. Content type is video/mp4 for all of them.
    if &bytes[4..8] == b"ftyp" {
        return Some(Container::Mp4);
    }
    // EBML header: webm, and Matroska generally.
    if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return Some(Container::Webm);
    }
    None
}

/// A decoded upload, ready to be hashed and stored.
pub enum Media {
    /// Decoded pixels and the container they came out of, which decides how
    /// they go back in: JPEG stays JPEG, everything else becomes PNG.
    Image {
        img: DynamicImage,
        source: Container,
    },
    /// The original bytes plus the first frame for the thumbnail.
    Gif { bytes: Vec<u8>, first: DynamicImage },
    /// The original bytes, the container they are in, and the still the
    /// browser captured. `poster` is `None` only when the browser could not
    /// draw one (a codec the canvas could not read, or JavaScript failed
    /// partway), in which case `store` paints a placeholder.
    Video {
        bytes: Vec<u8>,
        container: Container,
        poster: Option<DynamicImage>,
    },
}

/// By hand, so a test failure prints "Video(2.1MB, mp4, poster)" rather than
/// two million bytes.
impl std::fmt::Debug for Media {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Media::Image { img, source } => {
                write!(f, "Image({}x{}, {source:?})", img.width(), img.height())
            }
            Media::Gif { bytes, first } => write!(
                f,
                "Gif({} bytes, {}x{})",
                bytes.len(),
                first.width(),
                first.height()
            ),
            Media::Video {
                bytes,
                container,
                poster,
            } => write!(
                f,
                "Video({} bytes, {:?}, poster: {})",
                bytes.len(),
                container,
                poster.is_some()
            ),
        }
    }
}

impl Media {
    pub fn kind(&self) -> MediaKind {
        match self {
            Media::Image { .. } => MediaKind::Image,
            Media::Gif { .. } => MediaKind::Gif,
            Media::Video { .. } => MediaKind::Video,
        }
    }

    /// What the exact-duplicate check hashes.
    ///
    /// Decoded pixels for a still, so the same picture saved through a
    /// different container is still caught. The file bytes for a GIF or a
    /// video, because those are stored as-is and "same bytes" is the only
    /// sameness that can be established without decoding them.
    pub fn fingerprint(&self) -> String {
        match self {
            Media::Image { img, .. } => crate::dedupe::sha256_hex(img.to_rgba8().as_raw()),
            Media::Gif { bytes, .. } | Media::Video { bytes, .. } => {
                crate::dedupe::sha256_hex(bytes)
            }
        }
    }
}

/// Decode an upload, rejecting anything oversized or unrecognised before it is
/// rasterized. Separate from `store` so the caller can hash and dedupe before
/// anything is persisted.
///
/// `poster` is the JPEG the browser captured for a video, if it sent one.
/// Ignored for anything that is not a video.
pub fn decode(bytes: Vec<u8>, poster: Option<&[u8]>) -> anyhow::Result<Media> {
    let container = sniff(&bytes).ok_or_else(|| {
        anyhow::anyhow!("not a format this site stores (PNG, JPG, WEBP, GIF, MP4 or WEBM)")
    })?;

    let limit = match container.kind() {
        MediaKind::Video => MAX_VIDEO_BYTES,
        _ => MAX_IMAGE_BYTES,
    };
    if bytes.len() > limit {
        anyhow::bail!(
            "too big: {:.1}MB, limit is {}MB for {}",
            bytes.len() as f64 / 1_048_576.0,
            limit / 1_048_576,
            match container.kind() {
                MediaKind::Video => "a video",
                MediaKind::Gif => "a GIF",
                MediaKind::Image => "an image",
            }
        );
    }

    match container.kind() {
        MediaKind::Image => Ok(Media::Image {
            img: decode_still(&bytes)?,
            source: container,
        }),
        // `image` decodes the first frame of a GIF when asked for a single image,
        // which is exactly the frame a thumbnail wants.
        MediaKind::Gif => {
            // The still is the liveliest early frame, not frame zero: a GIF
            // that opens on a black or a title card would otherwise be a black
            // tile in the grid for the whole of its life.
            let first = match crate::media_tools::liveliest_gif_frame(&bytes) {
                Some(f) if !crate::media_tools::is_flat(&f) => f,
                _ => decode_still(&bytes)?,
            };
            Ok(Media::Gif { bytes, first })
        }
        MediaKind::Video => {
            // Put the index at the front of an MP4 so playback can start
            // before the tail has arrived. Done before the fingerprint so the
            // stored bytes are what gets hashed. WebM already streams.
            let bytes = match container {
                Container::Mp4 => match crate::faststart::relocate(&bytes) {
                    Some(fast) => {
                        tracing::debug!("moved moov ahead of mdat ({} bytes)", fast.len());
                        fast
                    }
                    None => bytes,
                },
                _ => bytes,
            };
            // A poster that fails to decode is dropped, not fatal: the video is
            // the upload, the poster is a courtesy the browser did its best on.
            let poster = poster.and_then(|p| match decode_still(p) {
                Ok(img) => Some(img),
                Err(e) => {
                    tracing::warn!("video poster undecodable, using a placeholder: {e}");
                    None
                }
            });
            Ok(Media::Video {
                bytes,
                container,
                poster,
            })
        }
    }
}

/// Decode a still, with the header's dimensions checked before the body is
/// touched.
fn decode_still(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| anyhow::anyhow!("unreadable image: {e}"))?;

    if let Ok((w, h)) = reader.into_dimensions() {
        let pixels = w as u64 * h as u64;
        if pixels > MAX_PIXELS {
            anyhow::bail!("{w}x{h} is too many pixels");
        }
    }

    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| anyhow::anyhow!("unreadable image: {e}"))?;

    reader
        .decode()
        .map_err(|e| anyhow::anyhow!("could not decode: {e}"))
}

/// Scale a still down so neither edge exceeds `MAX_EDGE`. Never up, never a
/// crop: aspect is kept exactly. A no-op for anything already small enough,
/// which is most memes.
pub fn bound(img: &DynamicImage) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w <= MAX_EDGE && h <= MAX_EDGE {
        return img.clone();
    }
    // Lanczos3 is the only filter here that does not visibly alias on the way
    // down at these ratios.
    img.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Lanczos3)
}

/// Where the bytes actually live.
#[async_trait::async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> anyhow::Result<()>;

    /// Fetch an object's bytes back. Used only for the same-origin download
    /// route (`Content-Disposition: attachment` has to come from a response
    /// this server controls -- browsers ignore an `<a download>` attribute
    /// pointed at a cross-origin URL, and R2's public domain is a different
    /// origin from the app itself). Normal viewing never calls this; the
    /// gallery links straight to the CDN.
    async fn get(&self, key: &str) -> anyhow::Result<Vec<u8>>;

    /// Best-effort: a missing object is not an error, since the goal of calling
    /// this is for the object to be gone.
    async fn delete(&self, key: &str);

    /// Publicly reachable URL for a stored key.
    fn public_url(&self, key: &str) -> String;

    fn name(&self) -> String;
}

static BACKEND: std::sync::OnceLock<Arc<dyn Backend>> = std::sync::OnceLock::new();

pub fn set_backend(b: Arc<dyn Backend>) {
    let _ = BACKEND.set(b);
}

pub fn backend() -> &'static Arc<dyn Backend> {
    BACKEND.get().expect("storage backend not initialized")
}

/// Pick a backend from the environment: R2 when fully configured, local disk
/// otherwise. Falling back rather than failing keeps `cargo leptos watch`
/// working with no cloud credentials present.
pub async fn backend_from_env() -> Arc<dyn Backend> {
    match r2::R2::from_env().await {
        Ok(Some(r2)) => match r2.check().await {
            Ok(()) => Arc::new(r2),
            Err(e) => {
                tracing::error!("R2 credentials rejected ({e}); falling back to local disk");
                Arc::new(local::LocalDisk::new(UPLOAD_ROOT, "/uploads"))
            }
        },
        Ok(None) => Arc::new(local::LocalDisk::new(UPLOAD_ROOT, "/uploads")),
        Err(e) => {
            tracing::error!("R2 is configured but unusable ({e}); falling back to local disk");
            Arc::new(local::LocalDisk::new(UPLOAD_ROOT, "/uploads"))
        }
    }
}

pub struct Stored {
    pub id: String,
    pub kind: MediaKind,
    pub orig_url: String,
    pub thumb_url: String,
    pub poster_url: Option<String>,
    pub width: u32,
    pub height: u32,
}

/// The prefix embedded in every still's invisible watermark. Kept short: the
/// payload competes with image content for bits, and this is already enough to
/// trace a copy back to a specific upload via `watermark::extract`.
/// The site name is part of it, so a still traced back says which gallery it
/// came from as well as which upload.
fn watermark_prefix() -> String {
    format!("{}:v1:", crate::flavor::get().name)
}

/// The original's key. The extension is part of the key because the kind is:
/// `orig/<id>.png`, `.jpg`, `.gif`, `.mp4` or `.webm`.
pub fn orig_key(id: &str, extension: &str) -> String {
    format!("orig/{id}.{extension}")
}

/// Thumbnails are JPEG, and the extension is part of the key because the
/// original's is not fixed.
///
/// Measured on the site this grew out of, a 480px PNG thumbnail weighed ~300KB
/// and one 24-tile page pulled ~7MB of images; the same tile as JPEG is
/// 20-40KB. Losing alpha is fine here: the thumbnail is only ever drawn into a
/// grid tile with `object-cover`, so nothing behind it can show through.
pub fn thumb_key(id: &str) -> String {
    format!("thumb/{id}.jpg")
}

/// The full-size still of a video. Videos only.
pub fn poster_key(id: &str) -> String {
    format!("poster/{id}.jpg")
}

/// Every key a item could have, so a delete can be issued without first
/// looking up which container the original was in. Deleting a key that was
/// never written is a no-op on both backends.
pub fn all_keys(id: &str) -> Vec<String> {
    let mut keys: Vec<String> = ["png", "jpg", "gif", "mp4", "webm"]
        .iter()
        .map(|ext| orig_key(id, ext))
        .collect();
    keys.push(thumb_key(id));
    keys.push(poster_key(id));
    keys
}

/// Quality for thumbnails and posters. 82 is the usual sweet spot where JPEG
/// artefacts stop being visible on photographic content; the caption text
/// memes carry is the part that would show ringing first, and it survives it.
const JPEG_QUALITY: u8 = 82;

/// Quality for a video's poster. Higher than a thumbnail: it is shown at
/// full size for as long as the clip has not started, and is the frame every
/// unfurl shows. The browser already softened it once on the way up.
pub(crate) const POSTER_QUALITY: u8 = 90;

/// The tile background these are drawn on (`surface-raised` in
/// tailwind.config.js), so a transparent source flattens to the colour it
/// would have appeared to sit on rather than to black.
const FLATTEN_BG: image::Rgba<u8> = image::Rgba([0x17, 0x1a, 0x25, 0xff]);

/// Quality for a JPEG *original*. Higher than the thumbnails: this is the
/// file people save and repost, and a second generation of JPEG loss at 82
/// starts to show on the flat colour and hard edges of caption text.
const JPEG_ORIGINAL_QUALITY: u8 = 90;

pub(crate) fn encode_jpeg(img: &DynamicImage) -> anyhow::Result<Vec<u8>> {
    encode_jpeg_at(img, JPEG_QUALITY)
}

pub(crate) fn encode_jpeg_at(img: &DynamicImage, quality: u8) -> anyhow::Result<Vec<u8>> {
    use image::RgbaImage;

    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut flat = RgbaImage::from_pixel(w, h, FLATTEN_BG);
    image::imageops::overlay(&mut flat, &rgba, 0, 0);

    let rgb = DynamicImage::ImageRgba8(flat).to_rgb8();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality).encode(
        rgb.as_raw(),
        w,
        h,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(buf)
}

/// What a video shows before it plays when the browser sent no frame: a flat
/// panel in the site's surface colour, 16:9. Deliberately blank rather than
/// text -- drawing text needs a font in the binary, and the card draws a play
/// badge over whatever is here anyway.
fn placeholder_poster() -> DynamicImage {
    DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1280, 720, FLATTEN_BG))
}

/// Persist a decoded upload plus its thumbnail (and poster, for a video).
/// Call only after the duplicate check has passed -- this writes files, and a
/// rejected upload should leave none behind.
///
/// `title`/`uploader_name` are written back into a still's PNG text chunks --
/// provenance metadata the site controls, replacing whatever (or nothing) the
/// source file carried before the EXIF strip. `dims` is the browser's report
/// of a video's frame size; a still knows its own.
pub async fn store(
    media: Media,
    title: &str,
    uploader_name: Option<&str>,
    dims: Option<(u32, u32)>,
) -> anyhow::Result<Stored> {
    let id = Uuid::new_v4().to_string();
    let page_url = crate::seo::absolute(&crate::flavor::get().item_path(&id));
    let be = backend();

    // Every put is collected so a failure partway removes what already landed
    // rather than leaving a item the gallery cannot render.
    let mut written: Vec<String> = Vec::new();
    macro_rules! put {
        ($key:expr, $bytes:expr, $ct:expr) => {{
            let key: String = $key;
            if let Err(e) = be.put(&key, $bytes, $ct).await {
                for k in &written {
                    be.delete(k).await;
                }
                return Err(e);
            }
            written.push(key.clone());
            key
        }};
    }

    match media {
        Media::Image { img, source } => {
            let img = bound(&img);
            let (width, height) = (img.width(), img.height());
            let thumb_bytes = encode_jpeg(&img.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE))?;
            let meta = PngMeta {
                title,
                uploader_name,
                page_url: &page_url,
            };

            // A JPEG source goes back out as JPEG. Measured: a 2048px photo
            // meme that arrived as a 400KB JPEG became a 6-8MB PNG, and that
            // PNG was the largest thing on its page by a factor of ten. The
            // re-encode still strips EXIF (the decoder never carries it) and
            // still writes provenance, into a COM segment rather than iTXt.
            // What a JPEG cannot carry is the pixel watermark: LSB data does
            // not survive DCT quantisation, so `watermark::embed` is not run
            // on it -- it would only cost bits and prove nothing later.
            //
            // PNG and WebP sources become PNG, exactly as before: lossless in,
            // lossless out, watermark and iTXt included.
            let (orig_bytes, ext, mime) = if source == Container::Jpeg {
                (encode_jpeg_original(&img, &meta)?, "jpg", "image/jpeg")
            } else {
                // Only the original carries the watermark: a thumbnail this
                // small has nowhere near enough pixels for the payload to
                // survive being meaningful.
                let watermarked =
                    crate::watermark::embed(&img, format!("{}{id}", watermark_prefix()).as_bytes());
                (encode_png(&watermarked, &meta)?, "png", "image/png")
            };

            let ok = put!(orig_key(&id, ext), orig_bytes, mime);
            let tk = put!(thumb_key(&id), thumb_bytes, "image/jpeg");
            Ok(Stored {
                orig_url: be.public_url(&ok),
                thumb_url: be.public_url(&tk),
                poster_url: None,
                id,
                kind: MediaKind::Image,
                width,
                height,
            })
        }
        Media::Gif { bytes, first } => {
            let (width, height) = (first.width(), first.height());
            let thumb_bytes = encode_jpeg(&first.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE))?;
            let ok = put!(orig_key(&id, "gif"), bytes, "image/gif");
            let tk = put!(thumb_key(&id), thumb_bytes, "image/jpeg");
            Ok(Stored {
                orig_url: be.public_url(&ok),
                thumb_url: be.public_url(&tk),
                poster_url: None,
                id,
                kind: MediaKind::Gif,
                width,
                height,
            })
        }
        Media::Video {
            bytes,
            container,
            poster,
        } => {
            // The browser's frame, unless it is the black one a phone hands
            // over for a clip it had not decoded; then a frame ffmpeg picks
            // from the file itself; then the placeholder.
            let poster = match poster.filter(|p| !crate::media_tools::is_flat(p)) {
                Some(p) => p,
                None => match crate::media_tools::video_poster(&bytes).await {
                    Some(jpeg) => match image::load_from_memory(&jpeg) {
                        Ok(img) => img,
                        Err(_) => placeholder_poster(),
                    },
                    None => placeholder_poster(),
                },
            };
            let poster = bound(&poster);
            // The browser's numbers first: they describe the video, and the
            // poster it drew is at most a copy of them. Then the poster's own
            // size, which is right whenever the browser drew the frame at
            // native resolution. A zero from either side falls through.
            let (width, height) = dims
                .filter(|(w, h)| *w > 0 && *h > 0)
                .unwrap_or((poster.width(), poster.height()));
            let poster_bytes = encode_jpeg_at(&poster, POSTER_QUALITY)?;
            let thumb_bytes = encode_jpeg(&poster.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE))?;

            let ok = put!(
                orig_key(&id, container.stored_extension()),
                bytes,
                container.stored_mime()
            );
            let pk = put!(poster_key(&id), poster_bytes, "image/jpeg");
            let tk = put!(thumb_key(&id), thumb_bytes, "image/jpeg");
            Ok(Stored {
                orig_url: be.public_url(&ok),
                thumb_url: be.public_url(&tk),
                poster_url: Some(be.public_url(&pk)),
                id,
                kind: MediaKind::Video,
                width,
                height,
            })
        }
    }
}

struct PngMeta<'a> {
    title: &'a str,
    uploader_name: Option<&'a str>,
    page_url: &'a str,
}

/// Encodes via the `png` crate directly rather than `DynamicImage::write_to`:
/// `image`'s generic encoder has no way to attach text chunks, and provenance
/// metadata is the whole point of this function. `iTXt`, not `tEXt`: titles
/// are free text and can contain characters `tEXt`'s Latin-1 encoding can't
/// represent.
fn encode_png(img: &DynamicImage, meta: &PngMeta) -> anyhow::Result<Vec<u8>> {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    let mut buf = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut buf, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.add_itxt_chunk("Title".to_string(), meta.title.to_string())?;
        encoder.add_itxt_chunk("Software".to_string(), software_line())?;
        encoder.add_itxt_chunk("Description".to_string(), meta.page_url.to_string())?;
        if let Some(author) = meta.uploader_name {
            encoder.add_itxt_chunk("Author".to_string(), author.to_string())?;
        }
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba.as_raw())?;
    }
    Ok(buf)
}

/// "sitename (example.com)": the site, as written into every stored still's
/// metadata.
fn software_line() -> String {
    let f = crate::flavor::get();
    let host = f
        .origin
        .split_once("://")
        .map(|(_, h)| h)
        .unwrap_or(&f.origin);
    format!("{} ({host})", f.name)
}

/// A JPEG original: the pixels at `JPEG_ORIGINAL_QUALITY` with one COM
/// segment of provenance spliced in right after SOI. Every JPEG reader skips
/// COM; `exiftool`/`file` show it; and it is the same facts the PNG path
/// writes as iTXt, so a saved copy of either kind of still says where it
/// came from.
fn encode_jpeg_original(img: &DynamicImage, meta: &PngMeta) -> anyhow::Result<Vec<u8>> {
    let body = encode_jpeg_at(img, JPEG_ORIGINAL_QUALITY)?;
    let mut note = format!("{} | {} | {}", software_line(), meta.title, meta.page_url);
    if let Some(author) = meta.uploader_name {
        note.push_str(" | by ");
        note.push_str(author);
    }
    Ok(with_jpeg_comment(body, &note))
}

/// Insert a COM (0xFFFE) segment after the SOI marker. The segment length
/// field counts itself, so the payload is capped at 65533 bytes -- far more
/// than a title will ever be, but a cap is a cap.
fn with_jpeg_comment(jpeg: Vec<u8>, text: &str) -> Vec<u8> {
    if jpeg.len() < 2 || jpeg[..2] != [0xff, 0xd8] {
        return jpeg;
    }
    let mut payload = text.as_bytes();
    if payload.len() > 65_533 {
        // Cut on a char boundary so the comment stays valid UTF-8.
        let mut cut = 65_533;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        payload = &payload[..cut];
    }
    let len = (payload.len() + 2) as u16;
    let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xff, 0xfe]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Delete every object a item could have.
pub async fn remove(id: &str) {
    let be = backend();
    for key in all_keys(id) {
        be.delete(&key).await;
    }
}

/// Local path for a key, used only by the disk backend.
fn local_path(root: &str, key: &str) -> PathBuf {
    PathBuf::from(root).join(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_derived_from_id_and_kind_only() {
        assert_eq!(orig_key("abc", "png"), "orig/abc.png");
        assert_eq!(orig_key("abc", "mp4"), "orig/abc.mp4");
        assert_eq!(thumb_key("abc"), "thumb/abc.jpg");
        assert_eq!(poster_key("abc"), "poster/abc.jpg");
        // A delete without a lookup has to cover every container a item can be
        // stored in, or a removed video leaves its bytes behind on R2.
        let all = all_keys("abc");
        for k in [
            "orig/abc.png",
            "orig/abc.jpg",
            "orig/abc.gif",
            "orig/abc.mp4",
            "orig/abc.webm",
            "thumb/abc.jpg",
            "poster/abc.jpg",
        ] {
            assert!(all.iter().any(|x| x == k), "missing {k}");
        }
    }

    #[test]
    fn sniffing_reads_magic_bytes_not_names() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0u8; 8]);
        assert_eq!(sniff(&png), Some(Container::Png));
        let mut jpg = vec![0xff, 0xd8, 0xff, 0xe0];
        jpg.extend([0u8; 8]);
        assert_eq!(sniff(&jpg), Some(Container::Jpeg));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Container::Webp));
        assert_eq!(sniff(b"GIF89a\0\0\0\0\0\0\0\0"), Some(Container::Gif));
        assert_eq!(
            sniff(b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2"),
            Some(Container::Mp4)
        );
        assert_eq!(sniff(b"\0\0\0\x14ftypqt  \0\0\0\0"), Some(Container::Mp4));
        assert_eq!(
            sniff(&[0x1a, 0x45, 0xdf, 0xa3, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(Container::Webm)
        );
        assert_eq!(sniff(b"%PDF-1.7 hello world"), None);
        assert_eq!(sniff(b"short"), None);
    }

    #[test]
    fn oversized_input_is_rejected_before_decode() {
        let mut huge = b"\x89PNG\r\n\x1a\n".to_vec();
        huge.resize(MAX_IMAGE_BYTES + 1, 0);
        let err = decode(huge, None).unwrap_err().to_string();
        assert!(err.contains("too big"), "got: {err}");

        // A video gets the bigger limit, so the same size passes the size
        // check and fails only because there is no real video here to store --
        // which it does not: a video is never decoded, so this succeeds.
        let mut clip = b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2".to_vec();
        clip.resize(MAX_IMAGE_BYTES + 1, 0);
        assert!(matches!(decode(clip, None), Ok(Media::Video { .. })));

        let mut too_long = b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2".to_vec();
        too_long.resize(MAX_VIDEO_BYTES + 1, 0);
        let err = decode(too_long, None).unwrap_err().to_string();
        assert!(err.contains("a video"), "got: {err}");
    }

    #[test]
    fn garbage_is_not_media() {
        let err = decode(b"definitely not a png, nor anything".to_vec(), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a format"), "got: {err}");
    }

    /// A still keeps its aspect and is only ever scaled down.
    #[test]
    fn bound_never_upscales_or_crops() {
        let small = DynamicImage::new_rgba8(640, 360);
        let out = bound(&small);
        assert_eq!((out.width(), out.height()), (640, 360));

        let wide = DynamicImage::new_rgba8(8000, 2000);
        let out = bound(&wide);
        assert_eq!((out.width(), out.height()), (2048, 512));

        let tall = DynamicImage::new_rgba8(1000, 5000);
        let out = bound(&tall);
        // `resize` rounds 409.6 up; the aspect is what is being asserted.
        assert_eq!((out.width(), out.height()), (410, 2048));
    }

    /// The same pixels through two containers must fingerprint the same, or a
    /// re-saved copy of a meme is not caught as a duplicate.
    #[test]
    fn stills_fingerprint_by_pixels() {
        let img = DynamicImage::new_rgba8(4, 4);
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut webp = std::io::Cursor::new(Vec::new());
        img.write_to(&mut webp, image::ImageFormat::WebP).unwrap();
        let a = decode(png.into_inner(), None).unwrap().fingerprint();
        let b = decode(webp.into_inner(), None).unwrap().fingerprint();
        assert_eq!(a, b);
    }

    /// A JPEG source is remembered as one, so `store` can keep it a JPEG;
    /// the other stills are not.
    #[test]
    fn stills_remember_their_source_container() {
        let img = DynamicImage::new_rgb8(4, 4);
        let mut jpg = std::io::Cursor::new(Vec::new());
        img.write_to(&mut jpg, image::ImageFormat::Jpeg).unwrap();
        match decode(jpg.into_inner(), None).unwrap() {
            Media::Image { source, .. } => assert_eq!(source, Container::Jpeg),
            other => panic!("expected an image, got {other:?}"),
        }
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        match decode(png.into_inner(), None).unwrap() {
            Media::Image { source, .. } => assert_eq!(source, Container::Png),
            other => panic!("expected an image, got {other:?}"),
        }
        assert_eq!(Container::Jpeg.stored_extension(), "jpg");
        assert_eq!(Container::Jpeg.stored_mime(), "image/jpeg");
        assert_eq!(Container::Webp.stored_extension(), "png");
    }

    /// The provenance comment lands right after SOI, is well-formed, and the
    /// result still decodes to the same size.
    #[test]
    fn jpeg_originals_carry_a_comment_and_still_decode() {
        let img = DynamicImage::new_rgb8(16, 9);
        let meta = PngMeta {
            title: "item, mid-sentence",
            uploader_name: Some("someone"),
            page_url: "https://geekgallery.com/item/x",
        };
        let bytes = encode_jpeg_original(&img, &meta).unwrap();
        assert_eq!(&bytes[..4], &[0xff, 0xd8, 0xff, 0xfe]);
        let len = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let comment = std::str::from_utf8(&bytes[6..4 + len]).unwrap();
        assert!(comment.contains("item, mid-sentence"), "got {comment}");
        assert!(comment.contains("/item/x"));
        assert!(comment.contains("by someone"));
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (16, 9));

        // Not a JPEG: passed through untouched.
        assert_eq!(with_jpeg_comment(vec![1, 2, 3], "x"), vec![1, 2, 3]);
        // An absurd comment is cut, not rejected, and the cut is a char
        // boundary.
        let long = "é".repeat(40_000);
        let out = with_jpeg_comment(vec![0xff, 0xd8, 0xff, 0xd9], &long);
        let len = u16::from_be_bytes([out[4], out[5]]) as usize;
        assert!(std::str::from_utf8(&out[6..4 + len]).is_ok());
    }

    /// An MP4 with its index at the back comes out of `decode` with the index
    /// at the front; everything else is untouched.
    #[test]
    fn mp4s_are_fast_started_on_the_way_in() {
        fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(payload);
            v
        }
        let mut file = bx(b"ftyp", b"isom\0\0\x02\0isomiso2");
        file.extend(bx(b"mdat", &[7; 16]));
        let mut stco = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stco.extend(32u32.to_be_bytes());
        file.extend(bx(b"moov", &bx(b"stco", &stco)));
        match decode(file.clone(), None).unwrap() {
            Media::Video { bytes, .. } => {
                assert_eq!(bytes.len(), file.len());
                assert_eq!(&bytes[28..32], b"moov");
            }
            other => panic!("expected a video, got {other:?}"),
        }
    }

    /// A video's poster failing to decode is not the video failing.
    #[test]
    fn a_bad_poster_does_not_sink_the_video() {
        let clip = b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2".to_vec();
        match decode(clip, Some(b"not a jpeg at all, sorry")).unwrap() {
            Media::Video { poster, .. } => assert!(poster.is_none()),
            _ => panic!("expected a video"),
        }
    }
}
