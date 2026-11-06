//! Interactive forms through PDFium's form-fill engine. Values are set the way a user
//! would set them (focus, replace text, select option, click), so PDFium regenerates
//! field appearances and the saved file shows the values in any viewer. The PDFium
//! build has no JavaScript engine, so form scripts never run.

use std::os::raw::c_int;

use super::document::PdfDoc;
use super::ffi::{self, FPDF_FORMHANDLE, b};
use crate::core::geometry::{Point, Rect};

const FPDF_ANNOT_WIDGET: c_int = 20;
const FORMFLAG_READONLY: c_int = 1;
const FORMFLAG_REQUIRED: c_int = 2;
const FORMFLAG_TEXT_MULTILINE: c_int = 4096;
const FORMFLAG_TEXT_PASSWORD: c_int = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    CheckBox,
    Radio,
    ComboBox,
    ListBox,
    PushButton,
    Signature,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub page: usize,
    /// Annotation index on the page (the widget's identity while the page is open).
    pub annot_index: usize,
    pub kind: FieldKind,
    pub name: String,
    /// Tooltip / alternate name, used as the accessible label when present.
    pub label: String,
    pub rect: Rect,
    pub value: String,
    pub options: Vec<String>,
    pub selected: Option<usize>,
    pub checked: bool,
    pub read_only: bool,
    pub required: bool,
    pub multiline: bool,
    pub password: bool,
}

/// Owns a form-fill environment. Must be dropped before its document.
pub struct FormEnv {
    pub(crate) handle: FPDF_FORMHANDLE,
    _info: Box<ffi::FPDF_FORMFILLINFO>,
}

impl FormEnv {
    /// Creates the environment when the document has an AcroForm.
    pub fn init(doc: &ffi::Doc) -> Option<FormEnv> {
        if unsafe { b().FPDF_GetFormType(doc.handle()) } == 0 {
            return None;
        }
        // Version 1, no callbacks: PDFium guards every optional callback.
        let mut info: Box<ffi::FPDF_FORMFILLINFO> = Box::new(unsafe { std::mem::zeroed() });
        info.version = 1;
        let handle = unsafe { b().FPDFDOC_InitFormFillEnvironment(doc.handle(), &mut *info) };
        if handle.is_null() {
            return None;
        }
        // Tint fillable fields light blue, as Preview does.
        unsafe {
            // COLORREF order: 0x00BBGGRR.
            b().FPDF_SetFormFieldHighlightColor(handle, 0, 0x00FFE8DD);
            b().FPDF_SetFormFieldHighlightAlpha(handle, 90);
        }
        Some(FormEnv { handle, _info: info })
    }
}

impl Drop for FormEnv {
    fn drop(&mut self) {
        unsafe { b().FPDFDOC_ExitFormFillEnvironment(self.handle) };
    }
}

fn wstr(f: impl FnMut(*mut std::ffi::c_void, std::os::raw::c_ulong) -> std::os::raw::c_ulong) -> String {
    ffi::read_utf16(f)
}

