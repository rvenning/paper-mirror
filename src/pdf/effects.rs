//! Export transformations matching Preview's PDF effects ("Quartz filters") and
//! Reduce File Size. Both work on a copy of the document: colours of text and vector
//! graphics are rewritten in place (so text stays selectable) and image pixels are
//! transformed; Reduce File Size downsamples images above 150 ppi and re-encodes
//! opaque ones as JPEG.

use std::ffi::c_void;
use std::os::raw::{c_float, c_int, c_uchar, c_uint, c_ulong};

use serde::{Deserialize, Serialize};

use super::document::PdfDoc;
use super::ffi::{self, FPDF_PAGEOBJECT, b};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    BlackAndWhite,
    GrayTone,
    SepiaTone,
    BlueTone,
    LightnessDecrease,
    LightnessIncrease,
}

impl Effect {
    pub const ALL: [Effect; 6] = [
        Effect::BlackAndWhite,
        Effect::BlueTone,
        Effect::GrayTone,
        Effect::LightnessDecrease,
        Effect::LightnessIncrease,
        Effect::SepiaTone,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Effect::BlackAndWhite => "Black & White",
            Effect::GrayTone => "Gray Tone",
            Effect::SepiaTone => "Sepia Tone",
            Effect::BlueTone => "Blue Tone",
            Effect::LightnessDecrease => "Lightness Decrease",
            Effect::LightnessIncrease => "Lightness Increase",
        }
    }

    /// Maps one sRGB colour.
    pub fn apply(self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let (rf, gf, bf) = (r as f32, g as f32, b as f32);
        let lum = 0.299 * rf + 0.587 * gf + 0.114 * bf;
        let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        match self {
            Effect::GrayTone => (c(lum), c(lum), c(lum)),
            Effect::BlackAndWhite => {
                // High-contrast grey: a smoothstep around the midpoint.
                let t = (lum / 255.0 - 0.5) * 2.2 + 0.5;
                let t = t.clamp(0.0, 1.0);
                let v = t * t * (3.0 - 2.0 * t) * 255.0;
                (c(v), c(v), c(v))
            }
            Effect::SepiaTone => (
                c(0.393 * rf + 0.769 * gf + 0.189 * bf),
                c(0.349 * rf + 0.686 * gf + 0.168 * bf),
                c(0.272 * rf + 0.534 * gf + 0.131 * bf),
            ),
            Effect::BlueTone => (c(lum * 0.80), c(lum * 0.90), c(lum * 0.80 + 255.0 * 0.20)),
            Effect::LightnessDecrease => (c(rf * 0.8), c(gf * 0.8), c(bf * 0.8)),
            Effect::LightnessIncrease => {
                (c(rf + (255.0 - rf) * 0.2), c(gf + (255.0 - gf) * 0.2), c(bf + (255.0 - bf) * 0.2))
            }
        }
    }
}

/// All page objects of a page, descending into form XObjects.
fn objects(page: ffi::FPDF_PAGE) -> Vec<FPDF_PAGEOBJECT> {
    fn walk(form: FPDF_PAGEOBJECT, out: &mut Vec<FPDF_PAGEOBJECT>, depth: usize) {
        for i in 0..unsafe { b().FPDFFormObj_CountObjects(form) }.max(0) {
            let o = unsafe { b().FPDFFormObj_GetObject(form, i as c_ulong) };
            if !o.is_null() {
                out.push(o);
                if depth < 8 && unsafe { b().FPDFPageObj_GetType(o) } == 5 {
                    walk(o, out, depth + 1);
                }
            }
        }
    }
    let mut out = Vec::new();
    for i in 0..unsafe { b().FPDFPage_CountObjects(page) }.max(0) {
        let o = unsafe { b().FPDFPage_GetObject(page, i) };
        if !o.is_null() {
            out.push(o);
            if unsafe { b().FPDFPageObj_GetType(o) } == 5 {
                walk(o, &mut out, 0);
            }
        }
    }
    out
}

