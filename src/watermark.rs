//! An invisible provenance watermark: a fixed marker string plus this item's
//! own id, embedded into the low bit of each RGBA channel across the image.
//! Lossless formats (this site stores PNG originals) preserve it exactly --
//! it survives re-saving, but not a lossy re-compression or a resize, which
//! is the expected tradeoff for a mark that must not be visible at all.
//!
//! This is steganographic marking for provenance, not DRM: nothing here
//! stops a determined actor from stripping it. The goal is that an
//! unmodified copy of a item, found elsewhere, can be traced back to the
//! exact upload it came from.

use image::{DynamicImage, RgbaImage};

/// Bits needed to store a payload's byte length, ahead of the payload
/// itself -- lets `extract` know where the real data ends instead of
/// reading noise as more payload.
const LENGTH_BITS: usize = 32;

/// Embeds `payload` into the low bit of every RGBA channel byte, starting
/// with a 32-bit big-endian length prefix. Returns the original image
/// unchanged if it has too few pixels to carry the payload -- silently
/// skipping the watermark is preferable to failing an otherwise-good upload
/// over it.
pub fn embed(img: &DynamicImage, payload: &[u8]) -> DynamicImage {
    let mut buf: RgbaImage = img.to_rgba8();
    let bits_needed = LENGTH_BITS + payload.len() * 8;
    let capacity_bits = buf.as_raw().len();

    if bits_needed > capacity_bits {
        tracing::warn!(
            "watermark payload ({} bytes) does not fit in a {}x{} image; skipping",
            payload.len(),
            buf.width(),
            buf.height()
        );
        return img.clone();
    }

    let len_bytes = (payload.len() as u32).to_be_bytes();
    let len_bits = bit_iter(&len_bytes);
    let payload_bits = bit_iter(payload);

    for (byte, bit) in buf.as_mut().iter_mut().zip(len_bits.chain(payload_bits)) {
        *byte = (*byte & !1) | bit;
    }

    DynamicImage::ImageRgba8(buf)
}

/// Inverse of `embed`. Returns `None` if the embedded length is absurd (a
/// strong sign this image was never watermarked, or was watermarked with a
/// different scheme) rather than trying to read gigabytes of "payload" out
/// of ordinary pixel noise.
pub fn extract(img: &DynamicImage) -> Option<Vec<u8>> {
    let buf = img.to_rgba8();
    let bytes = buf.as_raw();

    if bytes.len() < LENGTH_BITS {
        return None;
    }

    let len_bits = &bytes[..LENGTH_BITS];
    let len = u32::from_be_bytes(bits_to_bytes(len_bits).try_into().ok()?) as usize;

    let payload_bits_needed = len * 8;
    if LENGTH_BITS + payload_bits_needed > bytes.len() {
        return None;
    }

    let payload_bits = &bytes[LENGTH_BITS..LENGTH_BITS + payload_bits_needed];
    Some(bits_to_bytes(payload_bits))
}

fn bit_iter(bytes: &[u8]) -> impl Iterator<Item = u8> + '_ {
    bytes
        .iter()
        .flat_map(|b| (0..8).rev().map(move |i| (b >> i) & 1))
}

fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| chunk.iter().fold(0u8, |acc, &bit| (acc << 1) | (bit & 1)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([120, 90, 200, 255]),
        ))
    }

    #[test]
    fn round_trips_a_short_payload() {
        let img = blank(64, 64);
        let marked = embed(&img, b"item-collection:v1:abc-123");
        assert_eq!(
            extract(&marked).as_deref(),
            Some(&b"item-collection:v1:abc-123"[..])
        );
    }

    #[test]
    fn is_invisible_at_full_byte_resolution() {
        // The LSB flip can move a channel value by at most 1 -- nowhere near
        // perceptible, and no channel should ever change by more than that.
        let img = blank(64, 64);
        let marked = embed(&img, b"item-collection:v1:abc-123");
        let (orig, wm) = (img.to_rgba8(), marked.to_rgba8());
        for (a, b) in orig.as_raw().iter().zip(wm.as_raw().iter()) {
            assert!(a.abs_diff(*b) <= 1);
        }
    }

    #[test]
    fn skips_rather_than_panics_when_payload_does_not_fit() {
        let img = blank(2, 2); // 2*2*4 = 16 bits of capacity
        let huge_payload = vec![0u8; 100];
        let marked = embed(&img, &huge_payload);
        assert_eq!(marked.to_rgba8(), img.to_rgba8());
    }

    #[test]
    fn extract_on_an_unmarked_image_does_not_panic() {
        let img = blank(8, 8);
        // May return Some(garbage) or None depending on what the "length
        // prefix" happens to decode to from blank pixel data -- the only
        // real contract is that it never panics.
        let _ = extract(&img);
    }
}