/// Every form widget in the document, in page order and then reading order.
pub fn fields(doc: &PdfDoc) -> Vec<Field> {
    let Some(form) = doc.form_handle() else { return Vec::new() };
    let mut out = Vec::new();
    for page_index in 0..doc.page_count() {
        doc.with_page(page_index, |page| {
            let to_pdf = super::annot_io::page_transform(page);
            let Some(to_display) = to_pdf.inverse() else { return };
            let count = unsafe { b().FPDFPage_GetAnnotCount(page.handle()) };
            for i in 0..count.max(0) {
                let annot = unsafe { b().FPDFPage_GetAnnot(page.handle(), i) };
                if annot.is_null() {
                    continue;
                }
                if unsafe { b().FPDFAnnot_GetSubtype(annot) } as c_int == FPDF_ANNOT_WIDGET {
                    let kind = match unsafe { b().FPDFAnnot_GetFormFieldType(form, annot) } {
                        1 => Some(FieldKind::PushButton),
                        2 => Some(FieldKind::CheckBox),
                        3 => Some(FieldKind::Radio),
                        4 => Some(FieldKind::ComboBox),
                        5 => Some(FieldKind::ListBox),
                        6 => Some(FieldKind::Text),
                        7 => Some(FieldKind::Signature),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        let mut r = ffi::rectf(0.0, 0.0, 0.0, 0.0);
                        unsafe { b().FPDFAnnot_GetRect(annot, &mut r) };
                        let pdf = Rect::from_points(Point::new(r.left, r.bottom), Point::new(r.right, r.top));
                        let flags = unsafe { b().FPDFAnnot_GetFormFieldFlags(form, annot) };
                        let n_opts = unsafe { b().FPDFAnnot_GetOptionCount(form, annot) }.max(0);
                        let options: Vec<String> = (0..n_opts)
                            .map(|k| {
                                wstr(|buf, len| unsafe {
                                    b().FPDFAnnot_GetOptionLabel(form, annot, k, buf.cast(), len)
                                })
                            })
                            .collect();
                        let selected =
                            (0..n_opts).find(|k| unsafe { b().FPDFAnnot_IsOptionSelected(form, annot, *k) } != 0);
                        out.push(Field {
                            page: page_index,
                            annot_index: i as usize,
                            kind,
                            name: wstr(|buf, len| unsafe {
                                b().FPDFAnnot_GetFormFieldName(form, annot, buf.cast(), len)
                            }),
                            label: wstr(|buf, len| unsafe {
                                b().FPDFAnnot_GetFormFieldAlternateName(form, annot, buf.cast(), len)
                            }),
                            rect: to_display.apply_rect(pdf),
                            value: wstr(|buf, len| unsafe {
                                b().FPDFAnnot_GetFormFieldValue(form, annot, buf.cast(), len)
                            }),
                            options,
                            selected: selected.map(|k| k as usize),
                            checked: unsafe { b().FPDFAnnot_IsChecked(form, annot) } != 0,
                            read_only: flags & FORMFLAG_READONLY != 0,
                            required: flags & FORMFLAG_REQUIRED != 0,
                            multiline: flags & FORMFLAG_TEXT_MULTILINE != 0,
                            password: flags & FORMFLAG_TEXT_PASSWORD != 0,
                        });
                    }
                }
                unsafe { b().FPDFPage_CloseAnnot(annot) };
            }
        });
    }
    out.sort_by(|a, b| {
        a.page
            .cmp(&b.page)
            .then((a.rect.y / 4.0).round().total_cmp(&(b.rect.y / 4.0).round()))
            .then(a.rect.x.total_cmp(&b.rect.x))
    });
    out
}

/// Runs `f` with the focused widget, then removes focus so the value is committed.
fn with_focus(doc: &PdfDoc, page: usize, annot_index: usize, f: impl FnOnce(FPDF_FORMHANDLE, &ffi::Page)) -> bool {
    let Some(form) = doc.form_handle() else { return false };
    doc.with_page(page, |p| {
        let annot = unsafe { b().FPDFPage_GetAnnot(p.handle(), annot_index as c_int) };
        if annot.is_null() {
            return false;
        }
        let ok = unsafe { b().FORM_SetFocusedAnnot(form, annot) } != 0;
        if ok {
            f(form, p);
            unsafe { b().FORM_ForceToKillFocus(form) };
        }
        unsafe { b().FPDFPage_CloseAnnot(annot) };
        ok
    })
    .unwrap_or(false)
}

pub fn set_text(doc: &PdfDoc, page: usize, annot_index: usize, text: &str) -> bool {
    let w = ffi::wide(text);
    with_focus(doc, page, annot_index, |form, p| unsafe {
        b().FORM_SelectAllText(form, p.handle());
        b().FORM_ReplaceSelection(form, p.handle(), w.as_ptr());
    })
}

pub fn select_option(doc: &PdfDoc, page: usize, annot_index: usize, option: usize) -> bool {
    let mut ok = false;
    let focused = with_focus(doc, page, annot_index, |form, p| {
        ok = unsafe { b().FORM_SetIndexSelected(form, p.handle(), option as c_int, 1) } != 0;
    });
    focused && ok
}

/// Clicks a check box or radio button (toggling it the way a user would).
pub fn click(doc: &PdfDoc, field: &Field) -> bool {
    let Some(form) = doc.form_handle() else { return false };
    doc.with_page(field.page, |p| {
        let c = field.rect.center();
        let (x, y) = p.display_to_pdf(c.x, c.y);
        unsafe {
            b().FORM_OnLButtonDown(form, p.handle(), 0, x, y);
            b().FORM_OnLButtonUp(form, p.handle(), 0, x, y);
            b().FORM_ForceToKillFocus(form);
        }
        true
    })
    .unwrap_or(false)
}
