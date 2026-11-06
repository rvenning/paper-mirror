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
        PdfDoc { doc, path, password, sizes, pages: RefCell::new(Vec::new()) }
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
        self.pages.borrow_mut().clear();
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
            if cache.len() >= OPEN_PAGE_CACHE {
                cache.remove(0);
            }
            cache.push((index, page));
        }
        let (_, page) = cache.last().expect("just inserted");
        Some(f(page))
    }

    /// Drops cached page handles (e.g. before page structure changes).
    pub fn close_pages(&self) {
        self.pages.borrow_mut().clear();
    }

    /// Renders part of a page. `scale` is device pixels per point; `(ox, oy)` is the
    /// device-pixel offset of the target's top-left corner within the whole page at
    /// that scale, so a target smaller than the page renders a clipped tile.
    pub fn render(&self, index: usize, scale: f32, ox: f32, oy: f32, target: RenderTarget<'_>, annotations: bool) -> bool {
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
                b().FPDFBitmap_FillRect(bmp, 0, 0, width as c_int, height as c_int, 0xFFFF_FFFF);
                let m = ffi::matrix(scale, 0.0, 0.0, scale, -ox, -oy);
                let clip = ffi::rectf(0.0, 0.0, width as f32, height as f32);
                let mut flags = ffi::FPDF_REVERSE_BYTE_ORDER;
                if annotations {
                    flags |= ffi::FPDF_ANNOT;
                }
                b().FPDF_RenderPageBitmapWithMatrix(bmp, page.handle(), &m, &clip, flags);
                b().FPDFBitmap_Destroy(bmp);
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
