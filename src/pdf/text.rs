//! Text extraction, search and character geometry.
//!
//! All rectangles returned here are in page display space: points, origin at the
//! top-left of the page as displayed (rotation applied).

use std::os::raw::c_int;

use super::document::PdfDoc;
use super::ffi::{self, Page, TextPage, b};
use crate::core::geometry::{Point, Rect};

const FPDF_MATCHCASE: u32 = 0x1;
const FPDF_MATCHWHOLEWORD: u32 = 0x2;

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub page: usize,
    pub start: usize,
    pub len: usize,
    pub rects: Vec<Rect>,
    /// A short excerpt around the match for the results list.
    pub context: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub match_case: bool,
    pub whole_word: bool,
}

/// Converts a PDF-space rectangle (left, top, right, bottom) to display space.
pub fn pdf_rect_to_display(page: &Page, l: f64, t: f64, r: f64, bt: f64) -> Rect {
    let (x0, y0) = page.pdf_to_display(l, t);
    let (x1, y1) = page.pdf_to_display(r, bt);
    Rect::from_points(Point::new(x0, y0), Point::new(x1, y1))
}

/// Rectangles covering `count` characters from `start`, merged per text line.
pub fn range_rects(page: &Page, tp: &TextPage, start: usize, count: usize) -> Vec<Rect> {
    let n = unsafe { b().FPDFText_CountRects(tp.handle(), start as c_int, count as c_int) };
    let mut out = Vec::with_capacity(n.max(0) as usize);
    for i in 0..n.max(0) {
        let (mut l, mut t, mut r, mut bt) = (0.0, 0.0, 0.0, 0.0);
        if unsafe { b().FPDFText_GetRect(tp.handle(), i, &mut l, &mut t, &mut r, &mut bt) } != 0 {
            out.push(pdf_rect_to_display(page, l, t, r, bt));
        }
    }
    out
}

pub fn char_count(tp: &TextPage) -> usize {
    unsafe { b().FPDFText_CountChars(tp.handle()) }.max(0) as usize
}

/// Text of `count` characters from `start`.
pub fn text_range(tp: &TextPage, start: usize, count: usize) -> String {
    let total = char_count(tp);
    if start >= total || count == 0 {
        return String::new();
    }
    let count = count.min(total - start);
    let mut buf = vec![0u16; count + 1];
    let written = unsafe { b().FPDFText_GetText(tp.handle(), start as c_int, count as c_int, buf.as_mut_ptr()) };
    let end = (written.max(1) as usize - 1).min(count);
    String::from_utf16_lossy(&buf[..end])
}

/// Index of the character at a display-space point, within `tolerance` points.
pub fn char_at(page: &Page, tp: &TextPage, p: Point, tolerance: f64) -> Option<usize> {
    let (px, py) = page.display_to_pdf(p.x, p.y);
    let i = unsafe { b().FPDFText_GetCharIndexAtPos(tp.handle(), px, py, tolerance, tolerance) };
    (i >= 0).then_some(i as usize)
}

/// Display-space box of one character (loose box: full line height).
pub fn char_box(page: &Page, tp: &TextPage, index: usize) -> Option<Rect> {
    let mut r = ffi::rectf(0.0, 0.0, 0.0, 0.0);
    let ok = unsafe { b().FPDFText_GetLooseCharBox(tp.handle(), index as c_int, &mut r) };
    (ok != 0).then(|| pdf_rect_to_display(page, r.left as f64, r.top as f64, r.right as f64, r.bottom as f64))
}

fn context_for(tp: &TextPage, start: usize, len: usize) -> String {
    // Keep little text before the match so it stays visible in a narrow sidebar.
    const BEFORE: usize = 14;
    const AFTER: usize = 60;
    let from = start.saturating_sub(BEFORE);
    let raw = text_range(tp, from, (start - from) + len + AFTER);
    let mut s: String = raw.chars().map(|c| if c.is_whitespace() || c.is_control() { ' ' } else { c }).collect();
    // Trim partial words at both ends unless they are the match itself.
    if from > 0 {
        if let Some(i) = s.find(' ').filter(|i| *i < start - from) {
            s = s[i + 1..].to_string();
        }
        s.insert(0, '…');
    }
    let collapsed: Vec<&str> = s.split_whitespace().collect();
    let mut out = collapsed.join(" ");
    if out.chars().count() > 90 {
        out = out.chars().take(90).collect::<String>() + "…";
    }
    out
}

/// Finds every occurrence of `query` on one page.
pub fn search_page(doc: &PdfDoc, page_index: usize, query: &str, opts: SearchOptions) -> Vec<SearchHit> {
    if query.is_empty() {
        return Vec::new();
    }
    doc.with_page(page_index, |page| {
        let Some(tp) = page.text() else { return Vec::new() };
        let needle = ffi::wide(query);
        let mut flags = 0;
        if opts.match_case {
            flags |= FPDF_MATCHCASE;
        }
        if opts.whole_word {
            flags |= FPDF_MATCHWHOLEWORD;
        }
        let h = unsafe { b().FPDFText_FindStart(tp.handle(), needle.as_ptr(), flags as _, 0) };
        if h.is_null() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        while unsafe { b().FPDFText_FindNext(h) } != 0 {
            let start = unsafe { b().FPDFText_GetSchResultIndex(h) }.max(0) as usize;
            let len = unsafe { b().FPDFText_GetSchCount(h) }.max(0) as usize;
            if len == 0 {
                continue;
            }
            let rects = range_rects(page, &tp, start, len);
            hits.push(SearchHit { page: page_index, start, len, rects, context: context_for(&tp, start, len) });
            if hits.len() >= 10_000 {
                break;
            }
        }
        unsafe { b().FPDFText_FindClose(h) };
        hits
    })
    .unwrap_or_default()
}

/// All text on a page (used by copy-all and by tests).
pub fn page_text(doc: &PdfDoc, page_index: usize) -> String {
    doc.with_page(page_index, |page| page.text().map(|tp| text_range(&tp, 0, char_count(&tp))).unwrap_or_default())
        .unwrap_or_default()
}
