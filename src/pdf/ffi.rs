//! Controlled FFI over PDFium.
//!
//! `pdfium-render` is used only as a typed, dynamically loaded function table; Paper
//! Mirror owns every handle through the RAII wrappers below so lifetimes stay explicit.
//! All calls happen on the PDF engine thread (see `engine.rs`); unit tests may also
//! call in from test threads, which the `thread_safe` bindings serialise.

use std::ffi::c_void;
use std::fs::File;
use std::os::raw::{c_int, c_uchar, c_ulong};
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use pdfium_render::prelude::{
    FPDF_ANNOTATION, FPDF_BITMAP, FPDF_BOOKMARK, FPDF_DOCUMENT, FPDF_FILEACCESS, FPDF_FILEWRITE, FPDF_FONT,
    FPDF_FORMHANDLE, FPDF_PAGE, FPDF_PAGEOBJECT, FPDF_TEXTPAGE, FS_MATRIX, FS_POINTF, FS_QUADPOINTSF, FS_RECTF,
    FS_SIZEF, PdfiumLibraryBindings,
};

// Constants from fpdfview.h / fpdf_save.h (PDFium chromium/8138).
pub const FPDF_ANNOT: c_int = 0x01;
pub const FPDF_LCD_TEXT: c_int = 0x02;
pub const FPDF_REVERSE_BYTE_ORDER: c_int = 0x10;
pub const FPDF_PRINTING: c_int = 0x800;
pub const FPDF_ERR_SUCCESS: c_ulong = 0;
pub const FPDF_ERR_FILE: c_ulong = 2;
pub const FPDF_ERR_FORMAT: c_ulong = 3;
pub const FPDF_ERR_PASSWORD: c_ulong = 4;
pub const FPDF_ERR_SECURITY: c_ulong = 5;
pub const FPDF_ERR_PAGE: c_ulong = 6;
pub const FPDF_NO_INCREMENTAL: u32 = 2;
pub const FPDF_REMOVE_SECURITY: u32 = 4;
pub const FPDFBITMAP_BGRA: c_int = 4;

static LIB: OnceLock<Result<Box<dyn PdfiumLibraryBindings>, String>> = OnceLock::new();

/// Candidate locations for `pdfium.dll`, always as absolute paths so Windows never
/// searches the current directory or PATH for a substitute DLL.
fn library_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("PAPER_MIRROR_PDFIUM") {
        v.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("pdfium.dll"));
            // Test executables live in target/<profile>/deps.
            if let Some(parent) = dir.parent() {
                v.push(parent.join("pdfium.dll"));
            }
        }
    }
    v.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("third_party/pdfium/bin/pdfium.dll"));
    v
}

/// Returns the process-wide PDFium bindings, loading and initialising the library on
/// first use.
pub fn lib() -> Result<&'static dyn PdfiumLibraryBindings, String> {
    let r = LIB.get_or_init(|| {
        let mut errors = Vec::new();
        for path in library_candidates() {
            if !path.is_file() {
                continue;
            }
            match pdfium_render::prelude::Pdfium::bind_to_library(&path) {
                Ok(b) => {
                    unsafe { b.FPDF_InitLibrary() };
                    return Ok(b);
                }
                Err(e) => errors.push(format!("{}: {e:?}", path.display())),
            }
        }
        Err(if errors.is_empty() {
            "pdfium.dll was not found next to Paper Mirror".to_string()
        } else {
            errors.join("; ")
        })
    });
    match r {
        Ok(b) => Ok(b.as_ref()),
        Err(e) => Err(e.clone()),
    }
}

/// Convenience accessor for code paths that already proved the library loaded.
pub fn b() -> &'static dyn PdfiumLibraryBindings {
    lib().expect("PDFium library not loaded")
}

