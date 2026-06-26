//! Image paste glue: PURE downsample/encode + store-to-attachments I/O.
//!
//! ## BGRA → RGBA swizzle (critical)
//! `gdk::Texture::download` fills the buffer in **BGRA** byte order (Cairo's
//! ARGB32 in-memory layout: on little-endian x86/x64 the 32-bit word [0xAARRGGBB]
//! is stored as bytes [BB, GG, RR, AA] — i.e. BGRA). The `image` crate expects
//! **RGBA**. Without `bgra_to_rgba` the red and blue channels are swapped.
//!
//! All functions except `texture_to_raw` are unit-testable without a display.

use std::path::Path;

use crate::core::image_paste::{
    attachment_markdown, attachment_rel_path, image_action, ImageAction, ImageCaps,
};

/// Decoded clipboard image: raw RGBA8 pixels + dimensions.
/// Built from a `gdk::Texture` by `texture_to_raw`; PURE helpers operate on this.
pub struct RawImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8 pixels: width * height * 4 bytes.
    pub rgba: Vec<u8>,
}

/// PURE: swizzle BGRA → RGBA in place (swap bytes 0 and 2 in every 4-byte group).
///
/// Required after `gdk::Texture::download`, which fills in BGRA order (Cairo
/// ARGB32 memory layout). Without this step R and B are swapped in the PNG.
pub fn bgra_to_rgba(buf: &mut [u8]) {
    for chunk in buf.chunks_exact_mut(4) {
        chunk.swap(0, 2); // B ↔ R; G and A stay in place
    }
}

/// PURE: downsample `img` by integer `factor` (>= 1) using a box filter.
pub fn downsample(img: &RawImage, factor: u32) -> RawImage {
    let factor = factor.max(1);
    let src = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone())
        .expect("rgba buffer size matches dimensions");
    let nw = (img.width / factor).max(1);
    let nh = (img.height / factor).max(1);
    let out = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Triangle);
    RawImage { width: out.width(), height: out.height(), rgba: out.into_raw() }
}

/// PURE: encode a `RawImage` as PNG bytes.
pub fn encode_png(img: &RawImage) -> Vec<u8> {
    let buf = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone())
        .expect("rgba buffer size matches dimensions");
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("PNG encode of an in-memory RGBA buffer cannot fail");
    cursor.into_inner()
}

/// I/O glue: run the caps decision, optionally downsample, encode PNG, write
/// `<attachments_base>/<ts>.png`, and return the relative markdown snippet —
/// or `Ok(None)` on Reject (over caps).
///
/// `attachments_base` is `paths.attachments_dir(id)`. The returned snippet uses
/// the relative path `attachments/<id>/<ts>.png` (resolved against the note's
/// base_dir in the renderer).
///
/// Byte-cap check is evaluated against the **encoded PNG size** (per plan spec).
pub fn store_pasted_image(
    img: &RawImage,
    caps: ImageCaps,
    id: &str,
    ts: &str,
    attachments_base: &Path,
) -> std::io::Result<Option<String>> {
    // 1. Pixel-cap decision + optional downsample (bytes=0: ignore byte cap here).
    let prepared = match image_action(img.width, img.height, 0, caps) {
        ImageAction::Reject => return Ok(None),
        ImageAction::Accept => encode_png(img),
        ImageAction::Downsample(f) => encode_png(&downsample(img, f)),
    };
    // 2. Byte-cap decision against the ENCODED PNG size.
    if prepared.len() as u64 > caps.max_bytes {
        return Ok(None);
    }
    // 3. Write the file.
    std::fs::create_dir_all(attachments_base)?;
    std::fs::write(attachments_base.join(format!("{ts}.png")), &prepared)?;
    // 4. Return the relative snippet.
    let rel = attachment_rel_path(id, ts);
    Ok(Some(attachment_markdown(&rel)))
}

