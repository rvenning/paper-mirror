//! Images as PDF documents (ADR 0007): one page per frame at 0.75 pt per pixel, each
//! holding one image object. Rendering a page at 4/3 px per point gives back the
//! image's pixels, which is how edits and saves get pixels out again.

use std::os::raw::{c_int, c_void};
use std::sync::Arc;

use super::annot_io::{self, SaveOptions};
use super::document::PdfDoc;
use super::ffi::{self, Doc, Source, b};
use crate::core::annot::Annotation;
use crate::images::encode::{self, ImageTarget};
use crate::images::{Decoded, Frame, ImageInfo, PT_PER_PX, decode::check_size, pt_to_px};

const FPDFBITMAP_BGRX: c_int = 3;
const FPDF_RENDER_NO_SMOOTHIMAGE: c_int = 0x2000;

/// A new document with one page per frame.
/// Frames are consumed one by one, so their pixels are freed as PDFium takes copies.
pub fn build(frames: Vec<Frame>) -> Result<PdfDoc, String> {
    let doc = Doc::create().map_err(|e| e.to_string())?;
    for (i, f) in frames.into_iter().enumerate() {
        add_image_page(&doc, i, f)?;
    }
    Ok(PdfDoc::wrap(doc, None, None))
}

/// Inserts a page at `index` showing `f` at 0.75 pt per pixel.
pub fn add_image_page(doc: &Doc, index: usize, mut f: Frame) -> Result<(), String> {
    let (w, h) = (f.width, f.height);
    let (pw, ph) = (w as f64 * PT_PER_PX as f64, h as f64 * PT_PER_PX as f64);
    let alpha = f.has_alpha();
    // PDFium bitmaps are BGRA in memory: swap in place rather than copy.
    for p in f.rgba.as_chunks_mut::<4>().0.iter_mut() {
        p.swap(0, 2);
    }
    let bgra = &mut f.rgba;
    unsafe {
        let page = b().FPDFPage_New(doc.handle(), index as c_int, pw, ph);
        if page.is_null() {
            return Err("A page could not be created for the image.".into());
        }
        let result = (|| {
            let obj = b().FPDFPageObj_NewImageObj(doc.handle());
            if obj.is_null() {
                return Err("The image could not be added.".to_string());
            }
            let format = if alpha { ffi::FPDFBITMAP_BGRA } else { FPDFBITMAP_BGRX };
            let bmp = b().FPDFBitmap_CreateEx(
                w as c_int,
                h as c_int,
                format,
                bgra.as_mut_ptr().cast::<c_void>(),
                (w * 4) as c_int,
            );
            if bmp.is_null() {
                b().FPDFPageObj_Destroy(obj);
                return Err("The image is too large.".to_string());
            }
            let mut pages = [page];
            let ok = b().FPDFImageObj_SetBitmap(pages.as_mut_ptr(), 1, obj, bmp);
            b().FPDFBitmap_Destroy(bmp);
            if ok == 0 {
                b().FPDFPageObj_Destroy(obj);
                return Err("The image could not be stored.".to_string());
            }
            let m = ffi::matrix(pw as f32, 0.0, 0.0, ph as f32, 0.0, 0.0);
            b().FPDFPageObj_SetMatrix(obj, &m);
            let _ = b().FPDFPage_InsertObject(page, obj);
            if b().FPDFPage_GenerateContent(page) == 0 {
                return Err("The page could not be written.".to_string());
            }
            Ok(())
        })();
        b().FPDF_ClosePage(page);
        result
    }
}

/// The pixel size a page is saved at.
pub fn pixel_size(doc: &PdfDoc, page: usize) -> Option<(u32, u32)> {
    let (w, h) = *doc.page_sizes().get(page)?;
    Some((pt_to_px(w), pt_to_px(h)))
}

/// Renders a page at one pixel per image pixel onto transparency (RGBA). Page
/// rotation and cropping are applied, so this is what the user sees.
pub fn rasterise(doc: &PdfDoc, page: usize, annotations: bool) -> Option<Frame> {
    let (w, h) = pixel_size(doc, page)?;
    check_size(w, h).ok()?;
    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    let ok = doc.with_page(page, |p| unsafe {
        let bmp = b().FPDFBitmap_CreateEx(
            w as c_int,
            h as c_int,
            ffi::FPDFBITMAP_BGRA,
            rgba.as_mut_ptr().cast::<c_void>(),
            (w * 4) as c_int,
        );
        if bmp.is_null() {
            return false;
        }
        b().FPDFBitmap_FillRect(bmp, 0, 0, w as c_int, h as c_int, 0);
        let s = 1.0 / PT_PER_PX;
        let m = ffi::matrix(s, 0.0, 0.0, s, 0.0, 0.0);
        let clip = ffi::rectf(0.0, 0.0, w as f32, h as f32);
        let mut flags = ffi::FPDF_REVERSE_BYTE_ORDER | FPDF_RENDER_NO_SMOOTHIMAGE;
        if annotations {
            flags |= ffi::FPDF_ANNOT;
        }
        b().FPDF_RenderPageBitmapWithMatrix(bmp, p.handle(), &m, &clip, flags);
        b().FPDFBitmap_Destroy(bmp);
        true
    })?;
    ok.then(|| Frame::new(w, h, rgba))
}

/// Every page as pixels with `annots` flattened in: what Save writes for an image.
pub fn rasterise_with_annotations(doc: &PdfDoc, annots: &[Annotation]) -> Result<Vec<Frame>, String> {
    let flat;
    let source = if annots.is_empty() {
        doc
    } else {
        let bytes = annot_io::save(doc, annots, &SaveOptions { flatten: true, ..Default::default() })?;
        flat = PdfDoc::open(Source::Memory(Arc::new(bytes)), None).map_err(|e| e.to_string())?;
        &flat
    };
    (0..source.page_count())
        .map(|i| rasterise(source, i, true).ok_or_else(|| format!("Page {} could not be drawn.", i + 1)))
        .collect()
}

