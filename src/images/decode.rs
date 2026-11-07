//! Decoding with memory limits. Pure-Rust decoders from `image` come first; TIFF and
//! HEIC/HEIF/AVIF go through Windows Imaging Component; JPEG 2000 is decoded by PDFium
//! (see `pdf::imagedoc::decode_jpx`), so it is not handled here.

use std::io::Cursor;

use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageReader};

use super::{Decoded, Frame, ImageInfo, metadata};
use crate::core::filetype::ImageFormat;

/// Largest width or height accepted.
pub const MAX_SIDE: u32 = 40_000;
/// Largest frame accepted, in pixels.
pub const MAX_PIXELS: u64 = 250_000_000;
/// Largest total pixels across all frames (animation frames are expanded to RGBA).
pub const MAX_TOTAL_PIXELS: u64 = 400_000_000;
/// Largest single allocation the decoders may make.
const MAX_ALLOC: u64 = 1 << 30;

fn limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(MAX_SIDE);
    l.max_image_height = Some(MAX_SIDE);
    l.max_alloc = Some(MAX_ALLOC);
    l
}

pub fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 {
        return Err("The image has no pixels.".into());
    }
    if w > MAX_SIDE || h > MAX_SIDE || w as u64 * h as u64 > MAX_PIXELS {
        return Err(format!("The image is too large ({w} × {h} pixels)."));
    }
    Ok(())
}

/// Decodes every format except JPEG 2000.
pub fn decode(bytes: &[u8], format: ImageFormat) -> Result<Decoded, String> {
    let mut decoded = match format {
        ImageFormat::Gif => decode_gif(bytes)?,
        ImageFormat::Tiff => {
            crate::platform_windows::wic::decode(bytes).or_else(|wic_err| decode_with_image(bytes, format).map_err(|_| wic_err))?
        }
        ImageFormat::Heic => crate::platform_windows::wic::decode(bytes).map_err(|e| {
            format!("{e} HEIC and AVIF images need the HEIF Image Extensions (and HEVC or AV1 Video Extensions) from Microsoft.")
        })?,
        ImageFormat::Jpeg2000 => return Err("JPEG 2000 is decoded by the PDF engine.".into()),
        _ => decode_with_image(bytes, format)?,
    };
    decoded.info.format = Some(format);
    // EXIF from containers WIC read (TIFF, HEIF) is re-parsed here.
    if decoded.info.exif.is_none() && matches!(format, ImageFormat::Tiff | ImageFormat::Heic) {
        decoded.info.exif = metadata::exif_from_container(bytes);
    }
    // TIFF orientation is not applied by WIC; HEIF transforms are (irot/imir).
    if format == ImageFormat::Tiff
        && let Some(o) = decoded.info.exif.as_deref().and_then(metadata::orientation)
    {
        for f in &mut decoded.frames {
            apply_orientation(f, o);
        }
    }
    if decoded.info.dpi.0 <= 0.0 {
        decoded.info.dpi = decoded.info.exif.as_deref().and_then(metadata::resolution).unwrap_or((72.0, 72.0));
    }
    let total: u64 = decoded.frames.iter().map(|f| f.width as u64 * f.height as u64).sum();
    if total > MAX_TOTAL_PIXELS {
        return Err("The image has too many pixels to open.".into());
    }
    if decoded.frames.is_empty() {
        return Err("The image has no frames.".into());
    }
    decoded.info.has_alpha = decoded.info.has_alpha || decoded.frames.iter().any(Frame::has_alpha);
    Ok(decoded)
}

fn image_format(f: ImageFormat) -> Option<image::ImageFormat> {
    Some(match f {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::WebP => image::ImageFormat::WebP,
        ImageFormat::Ico => image::ImageFormat::Ico,
        ImageFormat::OpenExr => image::ImageFormat::OpenExr,
        ImageFormat::Heic | ImageFormat::Jpeg2000 => return None,
    })
}

fn friendly(e: image::ImageError) -> String {
    match e {
        image::ImageError::Limits(_) => "The image is too large to open.".into(),
        image::ImageError::Unsupported(u) => format!("This kind of image is not supported ({u})."),
        other => format!("The image is damaged or incomplete ({other})."),
    }
}

fn decode_with_image(bytes: &[u8], format: ImageFormat) -> Result<Decoded, String> {
    let fmt = image_format(format).ok_or("unsupported")?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), fmt);
    reader.limits(limits());
    let mut decoder = reader.into_decoder().map_err(friendly)?;
    let (w, h) = decoder.dimensions();
    check_size(w, h)?;
    let icc = decoder.icc_profile().ok().flatten();
    let exif = decoder.exif_metadata().ok().flatten();
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let color = decoder.color_type();
    let mut img = DynamicImage::from_decoder(decoder).map_err(friendly)?;
    img.apply_orientation(orientation);
    let (bits, model, alpha) = describe(color);
    let rgba = to_rgba8(img);
    let dpi = match format {
        ImageFormat::Png => png_dpi(bytes),
        ImageFormat::Jpeg => jfif_dpi(bytes),
        _ => None,
    }
    .unwrap_or((0.0, 0.0));
    let (fw, fh) = (rgba.width(), rgba.height());
    Ok(Decoded {
        frames: vec![Frame::new(fw, fh, rgba.into_raw())],
        info: ImageInfo {
            format: Some(format),
            icc,
            exif: exif.and_then(|e| metadata::normalise_exif(&e)),
            dpi,
            bits_per_channel: bits,
            color_model: model.into(),
            has_alpha: alpha,
        },
    })
}

