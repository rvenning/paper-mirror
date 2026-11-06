//! Reading and writing Paper Mirror annotations (ADR 0006).
//!
//! On open, annotations carrying a `/PaperMirror` key are converted back into the
//! editable model and removed from PDFium's in-memory page. On save, PDFium writes the
//! document and lopdf appends standard annotation objects with generated appearance
//! streams, then optional flattening and encryption are applied.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::os::raw::c_int;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use super::document::{PdfDoc, RenderTarget};
use super::ffi::{self, Page, b};
use super::security::{self, Protection};
use crate::core::annot::{Annotation, Prim, Shape, n, pdf_path};
use crate::core::geometry::{Affine, Point, Rect};
use crate::core::textpath::FontStack;

/// Private key holding the editable JSON form of an annotation.
pub const PM_KEY: &str = "PaperMirror";
/// Info-dictionary key listing pages that carry Paper Mirror annotations, so large
/// documents need not be scanned page by page on open.
pub const PM_PAGES_KEY: &str = "PaperMirrorPages";
/// Documents up to this size are always scanned fully.
const FULL_SCAN_PAGES: usize = 200;

/// Display space -> PDF user space for one page (rotation and box origin included).
pub fn page_transform(page: &Page) -> Affine {
    let (ox, oy) = page.display_to_pdf(0.0, 0.0);
    let (xx, xy) = page.display_to_pdf(100.0, 0.0);
    let (yx, yy) = page.display_to_pdf(0.0, 100.0);
    let o = Point::new(ox as f32, oy as f32);
    let x1 = Point::new(o.x + (xx as f32 - o.x) / 100.0, o.y + (xy as f32 - o.y) / 100.0);
    let y1 = Point::new(o.x + (yx as f32 - o.x) / 100.0, o.y + (yy as f32 - o.y) / 100.0);
    let mut t = Affine::from_basis(o, x1, y1);
    // Rotations are exact quarter turns: snap rounding noise.
    for v in [&mut t.a, &mut t.b, &mut t.c, &mut t.d] {
        *v = v.round();
    }
    t
}

pub fn transforms(doc: &PdfDoc) -> Vec<Affine> {
    (0..doc.page_count()).map(|i| doc.with_page(i, page_transform).unwrap_or_default()).collect()
}

