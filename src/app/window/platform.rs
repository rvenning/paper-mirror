//! Windows services from a document window: Print, Share and file-type registration.

use std::path::PathBuf;
use std::sync::Arc;

use super::DocWindow;
use crate::images::encode::SaveFormat;
use crate::pdf::annot_io::{self, SaveOptions};
use crate::pdf::document::PdfDoc;
use crate::pdf::engine::Priority;
use crate::pdf::ffi::Source;
use crate::pdf::print::{self, PrintSpec};
use crate::platform_windows::{assoc, print_dialog, share};

impl DocWindow {
    /// The native window handle (None under the test backend).
    pub(crate) fn hwnd(&self) -> Option<isize> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use slint::ComponentHandle;
        if !super::super::context().winit {
            return None;
        }
        let handle = self.ui.window().window_handle();
        match handle.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
            _ => None,
        }
    }

    /// File > Print…: the Windows Print dialog, then the chosen pages.
    pub(crate) fn print(&self) {
        self.finish_pending_edit();
        let Some(hwnd) = self.hwnd() else { return };
        if !self.has_document() {
            return;
        }
        let n = self.page_count();
        let has_selection = self.st.borrow().selected.len() > 1;
        match print_dialog::show(hwnd, n as u32, has_selection) {
            Ok(Some(chosen)) => {
                let pages = chosen.range.pages(n, self.current_page(), &self.selected_pages());
                self.print_to(chosen.dc, pages, None);
            }
            Ok(None) => {}
            Err(e) => self.error("Printing isn’t available.", &e),
        }
    }

    /// Prints `pages` (with markup) to the printer device context `dc`, which is
    /// deleted afterwards. `output` prints to a file (drivers that support it).
    pub fn print_to(&self, dc: usize, pages: Vec<usize>, output: Option<PathBuf>) {
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else {
            print::delete_dc(dc);
            return;
        };
        let annots = self.annotations();
        // Images fill the paper, as Preview's "Scale to Fit"; PDF pages only shrink.
        let spec = PrintSpec { pages, title: self.title(), output, enlarge: self.is_image(), auto_rotate: true };
        let win = self.id;
        self.ui.set_busy_text("Printing…".into());
        self.engine.submit(Priority::Interactive, None, move |s| {
            let result = (|| {
                let doc = s.doc(doc_id).ok_or("The document is no longer open.")?;
                let flat;
                let source = if annots.is_empty() {
                    doc
                } else {
                    let bytes = annot_io::save(doc, &annots, &SaveOptions { flatten: true, ..Default::default() })?;
                    flat = PdfDoc::open(Source::Memory(Arc::new(bytes)), None).map_err(|e| e.to_string())?;
                    &flat
                };
                print::print(source, dc, &spec)
            })();
            print::delete_dc(dc);
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| {
                    w.ui.set_busy_text(Default::default());
                    if let Err(e) = &result {
                        w.error("The document couldn’t be printed.", e);
                    }
                    *w.last_printed.borrow_mut() = Some(result);
                });
            });
        });
    }

    /// File > Share…: the Windows Share sheet. A document with unsaved changes is
    /// shared as a copy of what is on screen.
    pub(crate) fn share(&self) {
        self.finish_pending_edit();
        if !self.has_document() {
            return;
        }
        match self.doc_path() {
            Some(p) if !self.is_dirty() => self.share_now(&p),
            _ => {
                let dir = std::env::temp_dir().join("Paper Mirror Share").join(std::process::id().to_string());
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    self.error("The document couldn’t be shared.", &e.to_string());
                    return;
                }
                let stem = self
                    .doc_path()
                    .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| self.title().trim_end_matches(".pdf").to_string());
                let (target, opts) = if self.is_image() {
                    let format = self.image_save_format().unwrap_or(SaveFormat::Png);
                    let Some(t) = self.image_target(format, super::imaging::DEFAULT_QUALITY) else { return };
                    (
                        dir.join(format!("{stem}.{}", format.extension())),
                        SaveOptions { image: Some(t), ..Default::default() },
                    )
                } else {
                    (dir.join(format!("{stem}.pdf")), SaveOptions::default())
                };
                self.st.borrow_mut().share_after_save = Some(target.clone());
                self.save_to_copy(target, opts);
            }
        }
    }

    pub(crate) fn share_now(&self, path: &std::path::Path) {
        let Some(hwnd) = self.hwnd() else {
            *self.last_shared.borrow_mut() = Some(path.to_path_buf());
            return;
        };
        let title = crate::app::window::file_name(path);
        match share::share_file(hwnd, path, &title) {
            Ok(()) => *self.last_shared.borrow_mut() = Some(path.to_path_buf()),
            Err(e) => self.error("The Share window couldn’t be opened.", &e),
        }
    }

    /// Help > Use Paper Mirror for PDFs and Images…: registers the file types for
    /// this user, then opens Settings so the user can pick the defaults.
    pub(crate) fn register_file_types(&self) {
        if !self.confirm(
            "Use Paper Mirror for PDFs and images?",
            "Paper Mirror will be added to “Open with” for PDF and image files, for your account only. \
             Windows then opens Default apps, where you choose which file types Paper Mirror opens.",
        ) {
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                self.error("The file types couldn’t be registered.", &e.to_string());
                return;
            }
        };
        match assoc::register(assoc::BASE, &exe) {
            Ok(()) => crate::platform_windows::shell::open_default_apps(assoc::APP_NAME),
            Err(e) => self.error("The file types couldn’t be registered.", &e),
        }
    }

    pub(crate) fn unregister_file_types(&self) {
        if !self.confirm(
            "Remove Paper Mirror’s file types?",
            "Paper Mirror will no longer appear in “Open with” or in Default apps. Your files are not changed.",
        ) {
            return;
        }
        assoc::unregister(assoc::BASE);
    }
}