/// Decodes a UTF-16LE buffer as returned by PDFium (with trailing NUL).
pub fn utf16_bytes_to_string(buf: &[u8]) -> String {
    let units: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// Calls a PDFium "query length, then fill buffer" getter that returns UTF-16LE bytes.
pub fn read_utf16(mut f: impl FnMut(*mut c_void, c_ulong) -> c_ulong) -> String {
    let len = f(std::ptr::null_mut(), 0);
    if len <= 2 {
        return String::new();
    }
    let mut buf = vec![0u8; len as usize];
    let got = f(buf.as_mut_ptr().cast(), len);
    buf.truncate(got.min(len) as usize);
    utf16_bytes_to_string(&buf)
}

/// Encodes a string as NUL-terminated UTF-16 (`FPDF_WIDESTRING`).
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Where a document's bytes come from. File sources are read lazily so a 500 MB PDF
/// does not need 500 MB of RAM.
pub enum Source {
    File(PathBuf),
    Memory(std::sync::Arc<Vec<u8>>),
}

struct FileReader {
    file: File,
}

unsafe extern "C" fn get_block(param: *mut c_void, position: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int {
    let reader = unsafe { &*(param as *const FileReader) };
    let out = unsafe { std::slice::from_raw_parts_mut(buf, size as usize) };
    let mut done = 0usize;
    while done < out.len() {
        match reader.file.seek_read(&mut out[done..], position as u64 + done as u64) {
            Ok(0) | Err(_) => return 0,
            Ok(n) => done += n,
        }
    }
    1
}

/// Why opening failed, mapped from `FPDF_GetLastError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    NeedsPassword,
    WrongPassword,
    Malformed,
    Unsupported(String),
    Io(String),
    TooLarge,
    Library(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NeedsPassword => write!(f, "This PDF is password protected."),
            OpenError::WrongPassword => write!(f, "The password is incorrect."),
            OpenError::Malformed => write!(f, "The file is damaged or is not a valid PDF."),
            OpenError::Unsupported(s) => write!(f, "This PDF uses an unsupported security handler ({s})."),
            OpenError::Io(s) => write!(f, "The file could not be read: {s}"),
            OpenError::TooLarge => write!(f, "Files larger than 4 GB are not supported."),
            OpenError::Library(s) => write!(f, "The PDF engine could not be loaded: {s}"),
        }
    }
}

/// An open PDFium document plus whatever backs its bytes. Field order matters: the
/// document handle must close before its backing storage is dropped.
pub struct Doc {
    handle: FPDF_DOCUMENT,
    _reader: Option<Box<FileReader>>,
    _access: Option<Box<FPDF_FILEACCESS>>,
    _bytes: Option<std::sync::Arc<Vec<u8>>>,
}

impl Doc {
    pub fn open(source: Source, password: Option<&str>) -> Result<Doc, OpenError> {
        let b = lib().map_err(OpenError::Library)?;
        let doc = match source {
            Source::File(path) => {
                let file = File::open(&path).map_err(|e| OpenError::Io(e.to_string()))?;
                let len = file.metadata().map_err(|e| OpenError::Io(e.to_string()))?.len();
                if len > c_ulong::MAX as u64 {
                    return Err(OpenError::TooLarge);
                }
                let mut reader = Box::new(FileReader { file });
                let mut access = Box::new(FPDF_FILEACCESS {
                    m_FileLen: len as c_ulong,
                    m_GetBlock: Some(get_block),
                    m_Param: (&mut *reader as *mut FileReader).cast(),
                });
                let handle = unsafe { b.FPDF_LoadCustomDocument(&mut *access, password) };
                Doc { handle, _reader: Some(reader), _access: Some(access), _bytes: None }
            }
            Source::Memory(bytes) => {
                let handle = unsafe { b.FPDF_LoadMemDocument64(&bytes[..], password) };
                Doc { handle, _reader: None, _access: None, _bytes: Some(bytes) }
            }
        };
        if doc.handle.is_null() {
            // Drop skips null handles, and still releases the reader.
            let err = unsafe { b.FPDF_GetLastError() };
            drop(doc);
            return Err(match err {
                FPDF_ERR_PASSWORD if password.is_some() => OpenError::WrongPassword,
                FPDF_ERR_PASSWORD => OpenError::NeedsPassword,
                FPDF_ERR_SECURITY => OpenError::Unsupported("unknown".into()),
                FPDF_ERR_FILE => OpenError::Io("file not found or could not be opened".into()),
                _ => OpenError::Malformed,
            });
        }
        Ok(doc)
    }

