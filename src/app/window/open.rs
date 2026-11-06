//! Opening documents: file dialog, drag-and-drop, recent files, type detection and
//! the password flow for encrypted PDFs.

use std::path::PathBuf;

use slint::ComponentHandle;

use super::{DocWindow, OpenDoc, file_name};
use crate::core::filetype::{self, FileKind, OPEN_FILTERS};
use crate::core::geometry::Size;
use crate::core::layout::ZoomMode;
use crate::pdf::document::PdfDoc;
use crate::pdf::engine::{DocId, Priority};
use crate::pdf::ffi::{OpenError, Source};

/// Maximum password attempts before the prompt stops reappearing.
const MAX_PASSWORD_ATTEMPTS: u32 = 5;

impl DocWindow {
    pub(crate) fn choose_and_open(&self) {
        let mut dialog = rfd::FileDialog::new().set_title("Open").set_parent(&self.ui.window().window_handle());
        for (label, exts) in OPEN_FILTERS {
            dialog = dialog.add_filter(*label, exts);
        }
        if let Some(dir) = self.doc_path().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(paths) = dialog.pick_files() {
            for p in paths {
                self.open_here_or_new(p);
            }
        }
    }

    /// Opens a file here if this window is empty, otherwise in a new window
    /// (Preview opens each document in its own window).
    pub fn open_here_or_new(&self, path: PathBuf) {
        if let Some(existing) = super::super::find_window_for(&path) {
            existing.ui.window().set_minimized(false);
            let _ = existing.ui.show();
            return;
        }
        if !self.has_document() && self.st.borrow().password_for.is_none() {
            self.open_path(path);
        } else if let Err(e) = DocWindow::open_new(Some(path)) {
            self.error("Could not open a new window", &e);
        }
    }

    pub(crate) fn open_recent(&self, index: usize) {
        let path = super::super::context().settings.borrow().recent.get(index).cloned();
        let Some(path) = path else { return };
        if !path.exists() {
            self.error(
                &format!("“{}” can’t be found.", file_name(&path)),
                "It may have been moved, renamed or deleted. It has been removed from Open Recent.",
            );
            super::super::update_settings(|s| s.remove_recent(&path));
            super::super::refresh_window_menus();
            return;
        }
        self.open_here_or_new(path);
    }

    /// Detects the file type and opens it with the matching backend.
    pub fn open_path(&self, path: PathBuf) {
        let kind = match filetype::detect_path(&path) {
            Ok(k) => k,
            Err(e) => {
                self.error(&format!("“{}” could not be opened.", file_name(&path)), &friendly_io(&e));
                return;
            }
        };
        match kind {
            FileKind::Pdf => self.open_pdf(path, None, 0),
            FileKind::Image(fmt) => self.error(
                &format!("“{}” could not be opened.", file_name(&path)),
                &format!("{} images are not supported in this build.", fmt.name()),
            ),
            FileKind::Model3d(_) => self.error(
                &format!("“{}” could not be opened.", file_name(&path)),
                "3D files are not supported in this build.",
            ),
            FileKind::Unknown => self.error(
                &format!("“{}” could not be opened.", file_name(&path)),
                "Paper Mirror can open PDF documents, images and some 3D files. This file is not in a supported format.",
            ),
        }
    }

    pub(crate) fn open_pdf(&self, path: PathBuf, password: Option<String>, attempts: u32) {
        let engine = self.engine.clone();
        let id = engine.new_doc_id();
        let win = self.id;
        let p = path.clone();
        engine.submit(Priority::Interactive, None, move |st| {
            let result = PdfDoc::open(Source::File(p.clone()), password.as_deref()).map(|doc| {
                let sizes: Vec<Size> = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
                st.docs.insert(id, doc);
                sizes
            });
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_pdf_opened(id, p, result, attempts));
            });
        });
    }

    fn on_pdf_opened(&self, id: DocId, path: PathBuf, result: Result<Vec<Size>, OpenError>, attempts: u32) {
        let sizes = match result {
            Ok(s) => s,
            Err(OpenError::NeedsPassword) => {
                self.st.borrow_mut().password_for = Some((path.clone(), attempts));
                self.show_prompt(
                    "password",
                    &format!("“{}” is password protected", file_name(&path)),
                    "Enter the password to open this document.",
                    "",
                    true,
                    "Open",
                );
                return;
            }
            Err(OpenError::WrongPassword) if attempts + 1 < MAX_PASSWORD_ATTEMPTS => {
                self.st.borrow_mut().password_for = Some((path.clone(), attempts + 1));
                self.show_prompt(
                    "password",
                    &format!("“{}” is password protected", file_name(&path)),
                    "Enter the password to open this document.",
                    "The password is incorrect. Try again.",
                    true,
                    "Open",
                );
                return;
            }
            Err(e) => {
                self.error(&format!("“{}” could not be opened.", file_name(&path)), &e.to_string());
                return;
            }
        };
        super::super::measure("document-opened");
        let title = file_name(&path);
        self.release_document();
        {
            let mut st = self.st.borrow_mut();
            st.current_page = 0;
            st.selected = vec![0];
            st.zoom_mode = ZoomMode::FitWidth;
            st.layout = Default::default();
            st.doc = Some(OpenDoc { id, path: Some(path.clone()), title: title.clone(), sizes: sizes.clone() });
        }
        self.ui.set_doc_title(title.into());
        self.ui.set_has_document(true);
        self.ui.set_has_file(true);
        self.ui.set_multi_page(sizes.len() > 1);
        super::super::update_settings(|s| s.add_recent(&path));
        super::super::refresh_window_menus();
        self.relayout(None);
        self.ui.invoke_focus_canvas();
        self.ui.window().request_redraw();
    }
}

fn friendly_io(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "The file can’t be found. It may have been moved or deleted.".into(),
        std::io::ErrorKind::PermissionDenied => "You don’t have permission to read this file.".into(),
        _ => format!("The file could not be read ({e})."),
    }
}
