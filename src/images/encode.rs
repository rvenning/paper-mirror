//! Encoding frames to image files. See ADR 0007 for which encoder handles what.

use std::io::Cursor;

use image::ImageEncoder;

use super::Frame;
use crate::core::filetype::ImageFormat;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFormat {
    Png,
    Jpeg,
    Heic,
    Tiff,
    Gif,
    WebP,
    Bmp,
    Exr,
    Ico,
}

impl SaveFormat {
    pub const ALL: [SaveFormat; 9] = [
        SaveFormat::Png,
        SaveFormat::Jpeg,
        SaveFormat::Heic,
        SaveFormat::Tiff,
        SaveFormat::Gif,
        SaveFormat::WebP,
        SaveFormat::Bmp,
        SaveFormat::Exr,
        SaveFormat::Ico,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SaveFormat::Png => "PNG",
            SaveFormat::Jpeg => "JPEG",
            SaveFormat::Heic => "HEIC",
            SaveFormat::Tiff => "TIFF",
            SaveFormat::Gif => "GIF",
            SaveFormat::WebP => "WebP (lossless)",
            SaveFormat::Bmp => "BMP",
            SaveFormat::Exr => "OpenEXR",
            SaveFormat::Ico => "ICO",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            SaveFormat::Png => "png",
            SaveFormat::Jpeg => "jpg",
            SaveFormat::Heic => "heic",
            SaveFormat::Tiff => "tiff",
            SaveFormat::Gif => "gif",
            SaveFormat::WebP => "webp",
            SaveFormat::Bmp => "bmp",
            SaveFormat::Exr => "exr",
            SaveFormat::Ico => "ico",
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            SaveFormat::Png => &["png"],
            SaveFormat::Jpeg => &["jpg", "jpeg", "jpe"],
            SaveFormat::Heic => &["heic", "heif"],
            SaveFormat::Tiff => &["tiff", "tif"],
            SaveFormat::Gif => &["gif"],
            SaveFormat::WebP => &["webp"],
            SaveFormat::Bmp => &["bmp"],
            SaveFormat::Exr => &["exr"],
            SaveFormat::Ico => &["ico"],
        }
    }

    pub fn from_extension(ext: &str) -> Option<SaveFormat> {
        let ext = ext.to_ascii_lowercase();
        SaveFormat::ALL.into_iter().find(|f| f.extensions().contains(&ext.as_str()))
    }

    /// The format to save an opened image back to, if it can be written.
    pub fn for_source(f: ImageFormat) -> Option<SaveFormat> {
        Some(match f {
            ImageFormat::Png => SaveFormat::Png,
            ImageFormat::Jpeg => SaveFormat::Jpeg,
            ImageFormat::Gif => SaveFormat::Gif,
            ImageFormat::Tiff => SaveFormat::Tiff,
            ImageFormat::Bmp => SaveFormat::Bmp,
            ImageFormat::WebP => SaveFormat::WebP,
            ImageFormat::Ico => SaveFormat::Ico,
            ImageFormat::OpenExr => SaveFormat::Exr,
            ImageFormat::Heic => SaveFormat::Heic,
            ImageFormat::Jpeg2000 => return None,
        })
    }

    pub fn has_quality(self) -> bool {
        matches!(self, SaveFormat::Jpeg | SaveFormat::Heic)
    }

    pub fn keeps_all_frames(self) -> bool {
        matches!(self, SaveFormat::Gif | SaveFormat::Tiff)
    }

    pub fn keeps_alpha(self) -> bool {
        !matches!(self, SaveFormat::Jpeg)
    }
}

#[derive(Clone, Debug)]
pub struct EncodeOptions {
    /// 1..=100 for JPEG and HEIC.
    pub quality: u8,
    pub icc: Option<Vec<u8>>,
    /// Raw EXIF (TIFF structure), already rewritten for saving.
    pub exif: Option<Vec<u8>>,
    pub dpi: (f32, f32),
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions { quality: 90, icc: None, exif: None, dpi: (72.0, 72.0) }
    }
}

