//! Permanent redaction (ADR 0006): removes content under the redaction areas rather
//! than covering it.
//!
//! - Text: characters touching an area are deleted. Text objects that keep some
//!   characters are rebuilt from the remaining runs with the original font, size,
//!   matrix, colours and render mode.
//! - Images: pixels under an area are set to black in the image data itself.
//! - Vector paths entirely inside an area are deleted; partly covered paths remain
//!   under the black box (documented limitation).
//! - Annotations overlapping an area are deleted.
//! - An opaque black rectangle is drawn over each area.

use std::collections::{BTreeMap, HashSet};
use std::os::raw::{c_float, c_int, c_uint};

use super::document::PdfDoc;
use super::ffi::{self, FPDF_PAGEOBJECT, Page, b};
use crate::core::geometry::{Point, Rect};

const OBJ_TEXT: c_int = 1;
const OBJ_PATH: c_int = 2;
const OBJ_IMAGE: c_int = 3;
const OBJ_FORM: c_int = 5;
const FILLMODE_ALTERNATE: c_int = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RedactReport {
    pub chars_removed: usize,
    pub text_objects_rebuilt: usize,
    pub images_modified: usize,
    pub paths_removed: usize,
    pub annotations_removed: usize,
}

fn bounds(obj: FPDF_PAGEOBJECT) -> Option<Rect> {
    let (mut l, mut bt, mut r, mut t): (c_float, c_float, c_float, c_float) = (0.0, 0.0, 0.0, 0.0);
    (unsafe { b().FPDFPageObj_GetBounds(obj, &mut l, &mut bt, &mut r, &mut t) } != 0)
        .then(|| Rect::new(l, bt, r - l, t - bt))
}

fn intersects_any(r: &Rect, areas: &[Rect]) -> bool {
    areas.iter().any(|a| a.intersects(r))
}

fn inside_any(r: &Rect, areas: &[Rect]) -> bool {
    areas.iter().any(|a| r.x >= a.x && r.y >= a.y && r.right() <= a.right() && r.bottom() <= a.bottom())
}

/// All page objects, including those nested in form XObjects, with their parent form.
fn all_objects(page: &Page) -> Vec<(FPDF_PAGEOBJECT, Option<FPDF_PAGEOBJECT>)> {
    fn walk(form: FPDF_PAGEOBJECT, out: &mut Vec<(FPDF_PAGEOBJECT, Option<FPDF_PAGEOBJECT>)>, depth: usize) {
        let n = unsafe { b().FPDFFormObj_CountObjects(form) };
        for i in 0..n.max(0) {
            let o = unsafe { b().FPDFFormObj_GetObject(form, i as _) };
            if o.is_null() {
                continue;
            }
            out.push((o, Some(form)));
            if depth < 8 && unsafe { b().FPDFPageObj_GetType(o) } == OBJ_FORM {
                walk(o, out, depth + 1);
            }
        }
    }
    let mut out = Vec::new();
    let n = unsafe { b().FPDFPage_CountObjects(page.handle()) };
    for i in 0..n.max(0) {
        let o = unsafe { b().FPDFPage_GetObject(page.handle(), i) };
        if o.is_null() {
            continue;
        }
        out.push((o, None));
        if unsafe { b().FPDFPageObj_GetType(o) } == OBJ_FORM {
            walk(o, &mut out, 0);
        }
    }
    out
}

fn remove(page: &Page, obj: FPDF_PAGEOBJECT, parent: Option<FPDF_PAGEOBJECT>) -> bool {
    let ok = match parent {
        None => (unsafe { b().FPDFPage_RemoveObject(page.handle(), obj) }) != 0,
        Some(form) => (unsafe { b().FPDFFormObj_RemoveObject(form, obj) }) != 0,
    };
    if ok {
        unsafe { b().FPDFPageObj_Destroy(obj) };
    }
    ok
}

struct CharInfo {
    unit: u16,
    origin: (f64, f64),
    removed: bool,
}

