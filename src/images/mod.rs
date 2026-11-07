//! Raster images: decoding (with limits), EXIF metadata, pixel edits, colour
//! management for display, and encoding. Pixels are 8-bit RGBA, not premultiplied,
//! in the image's own colour space. See ADR 0007.

pub mod decode;
pub mod edit;
pub mod encode;
pub mod metadata;

use std::sync::Arc;

use crate::core::filetype::ImageFormat;

/// Points per image pixel: 100% zoom shows one image pixel per 96-DPI screen pixel.
pub const PT_PER_PX: f32 = 0.75;

/// Converts a length in points to whole pixels.
pub fn pt_to_px(pt: f32) -> u32 {
    ((pt / PT_PER_PX).round() as u32).max(1)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Display time for animation frames, in milliseconds.
    pub delay_ms: u32,
}

impl Frame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Frame {
        debug_assert_eq!(rgba.len(), width as usize * height as usize * 4);
        Frame { width, height, rgba, delay_ms: 0 }
    }

    pub fn has_alpha(&self) -> bool {
        self.rgba.as_chunks::<4>().0.iter().any(|p| p[3] != 255)
    }

    /// Pixel (r, g, b, a) at (x, y).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// What the decoder learned about the file besides its pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageInfo {
    pub format: Option<ImageFormat>,
    /// Embedded ICC profile.
    pub icc: Option<Vec<u8>>,
    /// Raw EXIF (TIFF structure, starting with "II*\0" or "MM\0*").
    pub exif: Option<Vec<u8>>,
    /// Resolution in pixels per inch.
    pub dpi: (f32, f32),
    pub bits_per_channel: u8,
    pub color_model: String,
    pub has_alpha: bool,
}

pub struct Decoded {
    pub frames: Vec<Frame>,
    pub info: ImageInfo,
}

/// Converts rendered display pixels (RGBA) for the screen: optional colour adjustment
/// preview, then the embedded profile to sRGB.
#[derive(Clone, Default)]
pub struct DisplayFilter {
    pub adjust: Option<edit::ColorAdjust>,
    pub icc: Option<Arc<moxcms::Transform8BitExecutor>>,
}

impl DisplayFilter {
    pub fn is_empty(&self) -> bool {
        self.adjust.is_none() && self.icc.is_none()
    }

    pub fn apply(&self, rgba: &mut [u8], width: u32, height: u32) {
        if let Some(a) = &self.adjust {
            a.apply(rgba, width, height);
        }
        if let Some(t) = &self.icc {
            let src = rgba.to_vec();
            let _ = t.transform(&src, rgba);
        }
    }
}

/// A transform from an embedded ICC profile to sRGB, or None when the profile is
/// sRGB-like, unreadable or not RGB.
pub fn display_transform(icc: &[u8]) -> Option<Arc<moxcms::Transform8BitExecutor>> {
    let src = moxcms::ColorProfile::new_from_slice(icc).ok()?;
    if src.color_space != moxcms::DataColorSpace::Rgb {
        return None;
    }
    let srgb = moxcms::ColorProfile::new_srgb();
    if src.red_colorant == srgb.red_colorant
        && src.green_colorant == srgb.green_colorant
        && src.blue_colorant == srgb.blue_colorant
    {
        return None;
    }
    src.create_transform_8bit(moxcms::Layout::Rgba, &srgb, moxcms::Layout::Rgba, Default::default()).ok()
}

/// A short human-readable name for an ICC profile (its description tag).
pub fn profile_name(icc: &[u8]) -> String {
    let Ok(p) = moxcms::ColorProfile::new_from_slice(icc) else { return "Embedded profile".into() };
    match &p.description {
        Some(moxcms::ProfileText::PlainString(s)) => s.clone(),
        Some(moxcms::ProfileText::Localizable(v)) => {
            v.first().map(|l| l.value.clone()).unwrap_or_else(|| "Embedded profile".into())
        }
        Some(moxcms::ProfileText::Description(d)) if !d.ascii_string.is_empty() => d.ascii_string.clone(),
        _ => "Embedded profile".into(),
    }
}

/// sRGB (0..=255) to linear light (0..=1).
pub fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Linear light to sRGB, clamped.
pub fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let c = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trip() {
        for v in 0..=255u8 {
            assert_eq!(linear_to_srgb(srgb_to_linear(v)), v);
        }
    }

    #[test]
    fn srgb_profile_needs_no_transform() {
        let srgb = moxcms::ColorProfile::new_srgb().encode().unwrap();
        assert!(display_transform(&srgb).is_none());
        let p3 = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let t = display_transform(&p3).expect("P3 converts");
        // Pure P3 red is outside sRGB: it clips to (255, 0, 0)-ish after conversion.
        let src = [255u8, 0, 0, 255];
        let mut dst = [0u8; 4];
        t.transform(&src, &mut dst).unwrap();
        assert!(dst[0] == 255 && dst[1] < 30 && dst[2] < 30, "{dst:?}");
        assert_eq!(dst[3], 255);
    }
}