/// 8-bit RGBA; floating-point images are linear light and get the sRGB curve.
fn to_rgba8(img: DynamicImage) -> image::RgbaImage {
    match img {
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
            let f = img.into_rgba32f();
            let (w, h) = f.dimensions();
            let px: Vec<u8> = f
                .into_raw()
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    [
                        super::linear_to_srgb(p[0]),
                        super::linear_to_srgb(p[1]),
                        super::linear_to_srgb(p[2]),
                        (p[3].clamp(0.0, 1.0) * 255.0).round() as u8,
                    ]
                })
                .collect();
            image::RgbaImage::from_raw(w, h, px).expect("sized")
        }
        other => other.into_rgba8(),
    }
}

pub fn describe(c: image::ColorType) -> (u8, &'static str, bool) {
    use image::ColorType as C;
    match c {
        C::L8 => (8, "Gray", false),
        C::La8 => (8, "Gray", true),
        C::Rgb8 => (8, "RGB", false),
        C::Rgba8 => (8, "RGB", true),
        C::L16 => (16, "Gray", false),
        C::La16 => (16, "Gray", true),
        C::Rgb16 => (16, "RGB", false),
        C::Rgba16 => (16, "RGB", true),
        C::Rgb32F => (32, "RGB (floating point)", false),
        C::Rgba32F => (32, "RGB (floating point)", true),
        _ => (8, "RGB", true),
    }
}

fn decode_gif(bytes: &[u8]) -> Result<Decoded, String> {
    let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(friendly)?;
    decoder.set_limits(limits()).map_err(friendly)?;
    let (w, h) = decoder.dimensions();
    check_size(w, h)?;
    let mut frames = Vec::new();
    let mut total = 0u64;
    for f in decoder.into_frames() {
        let f = f.map_err(friendly)?;
        let (num, den) = f.delay().numer_denom_ms();
        let buf = f.into_buffer();
        total += buf.width() as u64 * buf.height() as u64;
        if total > MAX_TOTAL_PIXELS {
            return Err("The animation has too many frames to open.".into());
        }
        let mut frame = Frame::new(buf.width(), buf.height(), buf.into_raw());
        frame.delay_ms = num.checked_div(den).unwrap_or(100);
        frames.push(frame);
    }
    Ok(Decoded {
        frames,
        info: ImageInfo {
            format: Some(ImageFormat::Gif),
            dpi: (0.0, 0.0),
            bits_per_channel: 8,
            color_model: "Indexed".into(),
            ..Default::default()
        },
    })
}

/// Applies an EXIF orientation (1-8) to a frame.
pub fn apply_orientation(f: &mut Frame, orientation: u16) {
    let Some(o) = image::metadata::Orientation::from_exif(orientation as u8) else { return };
    let Some(img) = image::RgbaImage::from_raw(f.width, f.height, std::mem::take(&mut f.rgba)) else { return };
    let mut d = DynamicImage::ImageRgba8(img);
    d.apply_orientation(o);
    let img = d.into_rgba8();
    f.width = img.width();
    f.height = img.height();
    f.rgba = img.into_raw();
}

/// PNG pHYs in pixels per inch.
fn png_dpi(bytes: &[u8]) -> Option<(f32, f32)> {
    let reader = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
    let d = reader.info().pixel_dims?;
    (d.unit == png::Unit::Meter && d.xppu > 0 && d.yppu > 0)
        .then(|| ((d.xppu as f32 * 0.0254).round(), (d.yppu as f32 * 0.0254).round()))
}

/// JFIF APP0 density in pixels per inch.
fn jfif_dpi(bytes: &[u8]) -> Option<(f32, f32)> {
    let app0 = bytes.get(2..20)?;
    if app0[0] != 0xFF || app0[1] != 0xE0 || &app0[4..9] != b"JFIF\0" {
        return None;
    }
    let unit = app0[11];
    let x = u16::from_be_bytes([app0[12], app0[13]]) as f32;
    let y = u16::from_be_bytes([app0[14], app0[15]]) as f32;
    match unit {
        1 if x > 0.0 && y > 0.0 => Some((x, y)),
        2 if x > 0.0 && y > 0.0 => Some(((x * 2.54).round(), (y * 2.54).round())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 40) as u8, (y * 60) as u8, 7, if alpha && x == 0 { 0 } else { 255 }])
        });
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn decodes_png_pixels_and_alpha() {
        let d = decode(&png_bytes(4, 3, true), ImageFormat::Png).unwrap();
        assert_eq!(d.frames.len(), 1);
        let f = &d.frames[0];
        assert_eq!((f.width, f.height), (4, 3));
        assert_eq!(f.pixel(2, 1), [80, 60, 7, 255]);
        assert_eq!(f.pixel(0, 0)[3], 0);
        assert!(d.info.has_alpha);
        assert_eq!(d.info.dpi, (72.0, 72.0));
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        for fmt in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Gif, ImageFormat::Bmp, ImageFormat::WebP] {
            assert!(decode(b"\x89PNG\r\n\x1a\nnot really", fmt).is_err());
        }
        // Truncated PNG.
        let mut b = png_bytes(16, 16, false);
        b.truncate(b.len() / 2);
        assert!(decode(&b, ImageFormat::Png).is_err());
    }

    #[test]
    fn size_limits() {
        assert!(check_size(40_001, 10).is_err());
        assert!(check_size(20_000, 20_000).is_err());
        assert!(check_size(0, 5).is_err());
        assert!(check_size(8000, 6000).is_ok());
    }

    #[test]
    fn jfif_density() {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        b.extend_from_slice(b"JFIF\0");
        b.extend_from_slice(&[1, 2, 1, 0, 150, 0, 150, 0, 0]);
        assert_eq!(jfif_dpi(&b), Some((150.0, 150.0)));
    }
}