/// An image file to write from a document: every page of an image document at its
/// own pixels (`page: None`), or one page of a PDF at `dpi`.
#[derive(Clone, Debug)]
pub struct ImageTarget {
    pub format: SaveFormat,
    pub page: Option<usize>,
    pub options: EncodeOptions,
    /// Frame delays (animation), by page.
    pub delays: Vec<u32>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Encodes `frames` (all of them for GIF and TIFF, else the first).
pub fn encode(frames: &[Frame], format: SaveFormat, opts: &EncodeOptions) -> Result<Vec<u8>, String> {
    let first = frames.first().ok_or("There is nothing to save.")?;
    let mut out = Vec::new();
    match format {
        SaveFormat::Png => encode_png(first, opts, &mut out)?,
        SaveFormat::Jpeg => {
            let rgb = flatten_on_white(first);
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, opts.quality.clamp(1, 100));
            enc.set_pixel_density(image::codecs::jpeg::PixelDensity {
                density: (opts.dpi.0.round().clamp(1.0, 65535.0) as u16, opts.dpi.1.round().clamp(1.0, 65535.0) as u16),
                unit: image::codecs::jpeg::PixelDensityUnit::Inches,
            });
            if let Some(icc) = &opts.icc {
                let _ = enc.set_icc_profile(icc.clone());
            }
            if let Some(exif) = &opts.exif {
                let _ = enc.set_exif_metadata(exif.clone());
            }
            enc.write_image(&rgb, first.width, first.height, image::ExtendedColorType::Rgb8).map_err(err)?;
        }
        SaveFormat::Gif => {
            let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 10);
            if frames.len() > 1 {
                enc.set_repeat(image::codecs::gif::Repeat::Infinite).map_err(err)?;
            }
            let (w, h) = (first.width, first.height);
            for f in frames {
                // GIF frames share one canvas; other sizes are scaled to fit it.
                let f = if (f.width, f.height) != (w, h) { super::edit::resize(f, w, h) } else { f.clone() };
                let buf = image::RgbaImage::from_raw(f.width, f.height, f.rgba).ok_or("bad frame")?;
                let delay = image::Delay::from_numer_denom_ms(f.delay_ms.max(20), 1);
                enc.encode_frame(image::Frame::from_parts(buf, 0, 0, delay)).map_err(err)?;
            }
        }
        SaveFormat::Tiff => encode_tiff(frames, opts, &mut out)?,
        SaveFormat::WebP => {
            let mut enc = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
            if let Some(icc) = &opts.icc {
                let _ = enc.set_icc_profile(icc.clone());
            }
            if let Some(exif) = &opts.exif {
                let _ = enc.set_exif_metadata(exif.clone());
            }
            enc.write_image(&first.rgba, first.width, first.height, image::ExtendedColorType::Rgba8).map_err(err)?;
        }
        SaveFormat::Bmp => {
            let mut c = Cursor::new(&mut out);
            let mut enc = image::codecs::bmp::BmpEncoder::new(&mut c);
            if first.has_alpha() {
                enc.encode(&first.rgba, first.width, first.height, image::ExtendedColorType::Rgba8).map_err(err)?;
            } else {
                let rgb = flatten_on_white(first);
                enc.encode(&rgb, first.width, first.height, image::ExtendedColorType::Rgb8).map_err(err)?;
            }
        }
        SaveFormat::Ico => {
            if first.width > 256 || first.height > 256 {
                return Err("ICO icons can be at most 256 × 256 pixels. Use Tools > Adjust Size first.".into());
            }
            image::codecs::ico::IcoEncoder::new(&mut out)
                .write_image(&first.rgba, first.width, first.height, image::ExtendedColorType::Rgba8)
                .map_err(err)?;
        }
        SaveFormat::Exr => {
            let linear: Vec<f32> = first
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    [
                        super::srgb_to_linear(p[0]),
                        super::srgb_to_linear(p[1]),
                        super::srgb_to_linear(p[2]),
                        p[3] as f32 / 255.0,
                    ]
                })
                .collect();
            let img = image::Rgba32FImage::from_raw(first.width, first.height, linear).ok_or("bad frame")?;
            image::DynamicImage::ImageRgba32F(img)
                .write_to(&mut Cursor::new(&mut out), image::ImageFormat::OpenExr)
                .map_err(err)?;
        }
        SaveFormat::Heic => {
            out = crate::platform_windows::wic::encode_heic(first, opts.quality)?;
        }
    }
    Ok(out)
}