/// Pages that should be scanned for Paper Mirror annotations.
pub fn pages_to_scan(doc: &PdfDoc) -> Vec<usize> {
    let n = doc.page_count();
    if n <= FULL_SCAN_PAGES {
        return (0..n).collect();
    }
    let mut v: Vec<usize> =
        doc.doc.meta(PM_PAGES_KEY).split(',').filter_map(|s| s.trim().parse().ok()).filter(|p| *p < n).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Removes Paper Mirror annotations from the in-memory document and returns them in
/// display space. `next_id` assigns fresh session ids.
pub fn extract(doc: &PdfDoc, next_id: &mut u64) -> Vec<Annotation> {
    let mut out = Vec::new();
    for page_index in pages_to_scan(doc) {
        doc.with_page(page_index, |page| {
            let to_pdf = page_transform(page);
            let Some(to_display) = to_pdf.inverse() else { return };
            let count = unsafe { b().FPDFPage_GetAnnotCount(page.handle()) };
            let mut found = Vec::new();
            for i in 0..count {
                let annot = unsafe { b().FPDFPage_GetAnnot(page.handle(), i) };
                if annot.is_null() {
                    continue;
                }
                let mut json = None;
                if unsafe { b().FPDFAnnot_HasKey(annot, PM_KEY) } != 0 {
                    json = Some(ffi::read_utf16(|buf, len| unsafe {
                        b().FPDFAnnot_GetStringValue(annot, PM_KEY, buf.cast(), len)
                    }));
                }
                unsafe { b().FPDFPage_CloseAnnot(annot) };
                let Some(json) = json else { continue };
                if let Ok(a) = serde_json::from_str::<Annotation>(&json) {
                    let mut a = a.transformed(&to_display);
                    a.page = page_index;
                    a.id = *next_id;
                    *next_id += 1;
                    out.push(a);
                    found.push(i);
                }
            }
            // Remove back to front so indices stay valid; the model keeps z-order.
            for i in found.into_iter().rev() {
                unsafe { b().FPDFPage_RemoveAnnot(page.handle(), i) };
            }
        });
    }
    out
}

/// What to produce when saving.
#[derive(Clone, Debug, Default)]
pub struct SaveOptions {
    /// Merge annotations and form fields into page content.
    pub flatten: bool,
    /// Apply new password protection.
    pub protection: Option<Protection>,
    /// The document was opened with this password; keep it protected.
    pub reopen_password: Option<String>,
}

/// Serialises `doc` with `annots` (display space) applied.
pub fn save(doc: &PdfDoc, annots: &[Annotation], opts: &SaveOptions) -> Result<Vec<u8>, String> {
    let encrypted = doc.doc.security_revision() != -1;
    let base = doc.doc.save_to_vec(encrypted).ok_or("The PDF engine could not write the document.")?;
    let ts = transforms(doc);
    let render_lens = |page: usize, source: Rect, w: u32, h: u32| -> Option<Vec<u8>> {
        let scale = w as f32 / source.w.max(0.01);
        let mut px = vec![0u8; (w * h * 4) as usize];
        doc.render(
            page,
            scale,
            source.x * scale,
            source.y * scale,
            RenderTarget { pixels: &mut px, width: w, height: h },
            true,
        )
        .then(|| px.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect())
    };
    let mut bytes = if annots.iter().any(|a| !matches!(a.shape, Shape::Redaction)) {
        write_annotations(&base, annots, &ts, &render_lens)?
    } else {
        base
    };
    if opts.flatten {
        bytes = flatten(&bytes)?;
    }
    let protection = opts.protection.clone().or_else(|| {
        opts.reopen_password.as_ref().filter(|_| encrypted).map(|pw| Protection {
            open_password: pw.clone(),
            owner_password: String::new(),
            allow_printing: doc.doc.permissions() & (1 << 2) != 0,
            allow_copying: doc.doc.permissions() & (1 << 4) != 0,
            allow_editing: doc.doc.permissions() & (1 << 3) != 0,
        })
    });
    if let Some(p) = protection {
        bytes = security::encrypt(&bytes, &p)?;
    }
    Ok(bytes)
}

/// Merges all annotations and form fields into page content using PDFium.
pub fn flatten(bytes: &[u8]) -> Result<Vec<u8>, String> {
    const FLAT_PRINT: c_int = 1;
    let doc =
        PdfDoc::open(ffi::Source::Memory(std::sync::Arc::new(bytes.to_vec())), None).map_err(|e| e.to_string())?;
    for i in 0..doc.page_count() {
        if let Some(page) = doc.doc.page(i) {
            unsafe { b().FPDFPage_Flatten(page.handle(), FLAT_PRINT) };
        }
    }
    doc.doc.save_to_vec(false).ok_or_else(|| "The flattened document could not be written.".into())
}

type LensRenderer<'a> = dyn Fn(usize, Rect, u32, u32) -> Option<Vec<u8>> + 'a;

/// Appends annotation objects to an unencrypted PDF.
pub fn write_annotations(
    pdf: &[u8],
    annots: &[Annotation],
    transforms: &[Affine],
    render_lens: &LensRenderer<'_>,
) -> Result<Vec<u8>, String> {
    let mut doc =
        Document::load_mem(pdf).map_err(|e| format!("could not prepare the document for annotations: {e}"))?;
    let pages: BTreeMap<u32, ObjectId> = doc.get_pages();
    let now = pdf_date_now();
    let mut marked: Vec<usize> = Vec::new();
    for a in annots {
        if matches!(a.shape, Shape::Redaction) {
            continue;
        }
        let Some(&page_id) = pages.get(&(a.page as u32 + 1)) else { continue };
        let Some(t) = transforms.get(a.page) else { continue };
        let annot_id = add_annotation(&mut doc, a, t, page_id, &now, render_lens)?;
        let page = doc.get_object_mut(page_id).and_then(Object::as_dict_mut).map_err(|e| e.to_string())?;
        match page.get_mut(b"Annots") {
            Ok(Object::Array(arr)) => arr.push(annot_id.into()),
            Ok(Object::Reference(r)) => {
                let r = *r;
                if let Ok(Object::Array(arr)) = doc.get_object_mut(r) {
                    arr.push(annot_id.into());
                }
            }
            _ => page.set("Annots", vec![Object::Reference(annot_id)]),
        }
        marked.push(a.page);
    }
    marked.sort_unstable();
    marked.dedup();
    set_info_marker(&mut doc, &marked);
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| format!("could not write annotations: {e}"))?;
    Ok(out)
}