/// Copies an image object's decoded pixels as RGBA, with its format's alpha.
fn image_rgba(obj: FPDF_PAGEOBJECT) -> Option<(u32, u32, Vec<u8>, bool)> {
    let bmp = unsafe { b().FPDFImageObj_GetBitmap(obj) };
    if bmp.is_null() {
        return None;
    }
    let (w, h) = unsafe { (b().FPDFBitmap_GetWidth(bmp) as u32, b().FPDFBitmap_GetHeight(bmp) as u32) };
    let fmt = unsafe { b().FPDFBitmap_GetFormat(bmp) };
    let stride = unsafe { b().FPDFBitmap_GetStride(bmp) } as usize;
    let buf = unsafe { std::slice::from_raw_parts(b().FPDFBitmap_GetBuffer(bmp) as *const u8, stride * h as usize) };
    let bpp = match fmt {
        1 => 1,
        2 => 3,
        3 | 4 => 4,
        _ => {
            unsafe { b().FPDFBitmap_Destroy(bmp) };
            return None;
        }
    };
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let row = &buf[y * stride..];
        for x in 0..w as usize {
            let p = &row[x * bpp..x * bpp + bpp];
            let px = match bpp {
                1 => [p[0], p[0], p[0], 255],
                3 => [p[2], p[1], p[0], 255],
                _ => [p[2], p[1], p[0], if fmt == 4 { p[3] } else { 255 }],
            };
            out.extend_from_slice(&px);
        }
    }
    unsafe { b().FPDFBitmap_Destroy(bmp) };
    Some((w, h, out, fmt == 4))
}

/// Replaces an image object's pixels with `rgba` (stored as BGRA or BGRx).
fn set_image_rgba(page: ffi::FPDF_PAGE, obj: FPDF_PAGEOBJECT, w: u32, h: u32, rgba: &[u8], alpha: bool) -> bool {
    let fmt = if alpha { ffi::FPDFBITMAP_BGRA } else { 3 };
    let bmp = unsafe { b().FPDFBitmap_CreateEx(w as c_int, h as c_int, fmt, std::ptr::null_mut(), 0) };
    if bmp.is_null() {
        return false;
    }
    let stride = unsafe { b().FPDFBitmap_GetStride(bmp) } as usize;
    let dst = unsafe { std::slice::from_raw_parts_mut(b().FPDFBitmap_GetBuffer(bmp) as *mut u8, stride * h as usize) };
    for y in 0..h as usize {
        for x in 0..w as usize {
            let s = &rgba[(y * w as usize + x) * 4..][..4];
            dst[y * stride + x * 4..][..4].copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
    }
    let mut pages = [page];
    let ok = unsafe { b().FPDFImageObj_SetBitmap(pages.as_mut_ptr(), 1, obj, bmp) } != 0;
    unsafe { b().FPDFBitmap_Destroy(bmp) };
    ok
}

/// Applies `effect` to every page of `doc` in place.
pub fn apply_effect(doc: &PdfDoc, effect: Effect) -> Result<(), String> {
    doc.close_pages();
    for i in 0..doc.page_count() {
        let page = doc.doc.page(i).ok_or("A page could not be loaded.")?;
        for obj in objects(page.handle()) {
            unsafe {
                let (mut r, mut g, mut bl, mut a): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 255);
                if b().FPDFPageObj_GetFillColor(obj, &mut r, &mut g, &mut bl, &mut a) != 0 {
                    let (nr, ng, nb) = effect.apply(r as u8, g as u8, bl as u8);
                    b().FPDFPageObj_SetFillColor(obj, nr as c_uint, ng as c_uint, nb as c_uint, a);
                }
                if b().FPDFPageObj_GetStrokeColor(obj, &mut r, &mut g, &mut bl, &mut a) != 0 {
                    let (nr, ng, nb) = effect.apply(r as u8, g as u8, bl as u8);
                    b().FPDFPageObj_SetStrokeColor(obj, nr as c_uint, ng as c_uint, nb as c_uint, a);
                }
            }
            if unsafe { b().FPDFPageObj_GetType(obj) } == 3
                && let Some((w, h, mut px, alpha)) = image_rgba(obj)
            {
                for p in px.as_chunks_mut::<4>().0 {
                    let (r, g, bl) = effect.apply(p[0], p[1], p[2]);
                    p[0] = r;
                    p[1] = g;
                    p[2] = bl;
                }
                set_image_rgba(page.handle(), obj, w, h, &px, alpha);
            }
        }
        unsafe { b().FPDFPage_GenerateContent(page.handle()) };
    }
    Ok(())
}

