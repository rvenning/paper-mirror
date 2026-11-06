//! Page organisation on the PDFium document: rotate, delete, move, insert, import
//! and crop. Every operation closes cached page handles first, since structural
//! changes invalidate them, and refreshes the page sizes afterwards.

use std::os::raw::{c_float, c_int, c_ulong};

use super::document::PdfDoc;
use super::ffi::b;
use crate::core::geometry::Rect;
use crate::core::pagemap::PageOp;

/// Applies `op`. `source` supplies imported pages for `Insert` without `blank`.
pub fn apply(doc: &mut PdfDoc, op: &PageOp, source: Option<&PdfDoc>) -> Result<(), String> {
    let n = doc.page_count();
    let op = op.normalised(n);
    doc.close_pages();
    let d = doc.doc.handle();
    match &op {
        PageOp::Rotate { pages, quarter_turns } => {
            for &p in pages {
                let Some(page) = doc.doc.page(p) else { continue };
                let r = unsafe { b().FPDFPage_GetRotation(page.handle()) };
                unsafe { b().FPDFPage_SetRotation(page.handle(), (r + quarter_turns).rem_euclid(4)) };
            }
        }
        PageOp::Delete { pages } => {
            if pages.len() >= n {
                return Err("A document must keep at least one page.".into());
            }
            for &p in pages.iter().rev() {
                unsafe { b().FPDFPage_Delete(d, p as c_int) };
            }
        }
        PageOp::Move { pages, dest } => {
            if !pages.is_empty() {
                let idx: Vec<c_int> = pages.iter().map(|p| *p as c_int).collect();
                let ok = unsafe { b().FPDF_MovePages(d, idx.as_ptr(), idx.len() as c_ulong, *dest as c_int) };
                if ok == 0 {
                    return Err("The pages could not be moved.".into());
                }
            }
        }
        PageOp::Insert { at, count, blank } => match (blank, source) {
            (Some(size), _) => {
                for i in 0..*count {
                    let page = unsafe { b().FPDFPage_New(d, (*at + i) as c_int, size.w as f64, size.h as f64) };
                    if page.is_null() {
                        return Err("A blank page could not be added.".into());
                    }
                    unsafe { b().FPDF_ClosePage(page) };
                }
            }
            (None, Some(src)) => {
                let idx: Vec<c_int> = (0..(*count).min(src.page_count()) as c_int).collect();
                let ok = unsafe {
                    b().FPDF_ImportPagesByIndex(d, src.doc.handle(), idx.as_ptr(), idx.len() as c_ulong, *at as c_int)
                };
                if ok == 0 {
                    return Err("The pages could not be inserted.".into());
                }
            }
            (None, None) => return Err("Nothing to insert.".into()),
        },
        PageOp::Crop { page, rect } => {
            let r: Option<Rect> = doc.with_page(*page, |p| {
                let t = super::annot_io::page_transform(p);
                t.apply_rect(*rect)
            });
            let r = r.ok_or("The page could not be loaded.")?;
            if r.w < 1.0 || r.h < 1.0 {
                return Err("The crop area is too small.".into());
            }
            doc.close_pages();
            let page = doc.doc.page(*page).ok_or("The page could not be loaded.")?;
            unsafe {
                b().FPDFPage_SetCropBox(
                    page.handle(),
                    r.x as c_float,
                    r.y as c_float,
                    r.right() as c_float,
                    r.bottom() as c_float,
                )
            };
        }
    }
    doc.refresh_pages();
    Ok(())
}