fn set_info_marker(doc: &mut Document, pages: &[usize]) {
    let list = pages.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
    let info_id = match doc.trailer.get(b"Info").and_then(Object::as_reference) {
        Ok(id) => id,
        Err(_) => {
            let id = doc.add_object(dictionary! {});
            doc.trailer.set("Info", id);
            id
        }
    };
    if let Ok(Object::Dictionary(info)) = doc.get_object_mut(info_id) {
        if list.is_empty() {
            info.remove(PM_PAGES_KEY.as_bytes());
        } else {
            info.set(PM_PAGES_KEY, Object::string_literal(list));
        }
    }
}

fn arr_f(v: &[f32]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

fn rect_obj(r: Rect) -> Object {
    arr_f(&[r.x, r.y, r.right(), r.bottom()])
}

/// Accumulates content-stream text and the graphics states it needs.
struct ApWriter {
    content: String,
    gstates: Dictionary,
    xobjects: Dictionary,
}

impl ApWriter {
    fn gs(&mut self, opacity: f32, multiply: bool) -> Option<String> {
        if opacity >= 0.999 && !multiply {
            return None;
        }
        let name = format!("GS{}{}", (opacity * 100.0).round() as i32, if multiply { "m" } else { "" });
        if !self.gstates.has(name.as_bytes()) {
            let mut d = dictionary! { "Type" => "ExtGState", "CA" => opacity, "ca" => opacity };
            if multiply {
                d.set("BM", "Multiply");
            }
            self.gstates.set(name.clone(), d);
        }
        Some(name)
    }
}

fn add_annotation(
    doc: &mut Document,
    a: &Annotation,
    t: &Affine,
    page_id: ObjectId,
    now: &str,
    render_lens: &LensRenderer<'_>,
) -> Result<ObjectId, String> {
    let pdf_a = a.transformed(t);
    let bbox = t.apply_rect(a.visual_bounds().inflate(1.0));
    let mut w = ApWriter { content: String::new(), gstates: Dictionary::new(), xobjects: Dictionary::new() };
    for prim in a.prims() {
        emit(doc, &mut w, &prim, a.page, t, render_lens);
    }
    let mut resources = Dictionary::new();
    if !w.gstates.is_empty() {
        resources.set("ExtGState", w.gstates);
    }
    if !w.xobjects.is_empty() {
        resources.set("XObject", w.xobjects);
    }
    let ap = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => rect_obj(bbox),
            "Resources" => resources,
        },
        w.content.into_bytes(),
    );
    let ap_id = doc.add_object(ap);

    let (r, g, bl) = a.style.stroke.pdf();
    let json = serde_json::to_string(&pdf_a).map_err(|e| e.to_string())?;
    let mut d = dictionary! {
        "Type" => "Annot",
        "Rect" => rect_obj(bbox),
        "F" => 4,
        "P" => page_id,
        "NM" => Object::string_literal(format!("paper-mirror-{}-{}", now.trim_start_matches("D:"), a.id)),
        "M" => Object::string_literal(now),
        "C" => arr_f(&[r, g, bl]),
        "CA" => a.style.opacity,
        "AP" => dictionary! { "N" => ap_id },
        PM_KEY => lopdf::text_string(&json),
    };
    let border = |w: f32, dashed: bool| {
        let mut bs = dictionary! { "W" => w, "S" => if dashed { "D" } else { "S" } };
        if dashed {
            bs.set("D", arr_f(&[w * 3.0, w * 2.0]));
        }
        bs
    };
    let quads = |rects: &Vec<Rect>| {
        let mut v = Vec::new();
        for r in rects {
            // Upper-left, upper-right, lower-left, lower-right in PDF space.
            for p in [
                Point::new(r.x, r.bottom()),
                Point::new(r.right(), r.bottom()),
                Point::new(r.x, r.y),
                Point::new(r.right(), r.y),
            ] {
                v.push(p.x);
                v.push(p.y);
            }
        }
        arr_f(&v)
    };
    match &pdf_a.shape {
        Shape::Highlight { rects } => {
            d.set("Subtype", "Highlight");
            d.set("QuadPoints", quads(rects));
        }
        Shape::Underline { rects } => {
            d.set("Subtype", "Underline");
            d.set("QuadPoints", quads(rects));
        }
        Shape::StrikeOut { rects } => {
            d.set("Subtype", "StrikeOut");
            d.set("QuadPoints", quads(rects));
        }
        Shape::Ink { strokes } | Shape::Signature { strokes } => {
            d.set("Subtype", "Ink");
            let list: Vec<Object> =
                strokes.iter().map(|s| arr_f(&s.iter().flat_map(|p| [p.x, p.y]).collect::<Vec<_>>())).collect();
            d.set("InkList", Object::Array(list));
            d.set("BS", border(pdf_a.style.width, false));
        }
        Shape::Line { from, to, arrow } => {
            d.set("Subtype", "Line");
            d.set("L", arr_f(&[from.x, from.y, to.x, to.y]));
            d.set("LE", Object::Array(vec!["None".into(), if *arrow { "ClosedArrow".into() } else { "None".into() }]));
            d.set("BS", border(pdf_a.style.width, pdf_a.style.dashed));
            d.set("IC", arr_f(&[r, g, bl]));
        }
        Shape::Rectangle | Shape::RoundedRectangle | Shape::Oval => {
            d.set("Subtype", if matches!(pdf_a.shape, Shape::Oval) { "Circle" } else { "Square" });
            d.set("BS", border(pdf_a.style.width, pdf_a.style.dashed));
            if let Some(f) = pdf_a.style.fill {
                let (fr, fg, fb) = f.pdf();
                d.set("IC", arr_f(&[fr, fg, fb]));
            }
        }
        Shape::Star | Shape::SpeechBubble { .. } => {
            d.set("Subtype", "Polygon");
            let verts: Vec<f32> = a
                .prims()
                .iter()
                .find_map(|p| match p {
                    Prim::Stroke { segs, .. } => Some(segs.clone()),
                    _ => None,
                })
                .unwrap_or_default()
                .iter()
                .filter_map(|s| match s {
                    crate::core::annot::Seg::Move(p) | crate::core::annot::Seg::Line(p) => Some(t.apply(*p)),
                    crate::core::annot::Seg::Cubic(_, _, p) => Some(t.apply(*p)),
                    crate::core::annot::Seg::Close => None,
                })
                .flat_map(|p| [p.x, p.y])
                .collect();
            d.set("Vertices", arr_f(&verts));
            d.set("BS", border(pdf_a.style.width, pdf_a.style.dashed));
        }
        Shape::TextBox { text } => {
            d.set("Subtype", "FreeText");
            d.set("Contents", lopdf::text_string(text));
            let (tr, tg, tb) = a.style.text_color.pdf();
            d.set(
                "DA",
                Object::string_literal(format!("/Helv {} Tf {} {} {} rg", n(a.style.font_size), n(tr), n(tg), n(tb))),
            );
            d.set(
                "Q",
                match a.style.align {
                    crate::core::annot::Align::Left => 0,
                    crate::core::annot::Align::Center => 1,
                    crate::core::annot::Align::Right => 2,
                },
            );
        }
        Shape::Note { text } => {
            d.set("Subtype", "Text");
            d.set("Name", "Comment");
            d.set("Contents", lopdf::text_string(text));
            d.set("Open", false);
        }
        Shape::ZoomLens { .. } => {
            d.set("Subtype", "Stamp");
            d.set("Name", "PaperMirrorLoupe");
        }
        Shape::Redaction => unreachable!("redactions are applied, never saved"),
    }
    Ok(doc.add_object(d))
}