/// Redacts `areas_display` (page display space) on one page.
pub fn redact_page(doc: &PdfDoc, page_index: usize, areas_display: &[Rect]) -> Result<RedactReport, String> {
    if areas_display.is_empty() {
        return Ok(RedactReport::default());
    }
    let report = doc
        .with_page(page_index, |page| {
            let t = super::annot_io::page_transform(page);
            let areas: Vec<Rect> = areas_display.iter().map(|r| t.apply_rect(*r)).collect();
            let mut rep = RedactReport::default();
            let objects = all_objects(page);
            let parent_of: BTreeMap<usize, Option<FPDF_PAGEOBJECT>> =
                objects.iter().map(|(o, p)| (*o as usize, *p)).collect();

            // --- Text, character by character -------------------------------------
            let mut by_object: BTreeMap<usize, Vec<CharInfo>> = BTreeMap::new();
            if let Some(tp) = page.text() {
                let n = super::text::char_count(&tp);
                for i in 0..n {
                    let obj = unsafe { b().FPDFText_GetTextObject(tp.handle(), i as c_int) };
                    if obj.is_null() {
                        continue; // generated character
                    }
                    let (mut l, mut r, mut bt, mut tt) = (0.0, 0.0, 0.0, 0.0);
                    unsafe { b().FPDFText_GetCharBox(tp.handle(), i as c_int, &mut l, &mut r, &mut bt, &mut tt) };
                    let cb = Rect::new(l as f32, bt as f32, (r - l) as f32, (tt - bt) as f32);
                    // Whitespace has no ink but still carries a position: test its origin.
                    let (mut ox, mut oy) = (0.0, 0.0);
                    unsafe { b().FPDFText_GetCharOrigin(tp.handle(), i as c_int, &mut ox, &mut oy) };
                    let removed = if cb.w > 0.0 && cb.h > 0.0 {
                        intersects_any(&cb, &areas)
                    } else {
                        // Strictly inside, so spaces beside a redacted word survive.
                        areas.iter().any(|a| a.inflate(-1.0).contains(Point::new(ox as f32, oy as f32)))
                    };
                    let unit = unsafe { b().FPDFText_GetUnicode(tp.handle(), i as c_int) } as u16;
                    by_object.entry(obj as usize).or_default().push(CharInfo { unit, origin: (ox, oy), removed });
                }
            }
            for (key, chars) in &by_object {
                let removed = chars.iter().filter(|c| c.removed).count();
                if removed == 0 {
                    continue;
                }
                let obj = *key as FPDF_PAGEOBJECT;
                let parent = parent_of.get(key).copied().flatten();
                rep.chars_removed += removed;
                // Runs of kept characters become new text objects.
                let mut runs: Vec<&[CharInfo]> = Vec::new();
                let mut start = None;
                for (i, c) in chars.iter().enumerate() {
                    match (c.removed, start) {
                        (false, None) => start = Some(i),
                        (true, Some(s)) => {
                            runs.push(&chars[s..i]);
                            start = None;
                        }
                        _ => {}
                    }
                }
                if let Some(s) = start {
                    runs.push(&chars[s..]);
                }
                let font = unsafe { b().FPDFTextObj_GetFont(obj) };
                let mut size: c_float = 0.0;
                unsafe { b().FPDFTextObj_GetFontSize(obj, &mut size) };
                let mut m = ffi::matrix(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
                unsafe { b().FPDFPageObj_GetMatrix(obj, &mut m) };
                let (mut fr, mut fg, mut fb, mut fa): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 255);
                let has_fill = unsafe { b().FPDFPageObj_GetFillColor(obj, &mut fr, &mut fg, &mut fb, &mut fa) } != 0;
                let (mut sr, mut sg, mut sb, mut sa): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 255);
                let has_stroke =
                    unsafe { b().FPDFPageObj_GetStrokeColor(obj, &mut sr, &mut sg, &mut sb, &mut sa) } != 0;
                let mode = unsafe { b().FPDFTextObj_GetTextRenderMode(obj) };
                if !font.is_null() && parent.is_none() {
                    for run in runs {
                        let text: String = String::from_utf16_lossy(&run.iter().map(|c| c.unit).collect::<Vec<_>>());
                        if text.trim().is_empty() {
                            continue;
                        }
                        let new = unsafe { b().FPDFPageObj_CreateTextObj(doc.doc.handle(), font, size) };
                        if new.is_null() {
                            continue;
                        }
                        let w = ffi::wide(&text);
                        unsafe {
                            b().FPDFText_SetText(new, w.as_ptr());
                            let mut nm = m;
                            nm.e = run[0].origin.0 as f32;
                            nm.f = run[0].origin.1 as f32;
                            b().FPDFPageObj_SetMatrix(new, &nm);
                            if has_fill {
                                b().FPDFPageObj_SetFillColor(new, fr, fg, fb, fa);
                            }
                            if has_stroke {
                                b().FPDFPageObj_SetStrokeColor(new, sr, sg, sb, sa);
                            }
                            b().FPDFTextObj_SetTextRenderMode(new, mode);
                            b().FPDFPage_InsertObject(page.handle(), new);
                        }
                    }
                    rep.text_objects_rebuilt += 1;
                }
                // Text inside form XObjects is removed whole: it cannot be re-created
                // inside the form through PDFium's API.
                remove(page, obj, parent);
            }

            // --- Images and paths ------------------------------------------------
            let mut seen = HashSet::new();
            for (obj, parent) in all_objects(page) {
                if !seen.insert(obj as usize) {
                    continue;
                }
                let Some(ob) = bounds(obj) else { continue };
                if !intersects_any(&ob, &areas) {
                    continue;
                }
                match unsafe { b().FPDFPageObj_GetType(obj) } {
                    OBJ_IMAGE => {
                        if inside_any(&ob, &areas) {
                            if remove(page, obj, parent) {
                                rep.images_modified += 1;
                            }
                        } else if black_out_image(page, obj, &areas) {
                            rep.images_modified += 1;
                        }
                    }
                    OBJ_PATH if inside_any(&ob, &areas) => {
                        if remove(page, obj, parent) {
                            rep.paths_removed += 1;
                        }
                    }
                    OBJ_TEXT if inside_any(&ob, &areas) => {
                        // Text with no extractable characters (e.g. Type 3 glyphs).
                        remove(page, obj, parent);
                    }
                    _ => {}
                }
            }

            // --- Annotations -------------------------------------------------------
            let count = unsafe { b().FPDFPage_GetAnnotCount(page.handle()) };
            for i in (0..count).rev() {
                let annot = unsafe { b().FPDFPage_GetAnnot(page.handle(), i) };
                if annot.is_null() {
                    continue;
                }
                let mut r = ffi::rectf(0.0, 0.0, 0.0, 0.0);
                let ok = unsafe { b().FPDFAnnot_GetRect(annot, &mut r) } != 0;
                unsafe { b().FPDFPage_CloseAnnot(annot) };
                let ar = Rect::new(
                    r.left.min(r.right),
                    r.bottom.min(r.top),
                    (r.right - r.left).abs(),
                    (r.top - r.bottom).abs(),
                );
                if ok && intersects_any(&ar, &areas) && unsafe { b().FPDFPage_RemoveAnnot(page.handle(), i) } != 0 {
                    rep.annotations_removed += 1;
                }
            }

            // --- Black boxes ---------------------------------------------------------
            for a in &areas {
                let rect = unsafe { b().FPDFPageObj_CreateNewRect(a.x, a.y, a.w, a.h) };
                if !rect.is_null() {
                    unsafe {
                        b().FPDFPageObj_SetFillColor(rect, 0, 0, 0, 255);
                        b().FPDFPath_SetDrawMode(rect, FILLMODE_ALTERNATE, 0);
                        b().FPDFPage_InsertObject(page.handle(), rect);
                    }
                }
            }
            if unsafe { b().FPDFPage_GenerateContent(page.handle()) } == 0 {
                return Err("The redacted page could not be regenerated.".to_string());
            }
            Ok(rep)
        })
        .ok_or("The page could not be loaded.")??;
    doc.close_pages();
    Ok(report)
}

