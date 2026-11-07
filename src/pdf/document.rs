//! A PDF document as held by the engine thread.

use std::cell::RefCell;
use std::os::raw::c_int;
use std::path::PathBuf;

use super::ffi::{self, Doc, OpenError, Page, Source, b};

/// Keep a few parsed pages open: re-parsing a complex page on every scroll costs far
/// more than the memory for a handful of page objects.
const OPEN_PAGE_CACHE: usize = 6;

pub struct PdfDoc {
    pub doc: Doc,
    pub path: Option<PathBuf>,
    /// Password used to open, retained in memory only so the document can be reopened
    /// after a save; never written to disk or logs.
    pub password: Option<String>,
    sizes: Vec<(f32, f32)>,
    pages: RefCell<Vec<(usize, Page)>>,
    /// Form-fill environment for documents with an AcroForm. Declared after `pages`
    /// and torn down explicitly in `Drop` so pages close before the form, and the
    /// form before the document.
    form: Option<super::forms::FormEnv>,
    /// Applied to rendered pixels for display (image colour profile, Adjust Color preview).
    pub filter: Option<std::sync::Arc<crate::images::DisplayFilter>>,
    /// Draw a checkerboard behind the page (images with transparency).
    pub checkerboard: bool,
}

impl Drop for PdfDoc {
    fn drop(&mut self) {
        self.close_pages();
        self.form = None;
    }
}

/// Rendering target: a caller-owned pixel buffer, tightly packed, 4 bytes per pixel,
/// filled as RGBA.
pub struct RenderTarget<'a> {
    pub pixels: &'a mut [u8],
    pub width: u32,
    pub height: u32,
}

impl PdfDoc {
    pub fn open(source: Source, password: Option<&str>) -> Result<PdfDoc, OpenError> {
        let path = match &source {
            Source::File(p) => Some(p.clone()),
            Source::Memory(_) => None,
        };
        let doc = Doc::open(source, password)?;
        Ok(Self::wrap(doc, path, password.map(str::to_owned)))
    }

    pub fn wrap(doc: Doc, path: Option<PathBuf>, password: Option<String>) -> PdfDoc {
        let sizes = (0..doc.page_count()).map(|i| doc.page_size(i)).collect();
        let form = super::forms::FormEnv::init(&doc);
        PdfDoc { doc, path, password, sizes, pages: RefCell::new(Vec::new()), form, filter: None, checkerboard: false }
    }

    pub fn form_handle(&self) -> Option<ffi::FPDF_FORMHANDLE> {
        self.form.as_ref().map(|f| f.handle)
    }

    fn release_page(&self, page: Page) {
        if let Some(form) = self.form_handle() {
            unsafe { b().FORM_OnBeforeClosePage(page.handle(), form) };
        }
        drop(page);
    }

    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    /// Displayed page sizes in points.
    pub fn page_sizes(&self) -> &[(f32, f32)] {
        &self.sizes
    }

    /// Must be called after any operation that changes pages (rotate, crop, delete...).
    pub fn refresh_pages(&mut self) {
        self.close_pages();
        self.sizes = (0..self.doc.page_count()).map(|i| self.doc.page_size(i)).collect();
    }

    /// Runs `f` with a parsed page, using the small open-page cache.
    pub fn with_page<R>(&self, index: usize, f: impl FnOnce(&Page) -> R) -> Option<R> {
        let mut cache = self.pages.borrow_mut();
        if let Some(pos) = cache.iter().position(|(i, _)| *i == index) {
            let entry = cache.remove(pos);
            cache.push(entry);
        } else {
            let page = self.doc.page(index)?;
            if let Some(form) = self.form_handle() {
                unsafe { b().FORM_OnAfterLoadPage(page.handle(), form) };
            }
            if cache.len() >= OPEN_PAGE_CACHE {
                let (_, old) = cache.remove(0);
                self.release_page(old);
            }
            cache.push((index, page));
        }
        let (_, page) = cache.last().expect("just inserted");
        Some(f(page))
    }