/// Encodes the document as an image file (see `ImageTarget`).
pub fn export(doc: &PdfDoc, annots: &[Annotation], t: &ImageTarget) -> Result<Vec<u8>, String> {
    let mut frames = match t.page {
        None => rasterise_with_annotations(doc, annots)?,
        Some(p) => vec![render_pdf_page(doc, annots, p, t.options.dpi.0)?],
    };
    for (f, d) in frames.iter_mut().zip(&t.delays) {
        f.delay_ms = *d;
    }
    if !t.format.keeps_all_frames() {
        frames.truncate(1);
    }
    encode::encode(&frames, t.format, &t.options)
}

/// One PDF page with its markup, on white, at `dpi`.
fn render_pdf_page(doc: &PdfDoc, annots: &[Annotation], page: usize, dpi: f32) -> Result<Frame, String> {
    let page_annots: Vec<Annotation> = annots.iter().filter(|a| a.page == page).cloned().collect();
    let flat;
    let source = if page_annots.is_empty() {
        doc
    } else {
        let bytes = annot_io::save(doc, &page_annots, &SaveOptions { flatten: true, ..Default::default() })?;
        flat = PdfDoc::open(Source::Memory(Arc::new(bytes)), None).map_err(|e| e.to_string())?;
        &flat
    };
    let (w, h) = *source.page_sizes().get(page).ok_or("The page does not exist.")?;
    let scale = dpi.clamp(10.0, 1200.0) / 72.0;
    let (pw, ph) = (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1));
    check_size(pw, ph)
        .map_err(|_| "That resolution makes the image too large. Choose a lower resolution.".to_string())?;
    let mut rgba = vec![0u8; pw as usize * ph as usize * 4];
    let target = super::document::RenderTarget { pixels: &mut rgba, width: pw, height: ph };
    if !source.render(page, scale, 0.0, 0.0, target, true) {
        return Err("The page could not be drawn.".into());
    }
    Ok(Frame::new(pw, ph, rgba))
}

/// Replaces a page with `f` (rotation and crop are reset: `f` is what is shown).
pub fn replace_page(doc: &mut PdfDoc, page: usize, f: Frame) -> Result<(), String> {
    doc.close_pages();
    unsafe { b().FPDFPage_Delete(doc.doc.handle(), page as c_int) };
    let r = add_image_page(&doc.doc, page, f);
    doc.refresh_pages();
    r
}

/// Decodes JPEG 2000 (JP2 file or raw codestream) with PDFium's OpenJPEG.
pub fn decode_jpx(bytes: &[u8]) -> Result<Decoded, String> {
    let (w, h) = jpx_size(bytes).ok_or("The JPEG 2000 image is damaged.")?;
    check_size(w, h)?;
    let pdf = jpx_pdf(bytes, w, h)?;
    let doc = PdfDoc::open(Source::Memory(Arc::new(pdf)), None).map_err(|e| e.to_string())?;
    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    let drawn = doc
        .with_page(0, |p| unsafe {
            let bmp = b().FPDFBitmap_CreateEx(
                w as c_int,
                h as c_int,
                ffi::FPDFBITMAP_BGRA,
                rgba.as_mut_ptr().cast::<c_void>(),
                (w * 4) as c_int,
            );
            if bmp.is_null() {
                return false;
            }
            b().FPDFBitmap_FillRect(bmp, 0, 0, w as c_int, h as c_int, 0);
            let m = ffi::matrix(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            let clip = ffi::rectf(0.0, 0.0, w as f32, h as f32);
            b().FPDF_RenderPageBitmapWithMatrix(
                bmp,
                p.handle(),
                &m,
                &clip,
                ffi::FPDF_REVERSE_BYTE_ORDER | FPDF_RENDER_NO_SMOOTHIMAGE,
            );
            b().FPDFBitmap_Destroy(bmp);
            true
        })
        .unwrap_or(false);
    if !drawn || rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        return Err("The JPEG 2000 image could not be decoded.".into());
    }
    Ok(Decoded {
        frames: vec![Frame::new(w, h, rgba)],
        info: ImageInfo {
            format: Some(crate::core::filetype::ImageFormat::Jpeg2000),
            dpi: (72.0, 72.0),
            bits_per_channel: 8,
            color_model: "RGB".into(),
            ..Default::default()
        },
    })
}

/// Image size from the SIZ marker of the codestream (inside a JP2 file or bare).
fn jpx_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let start = bytes.windows(4).position(|w| w == [0xFF, 0x4F, 0xFF, 0x51])?;
    let siz = bytes.get(start + 4..start + 4 + 20)?;
    let be = |i: usize| u32::from_be_bytes([siz[i], siz[i + 1], siz[i + 2], siz[i + 3]]);
    // Lsiz(2) Rsiz(2) Xsiz Ysiz XOsiz YOsiz
    let (x, y, xo, yo) = (be(4), be(8), be(12), be(16));
    Some((x.checked_sub(xo)?, y.checked_sub(yo)?)).filter(|(w, h)| *w > 0 && *h > 0)
}

/// A one-page PDF (1 pt per pixel) that draws the JPEG 2000 data as an image.
fn jpx_pdf(bytes: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let image = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "Filter" => "JPXDecode",
            "SMaskInData" => 1,
        },
        bytes.to_vec(),
    ));
    let content = doc.add_object(Stream::new(dictionary! {}, format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q").into_bytes()));
    let pages_id = doc.new_object_id();
    let page = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), (w as i64).into(), (h as i64).into()],
        "Contents" => content,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}