    pub fn create() -> Result<Doc, OpenError> {
        let b = lib().map_err(OpenError::Library)?;
        let handle = unsafe { b.FPDF_CreateNewDocument() };
        if handle.is_null() {
            return Err(OpenError::Library("could not create a document".into()));
        }
        Ok(Doc { handle, _reader: None, _access: None, _bytes: None })
    }

    pub fn handle(&self) -> FPDF_DOCUMENT {
        self.handle
    }

    pub fn page_count(&self) -> usize {
        unsafe { b().FPDF_GetPageCount(self.handle) }.max(0) as usize
    }

    /// Displayed page size in points (rotation applied, crop box respected).
    pub fn page_size(&self, index: usize) -> (f32, f32) {
        let mut s = FS_SIZEF { width: 0.0, height: 0.0 };
        let ok = unsafe { b().FPDF_GetPageSizeByIndexF(self.handle, index as c_int, &mut s) };
        if ok == 0 || !(s.width > 0.0 && s.height > 0.0) {
            (612.0, 792.0)
        } else {
            (s.width, s.height)
        }
    }

    pub fn page(&self, index: usize) -> Option<Page> {
        let h = unsafe { b().FPDF_LoadPage(self.handle, index as c_int) };
        (!h.is_null()).then_some(Page { handle: h })
    }

    pub fn meta(&self, tag: &str) -> String {
        read_utf16(|buf, len| unsafe { b().FPDF_GetMetaText(self.handle, tag, buf, len) })
    }

    pub fn file_version(&self) -> Option<i32> {
        let mut v: c_int = 0;
        (unsafe { b().FPDF_GetFileVersion(self.handle, &mut v) } != 0).then_some(v)
    }

    pub fn security_revision(&self) -> i32 {
        unsafe { b().FPDF_GetSecurityHandlerRevision(self.handle) }
    }

    pub fn permissions(&self) -> u32 {
        unsafe { b().FPDF_GetDocPermissions(self.handle) as u32 }
    }

    /// Serialises the whole document (a full rewrite, never incremental).
    pub fn save_to_vec(&self, remove_security: bool) -> Option<Vec<u8>> {
        #[repr(C)]
        struct Writer {
            base: FPDF_FILEWRITE,
            out: Vec<u8>,
        }
        unsafe extern "C" fn write_block(this: *mut FPDF_FILEWRITE, data: *const c_void, size: c_ulong) -> c_int {
            let w = unsafe { &mut *(this as *mut Writer) };
            let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) };
            w.out.extend_from_slice(bytes);
            1
        }
        let mut w = Writer { base: FPDF_FILEWRITE { version: 1, WriteBlock: Some(write_block) }, out: Vec::new() };
        let flags = if remove_security { FPDF_REMOVE_SECURITY } else { FPDF_NO_INCREMENTAL };
        let ok = unsafe { b().FPDF_SaveAsCopy(self.handle, &mut w.base, flags) };
        (ok != 0).then_some(w.out)
    }
}

impl Drop for Doc {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { b().FPDF_CloseDocument(self.handle) };
        }
    }
}

pub struct Page {
    handle: FPDF_PAGE,
}

impl Page {
    pub fn handle(&self) -> FPDF_PAGE {
        self.handle
    }

    pub fn width(&self) -> f32 {
        unsafe { b().FPDF_GetPageWidthF(self.handle) }
    }

    pub fn height(&self) -> f32 {
        unsafe { b().FPDF_GetPageHeightF(self.handle) }
    }

    pub fn rotation(&self) -> i32 {
        unsafe { b().FPDFPage_GetRotation(self.handle) * 90 }
    }