fn encode_png(f: &Frame, opts: &EncodeOptions, out: &mut Vec<u8>) -> Result<(), String> {
    let alpha = f.has_alpha();
    let mut info = png::Info::with_size(f.width, f.height);
    info.color_type = if alpha { png::ColorType::Rgba } else { png::ColorType::Rgb };
    info.bit_depth = png::BitDepth::Eight;
    info.pixel_dims = Some(png::PixelDimensions {
        xppu: (opts.dpi.0 / 0.0254).round() as u32,
        yppu: (opts.dpi.1 / 0.0254).round() as u32,
        unit: png::Unit::Meter,
    });
    info.icc_profile = opts.icc.as_deref().map(std::borrow::Cow::Borrowed);
    info.exif_metadata = opts.exif.as_deref().map(std::borrow::Cow::Borrowed);
    let enc = png::Encoder::with_info(&mut *out, info).map_err(err)?;
    let mut w = enc.write_header().map_err(err)?;
    if alpha {
        w.write_image_data(&f.rgba).map_err(err)?;
    } else {
        let rgb: Vec<u8> = f.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        w.write_image_data(&rgb).map_err(err)?;
    }
    w.finish().map_err(err)
}

fn encode_tiff(frames: &[Frame], opts: &EncodeOptions, out: &mut Vec<u8>) -> Result<(), String> {
    use tiff::encoder::{Rational, TiffEncoder, colortype};
    let mut c = Cursor::new(out);
    let mut enc = TiffEncoder::new(&mut c)
        .map_err(err)?
        .with_compression(tiff::encoder::Compression::Deflate(Default::default()));
    for f in frames {
        let mut img = enc.new_image::<colortype::RGBA8>(f.width, f.height).map_err(err)?;
        img.resolution(tiff::tags::ResolutionUnit::Inch, Rational { n: opts.dpi.0.round().max(1.0) as u32, d: 1 });
        if let Some(icc) = &opts.icc {
            img.encoder().write_tag(tiff::tags::Tag::IccProfile, &icc[..]).map_err(err)?;
        }
        img.write_data(&f.rgba).map_err(err)?;
    }
    Ok(())
}

