//! Table of contents (document outline), page links and document information.

use std::collections::HashSet;
use std::os::raw::c_int;

use super::document::PdfDoc;
use super::ffi::{FPDF_BOOKMARK, b, read_utf16};
use crate::core::geometry::Point;

/// Malformed outlines can be cyclic or enormous; stop walking past this many items.
const MAX_OUTLINE_ITEMS: usize = 20_000;
const PDFACTION_GOTO: u64 = 1;
const PDFACTION_URI: u64 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub title: String,
    pub depth: usize,
    pub page: Option<usize>,
    /// Index of the parent item in the flattened list.
    pub parent: Option<usize>,
    pub has_children: bool,
}

/// Flattens the outline tree depth-first.
pub fn outline(doc: &PdfDoc) -> Vec<OutlineItem> {
    let d = doc.doc.handle();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    fn walk(
        doc: &PdfDoc,
        first: FPDF_BOOKMARK,
        depth: usize,
        parent: Option<usize>,
        out: &mut Vec<OutlineItem>,
        seen: &mut HashSet<usize>,
    ) {
        let d = doc.doc.handle();
        let mut cur = first;
        while !cur.is_null() && out.len() < MAX_OUTLINE_ITEMS && depth < 64 {
            if !seen.insert(cur as usize) {
                return; // cycle
            }
            let title = read_utf16(|buf, len| unsafe { b().FPDFBookmark_GetTitle(cur, buf, len) });
            let page = bookmark_page(doc, cur);
            let child = unsafe { b().FPDFBookmark_GetFirstChild(d, cur) };
            let idx = out.len();
            out.push(OutlineItem {
                title: title.trim().to_string(),
                depth,
                page,
                parent,
                has_children: !child.is_null(),
            });
            if !child.is_null() {
                walk(doc, child, depth + 1, Some(idx), out, seen);
            }
            cur = unsafe { b().FPDFBookmark_GetNextSibling(d, cur) };
        }
    }
    let first = unsafe { b().FPDFBookmark_GetFirstChild(d, std::ptr::null_mut()) };
    walk(doc, first, 0, None, &mut out, &mut seen);
    out
}

fn bookmark_page(doc: &PdfDoc, bm: FPDF_BOOKMARK) -> Option<usize> {
    let d = doc.doc.handle();
    let mut dest = unsafe { b().FPDFBookmark_GetDest(d, bm) };
    if dest.is_null() {
        let action = unsafe { b().FPDFBookmark_GetAction(bm) };
        if !action.is_null() && unsafe { b().FPDFAction_GetType(action) } as u64 == PDFACTION_GOTO {
            dest = unsafe { b().FPDFAction_GetDest(d, action) };
        }
    }
    if dest.is_null() {
        return None;
    }
    let i = unsafe { b().FPDFDest_GetDestPageIndex(d, dest) };
    (i >= 0 && (i as usize) < doc.page_count()).then_some(i as usize)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Page(usize),
    Uri(String),
}

/// Display-space rectangles of every link on a page (for hover feedback).
pub fn link_rects(doc: &PdfDoc, page_index: usize) -> Vec<crate::core::geometry::Rect> {
    doc.with_page(page_index, |page| {
        let mut out = Vec::new();
        let mut pos: c_int = 0;
        let mut link = std::ptr::null_mut();
        while unsafe { b().FPDFLink_Enumerate(page.handle(), &mut pos, &mut link) } != 0 && out.len() < 5000 {
            let mut r = super::ffi::rectf(0.0, 0.0, 0.0, 0.0);
            if unsafe { b().FPDFLink_GetAnnotRect(link, &mut r) } != 0 {
                out.push(super::text::pdf_rect_to_display(
                    page,
                    r.left as f64,
                    r.top as f64,
                    r.right as f64,
                    r.bottom as f64,
                ));
            }
        }
        out
    })
    .unwrap_or_default()
}

