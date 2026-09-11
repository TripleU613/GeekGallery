//! Draw a flavor's brand art from two colours, at every size the shell asks
//! for.
//!
//! A gallery that has no logo yet still needs a favicon, an apple-touch-icon,
//! a manifest icon and a link-preview image, and the same six files at the
//! same names is what `Flavor::asset` expects wherever they end up. This
//! draws a plain geometric mark -- a rounded tile in the accent with a 2x2
//! gallery grid cut out of it -- so a fresh flavor is presentable before
//! anyone has opened an image editor. The placeholder art under
//! `public/brand/` is this program's output for the default palette.
//!
//!   cargo run --example brand_art --features ssr -- \
//!       --accent '#9aa4ff' --bg '#0a0b0f' --out public/brand
//!
//! Then upload the folder to the flavor's media bucket and point
//! `SITE_ASSETS_BASE` at it, or leave the default and let `public/` serve it.
//!
//! Drawn at 4x and downsampled, which is the whole anti-aliasing strategy;
//! nothing here needs a font, which is also why the default header prints the
//! site's name in text beside the mark rather than baking it into a PNG.

use image::imageops::{resize, FilterType};
use image::{Rgba, RgbaImage};

fn main() {
    let mut accent = "#9aa4ff".to_string();
    let mut bg = "#0a0b0f".to_string();
    let mut out = "public/brand".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--accent" => accent = args.next().expect("--accent needs a colour"),
            "--bg" => bg = args.next().expect("--bg needs a colour"),
            "--out" => out = args.next().expect("--out needs a directory"),
            _ => panic!("unknown argument {a}; see the file header for usage"),
        }
    }
    let accent = geekgallery::flavor::parse_hex(&accent).expect("accent is #rrggbb");
    let bg = geekgallery::flavor::parse_hex(&bg).expect("bg is #rrggbb");
    std::fs::create_dir_all(&out).expect("create the output directory");

    for (file, size) in [
        ("favicon-32.png", 32),
        ("favicon-192.png", 192),
        ("favicon-512.png", 512),
        ("apple-touch-icon.png", 180),
        // The header shows this at 28-32px tall; 2x for retina.
        ("logo.png", 192),
    ] {
        let img = mark(size, accent, bg, size == 180);
        img.save(format!("{out}/{file}")).expect("write the icon");
    }

    // The link-preview image: 1200x630, the mark centred on the background.
    let mut card = RgbaImage::from_pixel(1200, 630, Rgba([bg[0], bg[1], bg[2], 255]));
    let m = mark(360, accent, bg, false);
    image::imageops::overlay(&mut card, &m, (1200 - 360) / 2, (630 - 360) / 2);
    card.save(format!("{out}/logo-large.png"))
        .expect("write the preview image");
    println!("brand art written to {out}/");
}

/// The mark at `size` px. `opaque` fills the corners with the background
/// (Apple's touch icons are shown square, and a transparent corner comes out
/// black there).
fn mark(size: u32, accent: [u8; 3], bg: [u8; 3], opaque: bool) -> RgbaImage {
    const SCALE: u32 = 4;
    let s = (size * SCALE) as f32;
    let hi = RgbaImage::from_fn(size * SCALE, size * SCALE, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        // The tile: a rounded square inset 4% with a 22% corner radius.
        let inset = s * 0.04;
        let tile = rounded_rect(px, py, inset, inset, s - inset, s - inset, s * 0.22);
        // The grid: four rounded squares cut out of it, on a 2x2 with a gutter.
        let cell = s * 0.26;
        let gutter = s * 0.08;
        let start = (s - (2.0 * cell + gutter)) / 2.0;
        let mut hole = false;
        for i in 0..2 {
            for j in 0..2 {
                let x0 = start + i as f32 * (cell + gutter);
                let y0 = start + j as f32 * (cell + gutter);
                hole |= rounded_rect(px, py, x0, y0, x0 + cell, y0 + cell, cell * 0.28);
            }
        }
        match (tile, hole, opaque) {
            (true, false, _) => Rgba([accent[0], accent[1], accent[2], 255]),
            (true, true, _) | (false, _, true) => Rgba([bg[0], bg[1], bg[2], 255]),
            (false, _, false) => Rgba([0, 0, 0, 0]),
        }
    });
    resize(&hi, size, size, FilterType::Lanczos3)
}

/// Whether the point is inside the rounded rectangle.
fn rounded_rect(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    if px < x0 || px > x1 || py < y0 || py > y1 {
        return false;
    }
    let cx = px.clamp(x0 + r, x1 - r);
    let cy = py.clamp(y0 + r, y1 - r);
    (px - cx).powi(2) + (py - cy).powi(2) <= r * r
}
