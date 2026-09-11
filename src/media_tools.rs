//! Frames and measurements the server can take of a clip or a GIF without
//! decoding video in Rust: ffmpeg for video (present in the image, pinned by
//! hash, for the link importer), the `image` crate for GIF frames, and a
//! flatness test that tells a real frame from the black one a phone hands
//! over when its canvas had nothing decoded yet.

use std::path::PathBuf;
use std::time::Duration;

use image::DynamicImage;

fn ffmpeg() -> String {
    match std::env::var("FFMPEG_DIR") {
        Ok(d) => format!("{d}/ffmpeg"),
        Err(_) => "ffmpeg".into(),
    }
}
fn ffprobe() -> String {
    match std::env::var("FFMPEG_DIR") {
        Ok(d) => format!("{d}/ffprobe"),
        Err(_) => "ffprobe".into(),
    }
}

async fn scratch(bytes: &[u8]) -> Option<PathBuf> {
    let p = std::env::temp_dir().join(format!("item-frame-{}.bin", uuid::Uuid::new_v4()));
    tokio::fs::write(&p, bytes).await.ok()?;
    Some(p)
}

/// A representative frame of a video as JPEG: ffmpeg's `thumbnail` filter
/// picks the most typical of the first N frames, which skips the black and
/// the fade-in the same way the browser's cover picker does. `None` when
/// ffmpeg is absent or the file defeats it; the caller has a placeholder.
pub async fn video_poster(bytes: &[u8]) -> Option<Vec<u8>> {
    let path = scratch(bytes).await?;
    let out = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(ffmpeg())
            .args(["-v", "error", "-y", "-i"])
            .arg(&path)
            .args([
                "-vf",
                "thumbnail=120,scale='min(1280,iw)':-2",
                "-frames:v",
                "1",
                "-f",
                "image2",
                "-c:v",
                "mjpeg",
                "-q:v",
                "2",
                "pipe:1",
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    match out {
        Ok(Ok(o)) if o.status.success() && !o.stdout.is_empty() => Some(o.stdout),
        Ok(Ok(o)) => {
            tracing::warn!("ffmpeg poster: {}", String::from_utf8_lossy(&o.stderr));
            None
        }
        Ok(Err(e)) => {
            tracing::warn!("ffmpeg not runnable: {e}");
            None
        }
        Err(_) => None,
    }
}

/// `(width, height, duration)` from ffprobe, for a WebM or an MP4 whose
/// boxes did not say.
pub async fn probe(bytes: &[u8]) -> Option<(Option<u32>, Option<u32>, Option<f64>)> {
    let path = scratch(bytes).await?;
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(ffprobe())
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height:format=duration",
                "-of",
                "json",
            ])
            .arg(&path)
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    let o = out.ok()?.ok()?;
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).ok()?;
    let w = v["streams"][0]["width"].as_u64().map(|w| w as u32);
    let h = v["streams"][0]["height"].as_u64().map(|h| h as u32);
    let d = v["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);
    Some((w, h, d))
}

/// Whether a frame is, to a viewer, nothing: near-uniform and dark. This is
/// what a phone's canvas returns for a clip it had not decoded yet, and five
/// of the first eight clips uploaded to the site were stored with exactly
/// that as their cover. Sampled on a stride so a 2048px still costs nothing.
pub fn is_flat(img: &DynamicImage) -> bool {
    let g = img.to_luma8();
    let (w, h) = g.dimensions();
    if w == 0 || h == 0 {
        return true;
    }
    let step = ((w * h) / 4096).max(1) as usize;
    let px: Vec<f64> = g
        .as_raw()
        .iter()
        .step_by(step)
        .map(|&v| f64::from(v))
        .collect();
    let n = px.len() as f64;
    let mean = px.iter().sum::<f64>() / n;
    let var = px.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
    var.sqrt() < 10.0 && !(18.0..=237.0).contains(&mean)
}

/// The frame of a GIF worth showing as its still: the one with the most
/// contrast among the first `MAX_FRAMES`, composited the way a player would
/// (each frame over the last). A caption card or a fade-in at frame zero is
/// exactly what this is for.
pub fn liveliest_gif_frame(bytes: &[u8]) -> Option<DynamicImage> {
    use image::codecs::gif::GifDecoder;
    use image::AnimationDecoder;

    const MAX_FRAMES: usize = 90;

    let decoder = GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let mut best: Option<(f64, image::RgbaImage)> = None;
    let mut canvas: Option<image::RgbaImage> = None;
    for frame in decoder.into_frames().take(MAX_FRAMES) {
        let Ok(frame) = frame else { break };
        let (left, top) = (frame.left(), frame.top());
        let buf = frame.into_buffer();
        let canvas = match canvas.as_mut() {
            Some(c) => {
                image::imageops::overlay(c, &buf, i64::from(left), i64::from(top));
                c
            }
            None => canvas.insert(buf),
        };
        let score = contrast(canvas);
        if best.as_ref().is_none_or(|(s, _)| score > *s + 1.0) {
            best = Some((score, canvas.clone()));
        }
    }
    best.map(|(_, img)| DynamicImage::ImageRgba8(img))
}

/// Standard deviation of luma over a stride, halved for a dark frame.
fn contrast(img: &image::RgbaImage) -> f64 {
    let (w, h) = img.dimensions();
    let step = ((w * h) / 4096).max(1) as usize;
    let lumas: Vec<f64> = img
        .pixels()
        .step_by(step)
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .collect();
    if lumas.is_empty() {
        return 0.0;
    }
    let n = lumas.len() as f64;
    let mean = lumas.iter().sum::<f64>() / n;
    let sd = (lumas.iter().map(|l| (l - mean) * (l - mean)).sum::<f64>() / n).sqrt();
    if mean < 28.0 {
        sd * 0.5
    } else {
        sd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatness_tells_black_from_pictures() {
        let black = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            36,
            image::Rgba([2, 2, 2, 255]),
        ));
        assert!(is_flat(&black));
        let white = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            36,
            image::Rgba([250, 250, 250, 255]),
        ));
        assert!(is_flat(&white));
        let mut pic = image::RgbaImage::from_pixel(64, 36, image::Rgba([30, 30, 30, 255]));
        for x in 0..32 {
            for y in 0..36 {
                pic.put_pixel(x, y, image::Rgba([220, 200, 40, 255]));
            }
        }
        assert!(!is_flat(&DynamicImage::ImageRgba8(pic)));
        // Mid-grey but flat is not "nothing" -- a grey card is a picture.
        let grey = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([128, 128, 128, 255]),
        ));
        assert!(!is_flat(&grey));
    }

    #[test]
    fn gif_frame_choice_skips_the_black_opener() {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, RgbaImage};
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            let black = RgbaImage::from_pixel(16, 16, image::Rgba([0, 0, 0, 255]));
            let mut lively = RgbaImage::from_pixel(16, 16, image::Rgba([20, 20, 20, 255]));
            for x in 0..8 {
                for y in 0..16 {
                    lively.put_pixel(x, y, image::Rgba([240, 220, 60, 255]));
                }
            }
            let d = Delay::from_numer_denom_ms(100, 1);
            enc.encode_frames(vec![
                Frame::from_parts(black, 0, 0, d),
                Frame::from_parts(lively, 0, 0, d),
            ])
            .unwrap();
        }
        let best = liveliest_gif_frame(&out).expect("a frame");
        assert!(!is_flat(&best));
        assert!(liveliest_gif_frame(b"not a gif").is_none());
    }
}