/// Sets image pixels under the areas to black, in the image data itself.
fn black_out_image(page: &Page, obj: FPDF_PAGEOBJECT, areas: &[Rect]) -> bool {
    let bmp = unsafe { b().FPDFImageObj_GetBitmap(obj) };
    if bmp.is_null() {
        return false;
    }
    let (w, h) = unsafe { (b().FPDFBitmap_GetWidth(bmp), b().FPDFBitmap_GetHeight(bmp)) };
    let mut m = ffi::matrix(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    unsafe { b().FPDFPageObj_GetMatrix(obj, &mut m) };
    let to_page = crate::core::geometry::Affine { a: m.a, b: m.b, c: m.c, d: m.d, e: m.e, f: m.f };
    let Some(to_unit) = to_page.inverse() else {
        unsafe { b().FPDFBitmap_Destroy(bmp) };
        return false;
    };
    let mut changed = false;
    for a in areas {
        // Area in the image's unit square (y up), then pixels (row 0 at the top).
        let u = to_unit.apply_rect(*a);
        let x0 = (u.x * w as f32).floor().max(0.0) as c_int;
        let x1 = (u.right() * w as f32).ceil().min(w as f32) as c_int;
        let y0 = ((1.0 - u.bottom()) * h as f32).floor().max(0.0) as c_int;
        let y1 = ((1.0 - u.y) * h as f32).ceil().min(h as f32) as c_int;
        if x1 > x0 && y1 > y0 {
            unsafe { b().FPDFBitmap_FillRect(bmp, x0, y0, x1 - x0, y1 - y0, 0xFF00_0000) };
            changed = true;
        }
    }
    let mut pages = [page.handle()];
    let ok = changed && unsafe { b().FPDFImageObj_SetBitmap(pages.as_mut_ptr(), 1, obj, bmp) } != 0;
    unsafe { b().FPDFBitmap_Destroy(bmp) };
    ok
}