/// Resolution that Reduce File Size downsamples to.
pub const REDUCED_PPI: f32 = 150.0;
const JPEG_QUALITY: u8 = 70;

struct MemReader {
    data: Vec<u8>,
}

unsafe extern "C" fn mem_block(param: *mut c_void, pos: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int {
    let r = unsafe { &*(param as *const MemReader) };
    let (pos, size) = (pos as usize, size as usize);
    if pos + size > r.data.len() {
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(r.data.as_ptr().add(pos), buf, size) };
    1
}

/// Downsamples images above `REDUCED_PPI` and re-encodes opaque ones as JPEG.
/// Returns the number of images changed.
pub fn reduce_size(doc: &PdfDoc) -> Result<usize, String> {
    doc.close_pages();
    let mut changed = 0;
    for i in 0..doc.page_count() {
        let page = doc.doc.page(i).ok_or("A page could not be loaded.")?;
        for obj in objects(page.handle()) {
            if unsafe { b().FPDFPageObj_GetType(obj) } != 3 {
                continue;
            }
            let (mut l, mut bt, mut r, mut t): (c_float, c_float, c_float, c_float) = (0.0, 0.0, 0.0, 0.0);
            if unsafe { b().FPDFPageObj_GetBounds(obj, &mut l, &mut bt, &mut r, &mut t) } == 0 {
                continue;
            }
            let Some((w, h, px, alpha)) = image_rgba(obj) else { continue };
            let (shown_w, shown_h) = ((r - l).abs().max(1.0), (t - bt).abs().max(1.0));
            let ppi = (w as f32 / (shown_w / 72.0)).max(h as f32 / (shown_h / 72.0));
            if ppi <= REDUCED_PPI * 1.1 {
                continue;
            }
            let k = REDUCED_PPI / ppi;
            let (nw, nh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
            let img = image::RgbaImage::from_raw(w, h, px).ok_or("bad image buffer")?;
            let small = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
            let ok = if alpha {
                set_image_rgba(page.handle(), obj, nw, nh, small.as_raw(), true)
            } else {
                let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
                let mut jpeg = Vec::new();
                let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY);
                rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
                let mut reader = Box::new(MemReader { data: jpeg });
                let mut access = ffi::FPDF_FILEACCESS {
                    m_FileLen: reader.data.len() as c_ulong,
                    m_GetBlock: Some(mem_block),
                    m_Param: (&mut *reader as *mut MemReader).cast(),
                };
                let mut pages = [page.handle()];
                // The JPEG is read immediately ("inline"), so the reader may drop after.
                unsafe { b().FPDFImageObj_LoadJpegFileInline(pages.as_mut_ptr(), 1, obj, &mut access) != 0 }
            };
            if ok {
                changed += 1;
            }
        }
        unsafe { b().FPDFPage_GenerateContent(page.handle()) };
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::Effect;

    #[test]
    fn effect_colour_maps() {
        assert_eq!(Effect::GrayTone.apply(255, 0, 0), (76, 76, 76));
        let (r, g, b) = Effect::SepiaTone.apply(128, 128, 128);
        assert!(r > g && g > b, "sepia is warm");
        let (r, g, b) = Effect::BlueTone.apply(128, 128, 128);
        assert!(b > r && b > g);
        assert_eq!(Effect::BlackAndWhite.apply(250, 250, 250), (255, 255, 255));
        assert_eq!(Effect::BlackAndWhite.apply(10, 10, 10), (0, 0, 0));
        assert!(Effect::LightnessDecrease.apply(200, 200, 200).0 < 200);
        assert!(Effect::LightnessIncrease.apply(100, 100, 100).0 > 100);
    }
}