    pub fn text(&self) -> Option<TextPage> {
        let h = unsafe { b().FPDFText_LoadPage(self.handle) };
        (!h.is_null()).then_some(TextPage { handle: h })
    }

    /// Converts a point in page display space (points, top-left origin of the rotated
    /// page) to PDF user space. Uses PDFium's own transform so rotation and non-zero
    /// box origins are handled exactly.
    pub fn display_to_pdf(&self, x: f32, y: f32) -> (f64, f64) {
        // Map through a virtual device at 1000 px per point to keep integer rounding
        // error below 0.001 pt.
        const K: f64 = 1000.0;
        let (w, h) = ((self.width() as f64 * K) as c_int, (self.height() as f64 * K) as c_int);
        let (mut px, mut py) = (0.0f64, 0.0f64);
        unsafe {
            b().FPDF_DeviceToPage(
                self.handle,
                0,
                0,
                w,
                h,
                0,
                (x as f64 * K) as c_int,
                (y as f64 * K) as c_int,
                &mut px,
                &mut py,
            )
        };
        (px, py)
    }

    /// Inverse of [`display_to_pdf`].
    pub fn pdf_to_display(&self, px: f64, py: f64) -> (f32, f32) {
        const K: f64 = 1000.0;
        let (w, h) = ((self.width() as f64 * K) as c_int, (self.height() as f64 * K) as c_int);
        let (mut dx, mut dy): (c_int, c_int) = (0, 0);
        unsafe { b().FPDF_PageToDevice(self.handle, 0, 0, w, h, 0, px, py, &mut dx, &mut dy) };
        ((dx as f64 / K) as f32, (dy as f64 / K) as f32)
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        unsafe { b().FPDF_ClosePage(self.handle) };
    }
}

pub struct TextPage {
    handle: FPDF_TEXTPAGE,
}

impl TextPage {
    pub fn handle(&self) -> FPDF_TEXTPAGE {
        self.handle
    }
}

impl Drop for TextPage {
    fn drop(&mut self) {
        unsafe { b().FPDFText_ClosePage(self.handle) };
    }
}

/// A BGRA bitmap allocated by PDFium.
pub struct Bitmap {
    handle: FPDF_BITMAP,
    pub width: u32,
    pub height: u32,
}

impl Bitmap {
    pub fn new(width: u32, height: u32) -> Option<Bitmap> {
        let h = unsafe { b().FPDFBitmap_CreateEx(width as c_int, height as c_int, FPDFBITMAP_BGRA, std::ptr::null_mut(), 0) };
        (!h.is_null()).then_some(Bitmap { handle: h, width, height })
    }

    pub fn handle(&self) -> FPDF_BITMAP {
        self.handle
    }

    pub fn fill(&self, argb: u32) {
        unsafe { b().FPDFBitmap_FillRect(self.handle, 0, 0, self.width as c_int, self.height as c_int, argb as _) };
    }

    pub fn stride(&self) -> usize {
        unsafe { b().FPDFBitmap_GetStride(self.handle) as usize }
    }

    /// Copies pixels out as tightly packed rows (4 bytes per pixel).
    pub fn to_packed(&self) -> Vec<u8> {
        let stride = self.stride();
        let row = self.width as usize * 4;
        let ptr = unsafe { b().FPDFBitmap_GetBuffer(self.handle) } as *const u8;
        let src = unsafe { std::slice::from_raw_parts(ptr, stride * self.height as usize) };
        if stride == row {
            return src.to_vec();
        }
        let mut out = Vec::with_capacity(row * self.height as usize);
        for y in 0..self.height as usize {
            out.extend_from_slice(&src[y * stride..y * stride + row]);
        }
        out
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe { b().FPDFBitmap_Destroy(self.handle) };
    }
}

pub fn matrix(a: f32, b_: f32, c: f32, d: f32, e: f32, f: f32) -> FS_MATRIX {
    FS_MATRIX { a, b: b_, c, d, e, f }
}

pub fn rectf(left: f32, top: f32, right: f32, bottom: f32) -> FS_RECTF {
    FS_RECTF { left, top, right, bottom }
}