fn emit(doc: &mut Document, w: &mut ApWriter, prim: &Prim, page: usize, t: &Affine, render_lens: &LensRenderer<'_>) {
    match prim {
        Prim::Stroke { segs, color, width, dashed, opacity, round } => {
            let (r, g, b) = color.pdf();
            let width = width * t.scale_factor();
            w.content.push_str("q\n");
            if let Some(gs) = w.gs(*opacity, false) {
                let _ = writeln!(w.content, "/{gs} gs");
            }
            let cap = if *round { 1 } else { 0 };
            let _ = writeln!(w.content, "{} {} {} RG {} w {cap} J {cap} j", n(r), n(g), n(b), n(width));
            if *dashed {
                let _ = writeln!(w.content, "[{} {}] 0 d", n(width * 3.0), n(width * 2.0));
            }
            pdf_path(segs, t, &mut w.content);
            w.content.push_str("S\nQ\n");
        }
        Prim::Fill { segs, color, opacity, multiply } => {
            let (r, g, b) = color.pdf();
            w.content.push_str("q\n");
            if let Some(gs) = w.gs(*opacity, *multiply) {
                let _ = writeln!(w.content, "/{gs} gs");
            }
            let _ = writeln!(w.content, "{} {} {} rg", n(r), n(g), n(b));
            pdf_path(segs, t, &mut w.content);
            w.content.push_str("f\nQ\n");
        }
        Prim::Text { rect, text, color, font, size, align } => {
            let Some(stack) = FontStack::new(font) else { return };
            let lines = stack.wrap(text, *size, rect.w.max(*size));
            let segs = stack.outline(&lines, *rect, *size, *align);
            if segs.is_empty() {
                return;
            }
            let (r, g, b) = color.pdf();
            let _ = writeln!(w.content, "q\n{} {} {} rg", n(r), n(g), n(b));
            pdf_path(&segs, t, &mut w.content);
            w.content.push_str("f\nQ\n");
        }
        Prim::Lens { rect, source } => {
            // Render the magnified source at 2 pixels per point of the lens.
            let (pw, ph) = ((rect.w * 2.0).round().max(1.0) as u32, (rect.h * 2.0).round().max(1.0) as u32);
            let Some(rgb) = render_lens(page, *source, pw, ph) else { return };
            let mut img = Stream::new(
                dictionary! {
                    "Type" => "XObject", "Subtype" => "Image", "Width" => pw as i64, "Height" => ph as i64,
                    "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
                },
                rgb,
            );
            let _ = img.compress();
            let id = doc.add_object(img);
            let name = format!("Im{}", w.xobjects.len());
            w.xobjects.set(name.clone(), id);
            // Clip to the circle, then map the unit square onto the lens box.
            w.content.push_str("q\n");
            pdf_path(&crate::core::annot::ellipse_segs(*rect), t, &mut w.content);
            w.content.push_str("W n\n");
            let box_pdf = t.apply_rect(*rect);
            // Image space is y-up, so map (0,0)->bottom-left of the displayed box.
            let o = t.apply(Point::new(rect.x, rect.bottom()));
            let x1 = t.apply(Point::new(rect.right(), rect.bottom()));
            let y1 = t.apply(Point::new(rect.x, rect.y));
            let _ = box_pdf;
            let _ = writeln!(
                w.content,
                "{} {} {} {} {} {} cm /{name} Do",
                n(x1.x - o.x),
                n(x1.y - o.y),
                n(y1.x - o.x),
                n(y1.y - o.y),
                n(o.x),
                n(o.y)
            );
            w.content.push_str("Q\n");
        }
    }
}

/// Current UTC time as a PDF date string.
pub fn pdf_date_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!("D:{y:04}{mo:02}{d:02}{:02}{:02}{:02}Z", s / 3600, (s / 60) % 60, s % 60)
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        assert_eq!(civil_from_days(20_763), (2026, 11, 6));
    }
}
