//! PURE image-paste decisions: caps/downsample policy + attachment path/snippet.
//! No GTK, no clipboard, no I/O. The GTK clipboard read + PNG encode glue lives
//! in `platform::clipboard_image` (Plan 6 Task 2).

/// Caps from config: max output pixel count (w*h), max output bytes,
/// and whether oversized images may be downsampled (vs rejected).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageCaps {
    pub max_pixels: u64,
    pub max_bytes: u64,
    pub downsample: bool,
}

/// Decision for a pasted image given its decoded dimensions + encoded byte size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageAction {
    /// Within caps — store as-is.
    Accept,
    /// Over the pixel cap but downsampling is allowed: scale BOTH dimensions by
    /// this integer divisor (>= 2) so w*h falls within `max_pixels`.
    Downsample(u32),
    /// Over caps and downsampling disallowed, or over the byte cap.
    Reject,
}

/// Pixel count for w*h as u64 (no overflow for realistic image sizes).
fn pixels(w: u32, h: u32) -> u64 {
    w as u64 * h as u64
}

/// Smallest integer factor >= 2 so (w/f)*(h/f) <= max_pixels.
/// Caps the search at 64 (a 64x reduction is already absurd) to stay bounded.
fn smallest_fitting_factor(w: u32, h: u32, max_pixels: u64) -> Option<u32> {
    (2u32..=64).find(|&f| pixels(w / f, h / f) <= max_pixels)
}

/// PURE: decide what to do with a pasted image.
/// - bytes > max_bytes               -> Reject (byte cap is hard).
/// - w*h <= max_pixels               -> Accept.
/// - w*h >  max_pixels && downsample -> Downsample(factor).
/// - w*h >  max_pixels && !downsample-> Reject.
pub fn image_action(w: u32, h: u32, bytes: u64, caps: ImageCaps) -> ImageAction {
    if bytes > caps.max_bytes {
        return ImageAction::Reject;
    }
    if pixels(w, h) <= caps.max_pixels {
        return ImageAction::Accept;
    }
    if !caps.downsample {
        return ImageAction::Reject;
    }
    match smallest_fitting_factor(w, h, caps.max_pixels) {
        Some(f) => ImageAction::Downsample(f),
        None => ImageAction::Reject,
    }
}

/// PURE: relative attachment path for a note id + timestamp: forward-slash,
/// always `attachments/<id>/<ts>.png`.
pub fn attachment_rel_path(id: &str, ts: &str) -> String {
    format!("attachments/{id}/{ts}.png")
}

/// PURE: the markdown image snippet for a relative attachment path:
/// `![](attachments/<id>/<ts>.png)`.
pub fn attachment_markdown(rel: &str) -> String {
    format!("![]({rel})")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: ImageCaps = ImageCaps { max_pixels: 4_000_000, max_bytes: 5_242_880, downsample: true };

    #[test]
    fn within_pixel_and_byte_caps_is_accepted() {
        assert_eq!(image_action(1000, 1000, 500_000, CAPS), ImageAction::Accept);
    }

    #[test]
    fn over_byte_cap_is_rejected_even_when_pixels_ok() {
        assert_eq!(image_action(1000, 1000, 6_000_000, CAPS), ImageAction::Reject);
    }

    #[test]
    fn over_pixel_cap_with_downsample_picks_smallest_factor_that_fits() {
        // 4000*4000 = 16_000_000 px. factor 2 -> 2000*2000 = 4_000_000 <= cap. ok.
        assert_eq!(image_action(4000, 4000, 1_000, CAPS), ImageAction::Downsample(2));
    }

    #[test]
    fn far_over_pixel_cap_picks_larger_factor() {
        // 8000*8000 = 64M. f2 -> 16M (no). f3 -> ~7.1M (no). f4 -> 4M (yes).
        assert_eq!(image_action(8000, 8000, 1_000, CAPS), ImageAction::Downsample(4));
    }

    #[test]
    fn over_pixel_cap_with_downsample_disabled_is_rejected() {
        let caps = ImageCaps { downsample: false, ..CAPS };
        assert_eq!(image_action(4000, 4000, 1_000, caps), ImageAction::Reject);
    }

    #[test]
    fn attachment_rel_path_is_forward_slash_png() {
        assert_eq!(
            attachment_rel_path("01JZ9P6S0R8ZX0G8N3Z4V7Y8QK", "20260624-153012-001"),
            "attachments/01JZ9P6S0R8ZX0G8N3Z4V7Y8QK/20260624-153012-001.png"
        );
    }

    #[test]
    fn attachment_markdown_wraps_rel_path() {
        assert_eq!(
            attachment_markdown("attachments/ID/TS.png"),
            "![](attachments/ID/TS.png)"
        );
    }

    /// Verify that the snippet resolves to the actual on-disk file when joined
    /// against notes_dir (i.e. notes_dir.join(snippet) == attachments_dir(id)/ts.png).
    /// This is the contract: renderer uses notes_dir as base_dir, so images resolve.
    #[test]
    fn snippet_resolves_via_notes_dir_to_stored_file() {
        use std::path::PathBuf;
        let notes_dir = PathBuf::from("/home/user/.local/share/waynote/notes");
        let id = "ABC123";
        let ts = "20260624-120000-001";
        let snippet = attachment_rel_path(id, ts);
        // notes_dir/snippet must equal notes_dir/attachments/<id>/<ts>.png
        let resolved = notes_dir.join(&snippet);
        let expected = notes_dir.join("attachments").join(id).join(format!("{ts}.png"));
        assert_eq!(resolved, expected);
    }
}