/// DISPLAY (manual): convert a `gdk::Texture` to a `RawImage` with RGBA8 pixels.
///
/// `gdk::Texture::download` fills bytes in **BGRA** order (Cairo ARGB32 memory).
/// This function applies `bgra_to_rgba` to fix the channel order before returning.
/// Without this step, the stored PNG would have R and B swapped.
///
/// Display-bound — cannot be unit-tested headless. Verified manually (see plan
/// Task 2 Step 9).
pub fn texture_to_raw(texture: &gtk::gdk::Texture) -> RawImage {
    use gtk::prelude::{TextureExt, TextureExtManual};
    let w = texture.width() as u32;
    let h = texture.height() as u32;
    // Cairo ARGB32 in memory = BGRA on little-endian.
    let mut buf = vec![0u8; (w * h * 4) as usize];
    texture.download(&mut buf, (w * 4) as usize);
    // Swizzle B↔R so the buffer is RGBA before PNG encoding.
    bgra_to_rgba(&mut buf);
    RawImage { width: w, height: h, rgba: buf }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::image_paste::ImageCaps;

    fn solid(w: u32, h: u32) -> RawImage {
        RawImage { width: w, height: h, rgba: vec![200u8; (w * h * 4) as usize] }
    }

    #[test]
    fn bgra_to_rgba_swaps_b_and_r_leaves_g_and_a() {
        // Distinct values: B=10, G=20, R=30, A=40 (BGRA order from GDK)
        let mut pixel = [10u8, 20, 30, 40];
        bgra_to_rgba(&mut pixel);
        // After swizzle: R is now at [0], G at [1], B at [2], A at [3]
        assert_eq!(pixel[0], 30, "R should be at index 0 after BGRA→RGBA");
        assert_eq!(pixel[1], 20, "G unchanged at index 1");
        assert_eq!(pixel[2], 10, "B should be at index 2 after BGRA→RGBA");
        assert_eq!(pixel[3], 40, "A unchanged at index 3");
    }

    #[test]
    fn bgra_to_rgba_multiple_pixels() {
        // Two pixels: [B=1,G=2,R=3,A=4] [B=5,G=6,R=7,A=8]
        let mut buf = [1u8, 2, 3, 4, 5, 6, 7, 8];
        bgra_to_rgba(&mut buf);
        // First pixel: [R=3,G=2,B=1,A=4]
        assert_eq!(&buf[0..4], &[3, 2, 1, 4]);
        // Second pixel: [R=7,G=6,B=5,A=8]
        assert_eq!(&buf[4..8], &[7, 6, 5, 8]);
    }

    #[test]
    fn downsample_by_two_halves_each_dimension() {
        let small = downsample(&solid(8, 6), 2);
        assert_eq!((small.width, small.height), (4, 3));
        assert_eq!(small.rgba.len() as u32, small.width * small.height * 4);
    }

    #[test]
    fn encode_png_emits_png_magic_bytes() {
        let png = encode_png(&solid(2, 2));
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[test]
    fn encode_png_color_correctness_no_rb_swap() {
        // A 1x1 image with a known RGBA pixel: R=200, G=100, B=50, A=255
        let img = RawImage {
            width: 1,
            height: 1,
            rgba: vec![200, 100, 50, 255],
        };
        let png_bytes = encode_png(&img);
        // Decode back via image crate and verify R/B are not swapped.
        let decoded = image::load_from_memory_with_format(&png_bytes, image::ImageFormat::Png)
            .expect("PNG decode must succeed")
            .into_rgba8();
        let pixel = decoded.get_pixel(0, 0);
        assert_eq!(pixel[0], 200, "R channel must be 200, not swapped with B");
        assert_eq!(pixel[1], 100, "G unchanged");
        assert_eq!(pixel[2], 50,  "B channel must be 50, not swapped with R");
        assert_eq!(pixel[3], 255, "A unchanged");
    }

    #[test]
    fn store_writes_png_and_returns_relative_snippet() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("attachments").join("ID");
        let caps = ImageCaps { max_pixels: 4_000_000, max_bytes: 5_242_880, downsample: true };
        let snippet = store_pasted_image(&solid(4, 4), caps, "ID", "TS", &base)
            .unwrap()
            .expect("accepted");
        assert_eq!(snippet, "![](attachments/ID/TS.png)");
        assert!(base.join("TS.png").exists());
    }

    #[test]
    fn store_rejects_over_byte_cap_returns_none_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("attachments").join("ID");
        let caps = ImageCaps { max_pixels: 4_000_000, max_bytes: 4, downsample: true };
        let out = store_pasted_image(&solid(4, 4), caps, "ID", "TS", &base).unwrap();
        assert!(out.is_none(), "reject yields None");
        assert!(!base.join("TS.png").exists(), "rejected image is not written");
    }
}