/// RGB with transparency composited over white (for formats without alpha).
pub fn flatten_on_white(f: &Frame) -> Vec<u8> {
    f.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as u32;
            let mix = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            [mix(p[0]), mix(p[1]), mix(p[2])]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::decode::decode;

    fn sample(w: u32, h: u32) -> Frame {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[
                    (x * 9) as u8,
                    (y * 13) as u8,
                    ((x + y) * 5) as u8,
                    if x == 0 { 0 } else { 255 },
                ]);
            }
        }
        Frame::new(w, h, v)
    }

    #[test]
    fn lossless_formats_round_trip_exactly() {
        let f = sample(17, 11);
        let opts = EncodeOptions { dpi: (144.0, 144.0), ..Default::default() };
        for (fmt, src) in [
            (SaveFormat::Png, ImageFormat::Png),
            (SaveFormat::WebP, ImageFormat::WebP),
            (SaveFormat::Bmp, ImageFormat::Bmp),
            (SaveFormat::Tiff, ImageFormat::Tiff),
            (SaveFormat::Ico, ImageFormat::Ico),
        ] {
            let bytes = encode(std::slice::from_ref(&f), fmt, &opts).unwrap_or_else(|e| panic!("{fmt:?}: {e}"));
            let d = decode(&bytes, src).unwrap_or_else(|e| panic!("{fmt:?}: {e}"));
            // Fully transparent pixels may lose their hidden colour; compare the rest.
            for (a, b) in d.frames[0].rgba.as_chunks::<4>().0.iter().zip(f.rgba.as_chunks::<4>().0.iter()) {
                if b[3] == 0 {
                    assert_eq!(a[3], 0, "{fmt:?}");
                } else {
                    assert_eq!(a, b, "{fmt:?}");
                }
            }
            if matches!(fmt, SaveFormat::Png | SaveFormat::Tiff) {
                assert_eq!(d.info.dpi, (144.0, 144.0), "{fmt:?}");
            }
        }
    }

    #[test]
    fn png_keeps_icc_and_exif() {
        let icc = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let exif = crate::images::metadata::tests::sample_exif(1);
        let opts = EncodeOptions { icc: Some(icc.clone()), exif: Some(exif.clone()), ..Default::default() };
        let bytes = encode(&[sample(4, 4)], SaveFormat::Png, &opts).unwrap();
        let d = decode(&bytes, ImageFormat::Png).unwrap();
        assert_eq!(d.info.icc.as_deref(), Some(&icc[..]));
        assert_eq!(d.info.exif.as_deref(), Some(&exif[..]));
    }

    #[test]
    fn jpeg_is_close_and_keeps_metadata() {
        let mut f = sample(32, 32);
        for p in f.rgba.as_chunks_mut::<4>().0.iter_mut() {
            p[3] = 255;
        }
        let exif = crate::images::metadata::tests::sample_exif(1);
        let opts = EncodeOptions { quality: 95, exif: Some(exif), dpi: (300.0, 300.0), ..Default::default() };
        let bytes = encode(&[f.clone()], SaveFormat::Jpeg, &opts).unwrap();
        let d = decode(&bytes, ImageFormat::Jpeg).unwrap();
        let diff: u64 =
            d.frames[0].rgba.iter().zip(&f.rgba).map(|(a, b)| a.abs_diff(*b) as u64).sum::<u64>() / f.rgba.len() as u64;
        assert!(diff < 4, "mean difference {diff}");
        assert!(d.info.exif.is_some());
        assert_eq!(d.info.dpi, (300.0, 300.0));
    }

    #[test]
    fn gif_and_tiff_keep_every_frame() {
        let mut a = sample(8, 6);
        a.delay_ms = 70;
        let mut b = sample(8, 6);
        b.rgba.iter_mut().for_each(|v| *v = 255 - *v);
        b.delay_ms = 250;
        let gif = encode(&[a.clone(), b.clone()], SaveFormat::Gif, &Default::default()).unwrap();
        let d = decode(&gif, ImageFormat::Gif).unwrap();
        assert_eq!(d.frames.len(), 2);
        assert_eq!((d.frames[0].delay_ms, d.frames[1].delay_ms), (70, 250));
        let tiff = encode(&[a, b], SaveFormat::Tiff, &Default::default()).unwrap();
        let d = decode(&tiff, ImageFormat::Tiff).unwrap();
        assert_eq!(d.frames.len(), 2);
    }

    #[test]
    fn exr_round_trips_through_linear_light() {
        let mut f = sample(5, 5);
        for p in f.rgba.as_chunks_mut::<4>().0.iter_mut() {
            p[3] = 255;
        }
        let bytes = encode(&[f.clone()], SaveFormat::Exr, &Default::default()).unwrap();
        let d = decode(&bytes, ImageFormat::OpenExr).unwrap();
        for (a, b) in d.frames[0].rgba.iter().zip(&f.rgba) {
            assert!(a.abs_diff(*b) <= 1, "{a} vs {b}");
        }
    }

    #[test]
    fn formats_by_extension() {
        assert_eq!(SaveFormat::from_extension("JPEG"), Some(SaveFormat::Jpeg));
        assert_eq!(SaveFormat::from_extension("tif"), Some(SaveFormat::Tiff));
        assert_eq!(SaveFormat::from_extension("pdf"), None);
        assert_eq!(SaveFormat::for_source(ImageFormat::Jpeg2000), None);
        let ico = encode(&[sample(300, 2)], SaveFormat::Ico, &Default::default());
        assert!(ico.is_err());
    }
}
