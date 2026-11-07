//! Printing through GDI: each page is drawn by PDFium straight into the printer's
//! device context (vector output, so text stays sharp), fitted to the printable area
//! and centred. Runs on the engine thread.

use std::os::raw::c_int;
use std::path::PathBuf;

use windows_sys::Win32::Graphics::Gdi::{GetDeviceCaps, HDC, HORZRES, LOGPIXELSX, LOGPIXELSY, VERTRES};
use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use super::document::PdfDoc;
use super::ffi::{self, FPDF_ANNOT, FPDF_PRINTING};

/// What to print and how.
#[derive(Clone, Debug)]
pub struct PrintSpec {
    pub pages: Vec<usize>,
    pub title: String,
    /// Print to this file instead (drivers such as Microsoft Print to PDF).
    pub output: Option<PathBuf>,
    /// Enlarge pages smaller than the paper (images); otherwise only shrink.
    pub enlarge: bool,
    /// Turn landscape pages to match portrait paper and vice versa.
    pub auto_rotate: bool,
}

/// Where a page lands on the printable area, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Quarter turns clockwise.
    pub rotate: i32,
}

/// Fits a page of `page_pt` (points, as displayed) onto a printable area of
/// `area_px` device pixels at `dpi`.
pub fn place(page_pt: (f32, f32), area_px: (i32, i32), dpi: (f32, f32), enlarge: bool, auto_rotate: bool) -> Placement {
    let (mut pw, mut ph) = page_pt;
    let mut rotate = 0;
    let (aw, ah) = (area_px.0.max(1) as f32, area_px.1.max(1) as f32);
    if auto_rotate && (pw > ph) != (aw > ah) && (pw - ph).abs() > 1.0 {
        std::mem::swap(&mut pw, &mut ph);
        rotate = 1;
    }
    // Natural size in device pixels, then fitted.
    let (nw, nh) = (pw / 72.0 * dpi.0, ph / 72.0 * dpi.1);
    let fit = (aw / nw).min(ah / nh);
    let scale = if enlarge { fit } else { fit.min(1.0) };
    let (w, h) = ((nw * scale).round() as i32, (nh * scale).round() as i32);
    Placement { x: ((aw as i32 - w) / 2).max(0), y: ((ah as i32 - h) / 2).max(0), w, h, rotate }
}

type RenderPageFn = unsafe extern "system" fn(HDC, ffi::FPDF_PAGE, c_int, c_int, c_int, c_int, c_int, c_int);

/// PDFium's `FPDF_RenderPage` (Windows-only, draws to a device context), looked up
/// in the already-loaded library.
fn render_page_fn() -> Option<RenderPageFn> {
    let name: Vec<u16> = "pdfium.dll\0".encode_utf16().collect();
    unsafe {
        let module = GetModuleHandleW(name.as_ptr());
        if module.is_null() {
            return None;
        }
        let f = GetProcAddress(module, c"FPDF_RenderPage".as_ptr().cast())?;
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, RenderPageFn>(f))
    }
}

/// Prints `spec.pages` of `doc` (whose annotations should already be flattened in)
/// to the printer device context `hdc`. Returns the number of pages printed.
pub fn print(doc: &PdfDoc, hdc: usize, spec: &PrintSpec) -> Result<usize, String> {
    let render = render_page_fn().ok_or("The PDF engine can’t print on this system.")?;
    let hdc = hdc as HDC;
    let (area, dpi) = unsafe {
        (
            (GetDeviceCaps(hdc, HORZRES as i32), GetDeviceCaps(hdc, VERTRES as i32)),
            (GetDeviceCaps(hdc, LOGPIXELSX as i32) as f32, GetDeviceCaps(hdc, LOGPIXELSY as i32) as f32),
        )
    };
    if area.0 <= 0 || area.1 <= 0 || dpi.0 <= 0.0 || dpi.1 <= 0.0 {
        return Err("The printer reported no printable area.".into());
    }
    let title: Vec<u16> = spec.title.encode_utf16().chain(std::iter::once(0)).collect();
    let output: Option<Vec<u16>> = spec.output.as_ref().map(|p| crate::platform_windows::wide_path(p));
    let info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: title.as_ptr(),
        lpszOutput: output.as_ref().map_or(std::ptr::null(), |o| o.as_ptr()),
        lpszDatatype: std::ptr::null(),
        fwType: 0,
    };
    if unsafe { StartDocW(hdc, &info) } <= 0 {
        return Err("The printer didn’t accept the job.".into());
    }
    let mut printed = 0;
    for &p in &spec.pages {
        let Some(&size) = doc.page_sizes().get(p) else { continue };
        let at = place(size, area, dpi, spec.enlarge, spec.auto_rotate);
        let ok = unsafe { StartPage(hdc) } > 0
            && doc
                .with_page(p, |page| unsafe {
                    render(hdc, page.handle(), at.x, at.y, at.w, at.h, at.rotate, FPDF_ANNOT | FPDF_PRINTING);
                })
                .is_some()
            && unsafe { EndPage(hdc) } > 0;
        if !ok {
            unsafe { AbortDoc(hdc) };
            return Err(format!("Printing stopped at page {}.", p + 1));
        }
        printed += 1;
    }
    if unsafe { EndDoc(hdc) } <= 0 {
        return Err("The printer didn’t finish the job.".into());
    }
    Ok(printed)
}

/// A device context for a named printer (used for printing to a file in tests and
/// diagnostics). Delete it with `delete_dc`.
pub fn printer_dc(name: &str) -> Option<usize> {
    let driver: Vec<u16> = "WINSPOOL\0".encode_utf16().collect();
    let device: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let dc = unsafe {
        windows_sys::Win32::Graphics::Gdi::CreateDCW(
            driver.as_ptr(),
            device.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    (!dc.is_null()).then_some(dc as usize)
}

pub fn delete_dc(dc: usize) {
    unsafe { windows_sys::Win32::Graphics::Gdi::DeleteDC(dc as HDC) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_on_a4_at_600_dpi() {
        // A4 printable area roughly 4760 × 6814 px at 600 dpi.
        let p = place((612.0, 792.0), (4760, 6814), (600.0, 600.0), false, true);
        assert_eq!((p.w, p.h, p.rotate), (4760, 6160, 0), "shrunk to the width, aspect kept");
        assert_eq!((p.x, p.y), (0, 327));
    }

    #[test]
    fn small_pages_are_not_enlarged_unless_asked() {
        let p = place((144.0, 72.0), (4800, 6400), (600.0, 600.0), false, false);
        assert_eq!((p.w, p.h), (1200, 600));
        let p = place((144.0, 72.0), (4800, 6400), (600.0, 600.0), true, false);
        assert_eq!((p.w, p.h), (4800, 2400));
    }

    #[test]
    fn landscape_pages_turn_on_portrait_paper() {
        let p = place((792.0, 612.0), (4800, 6400), (600.0, 600.0), false, true);
        assert_eq!(p.rotate, 1);
        assert!(p.h > p.w);
        let p = place((792.0, 612.0), (4800, 6400), (600.0, 600.0), false, false);
        assert_eq!(p.rotate, 0);
        assert!(p.w > p.h);
    }
}