/// The link under a display-space point, if any.
pub fn link_at(doc: &PdfDoc, page_index: usize, p: Point) -> Option<LinkTarget> {
    let d = doc.doc.handle();
    doc.with_page(page_index, |page| {
        let (x, y) = page.display_to_pdf(p.x, p.y);
        let link = unsafe { b().FPDFLink_GetLinkAtPoint(page.handle(), x, y) };
        if link.is_null() {
            return None;
        }
        let dest = unsafe { b().FPDFLink_GetDest(d, link) };
        if !dest.is_null() {
            let i = unsafe { b().FPDFDest_GetDestPageIndex(d, dest) };
            return (i >= 0).then_some(LinkTarget::Page(i as usize));
        }
        let action = unsafe { b().FPDFLink_GetAction(link) };
        if action.is_null() {
            return None;
        }
        match unsafe { b().FPDFAction_GetType(action) } as u64 {
            PDFACTION_GOTO => {
                let dest = unsafe { b().FPDFAction_GetDest(d, action) };
                let i = unsafe { b().FPDFDest_GetDestPageIndex(d, dest) };
                (i >= 0).then_some(LinkTarget::Page(i as usize))
            }
            PDFACTION_URI => {
                let len = unsafe { b().FPDFAction_GetURIPath(d, action, std::ptr::null_mut(), 0) };
                if len == 0 {
                    return None;
                }
                let mut buf = vec![0u8; len as usize];
                unsafe { b().FPDFAction_GetURIPath(d, action, buf.as_mut_ptr().cast(), len) };
                let s = String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string();
                Some(LinkTarget::Uri(s))
            }
            _ => None,
        }
    })
    .flatten()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocInfo {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
    pub created: String,
    pub modified: String,
    pub version: String,
    pub pages: usize,
    pub first_page_size: (f32, f32),
    pub encryption: String,
    pub permissions: Vec<(&'static str, bool)>,
}

pub fn info(doc: &PdfDoc) -> DocInfo {
    let rev = doc.doc.security_revision();
    let encryption = match rev {
        -1 => "None".to_string(),
        2 => "RC4 40-bit".into(),
        3 => "RC4 128-bit".into(),
        4 => "RC4/AES 128-bit".into(),
        5 | 6 => "AES 256-bit".into(),
        r => format!("Standard security handler, revision {r}"),
    };
    let perms = doc.doc.permissions();
    let permissions = if rev == -1 {
        Vec::new()
    } else {
        vec![
            ("Printing", perms & (1 << 2) != 0),
            ("Modifying", perms & (1 << 3) != 0),
            ("Copying text and images", perms & (1 << 4) != 0),
            ("Annotating", perms & (1 << 5) != 0),
            ("Filling forms", perms & (1 << 8) != 0),
            ("Assembling pages", perms & (1 << 10) != 0),
        ]
    };
    let v = doc.doc.file_version().unwrap_or(0);
    DocInfo {
        title: doc.doc.meta("Title"),
        author: doc.doc.meta("Author"),
        subject: doc.doc.meta("Subject"),
        keywords: doc.doc.meta("Keywords"),
        creator: doc.doc.meta("Creator"),
        producer: doc.doc.meta("Producer"),
        created: format_pdf_date(&doc.doc.meta("CreationDate")),
        modified: format_pdf_date(&doc.doc.meta("ModDate")),
        version: if v > 0 { format!("{}.{}", v / 10, v % 10) } else { "Unknown".into() },
        pages: doc.page_count(),
        first_page_size: doc.page_sizes().first().copied().unwrap_or((0.0, 0.0)),
        encryption,
        permissions,
    }
}

/// Formats a PDF date string ("D:YYYYMMDDHHmmSS+HH'mm'") as "YYYY-MM-DD HH:mm (UTC+HH:mm)".
/// Unparseable input is returned unchanged.
pub fn format_pdf_date(s: &str) -> String {
    let t = s.trim().trim_start_matches("D:");
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return s.to_string();
    }
    let part = |a: usize, b: usize, d: &str| digits.get(a..b).unwrap_or(d).to_string();
    let mut out = format!("{}-{}-{}", part(0, 4, ""), part(4, 6, "01"), part(6, 8, "01"));
    if digits.len() >= 12 {
        out.push_str(&format!(" {}:{}", part(8, 10, "00"), part(10, 12, "00")));
    }
    let rest = &t[digits.len()..];
    if let Some(sign) = rest.chars().next() {
        if sign == 'Z' {
            out.push_str(" (UTC)");
        } else if sign == '+' || sign == '-' {
            let tz: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            if tz.len() >= 2 {
                let mm = tz.get(2..4).unwrap_or("00");
                out.push_str(&format!(" (UTC{sign}{}:{mm})", &tz[0..2]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::format_pdf_date;

    #[test]
    fn pdf_dates() {
        assert_eq!(format_pdf_date("D:20231001134500+01'00'"), "2023-10-01 13:45 (UTC+01:00)");
        assert_eq!(format_pdf_date("D:20231001134500Z"), "2023-10-01 13:45 (UTC)");
        assert_eq!(format_pdf_date("D:2023"), "2023-01-01");
        assert_eq!(format_pdf_date("yesterday"), "yesterday");
        assert_eq!(format_pdf_date(""), "");
    }
}