    /// Drops cached page handles (e.g. before page structure changes).
    pub fn close_pages(&self) {
        let pages: Vec<(usize, Page)> = self.pages.borrow_mut().drain(..).collect();
        for (_, p) in pages {
            self.release_page(p);
        }
    }

    /// Renders part of a page. `scale` is device pixels per point; `(ox, oy)` is the
    /// device-pixel offset of the target's top-left corner within the whole page at
    /// that scale, so a target smaller than the page renders a clipped tile.
    pub fn render(
        &self,
        index: usize,
        scale: f32,
        ox: f32,
        oy: f32,
        target: RenderTarget<'_>,
        annotations: bool,
    ) -> bool {
        let RenderTarget { pixels, width, height } = target;
        if width == 0 || height == 0 || pixels.len() < (width * height * 4) as usize {
            return false;
        }
        self.with_page(index, |page| {
            let bmp = unsafe {
                b().FPDFBitmap_CreateEx(
                    width as c_int,
                    height as c_int,
                    ffi::FPDFBITMAP_BGRA,
                    pixels.as_mut_ptr().cast(),
                    (width * 4) as c_int,
                )
            };
            if bmp.is_null() {
                return false;
            }
            unsafe {
                if self.checkerboard {
                    fill_checkerboard(pixels, width, height, ox, oy);
                } else {
                    b().FPDFBitmap_FillRect(bmp, 0, 0, width as c_int, height as c_int, 0xFFFF_FFFF);
                }
                let m = ffi::matrix(scale, 0.0, 0.0, scale, -ox, -oy);
                let clip = ffi::rectf(0.0, 0.0, width as f32, height as f32);
                let mut flags = ffi::FPDF_REVERSE_BYTE_ORDER;
                if annotations {
                    flags |= ffi::FPDF_ANNOT;
                }
                b().FPDF_RenderPageBitmapWithMatrix(bmp, page.handle(), &m, &clip, flags);
                // With a form environment, PDFium leaves widgets to FFLDraw.
                if annotations && let Some(form) = self.form_handle() {
                    let (pw, ph) = (page.width() * scale, page.height() * scale);
                    b().FPDF_FFLDraw(
                        form,
                        bmp,
                        page.handle(),
                        -ox.round() as c_int,
                        -oy.round() as c_int,
                        pw.round() as c_int,
                        ph.round() as c_int,
                        0,
                        flags,
                    );
                }
                b().FPDFBitmap_Destroy(bmp);
            }
            if let Some(f) = &self.filter {
                f.apply(&mut pixels[..(width * height * 4) as usize], width, height);
            }
            true
        })
        .unwrap_or(false)
    }

    /// Convenience: renders a whole page at a pixel width into a new RGBA buffer.
    pub fn render_to_vec(&self, index: usize, width: u32) -> Option<(u32, u32, Vec<u8>)> {
        let (w, h) = *self.sizes.get(index)?;
        let scale = width as f32 / w;
        let height = ((h * scale).round() as u32).max(1);
        let mut px = vec![0u8; (width * height * 4) as usize];
        self.render(index, scale, 0.0, 0.0, RenderTarget { pixels: &mut px, width, height }, true)
            .then_some((width, height, px))
    }
}

/// Light gray 8-pixel checks, aligned to the whole page so tiles line up.
fn fill_checkerboard(px: &mut [u8], width: u32, height: u32, ox: f32, oy: f32) {
    let (ox, oy) = (ox.round() as i64, oy.round() as i64);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let dark = (((x as i64 + ox).div_euclid(8)) + ((y as i64 + oy).div_euclid(8))) % 2 != 0;
            let v = if dark { 0xDD } else { 0xFF };
            let i = (y * width as usize + x) * 4;
            px[i..i + 4].copy_from_slice(&[v, v, v, 0xFF]);
        }
    }
}
