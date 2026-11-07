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

/// Page sizes, extracted annotations and the password that opened the file.
type OpenedPdf = (Vec<Size>, Vec<crate::core::annot::Annotation>, Option<String>);

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
        // Absolute paths keep Open Recent, bookmarks and the Inspector unambiguous.
        let path = std::path::absolute(&path).unwrap_or(path);
        let kind = match filetype::detect_path(&path) {
            Ok(k) => k,
            Err(e) => {
                self.error(&format!("“{}” could not be opened.", file_name(&path)), &friendly_io(&e));
                return;
            }
        };
        match kind {
            FileKind::Pdf => self.open_pdf(path, None, 0),
            FileKind::Image(fmt) => self.open_image(path, fmt),
            FileKind::Model3d(fmt) => self.open_model(path, fmt),
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
            let pw = password.clone();
            let result = PdfDoc::open(Source::File(p.clone()), password.as_deref()).map(|doc| {
                let sizes: Vec<Size> = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
                // Paper Mirror annotations become editable objects again.
                let mut next = 0;
                let annots = crate::pdf::annot_io::extract(&doc, &mut next);
                st.docs.insert(id, doc);
                (sizes, annots, pw)
            });
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_pdf_opened(id, p, result, attempts));
            });
        });
    }

    fn on_pdf_opened(&self, id: DocId, path: PathBuf, result: Result<OpenedPdf, OpenError>, attempts: u32) {
        let (sizes, annots, password) = match result {
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
        self.adopt_document(id, Some(path), title, sizes, annots, password, None);
    }

    /// Opens a document held in memory (File > Duplicate). It has no file until saved.
    pub(crate) fn open_memory(
        &self,
        bytes: std::sync::Arc<Vec<u8>>,
        title: String,
        image: Option<super::imaging::ImageMeta>,
    ) {
        let id = self.engine.new_doc_id();
        let win = self.id;
        let filter = image
            .as_ref()
            .and_then(|m| m.icc.clone())
            .map(|icc| std::sync::Arc::new(crate::images::DisplayFilter { adjust: None, icc: Some(icc) }));
        let checkerboard = image.as_ref().is_some_and(|m| m.info.has_alpha);
        self.engine.submit(Priority::Interactive, None, move |st| {
            let result = PdfDoc::open(Source::Memory(bytes), None).map(|mut doc| {
                doc.filter = filter;
                doc.checkerboard = checkerboard;
                let sizes: Vec<Size> = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
                let mut next = 0;
                let annots = crate::pdf::annot_io::extract(&doc, &mut next);
                st.docs.insert(id, doc);
                (sizes, annots)
            });
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| match result {
                    Ok((sizes, annots)) => {
                        w.adopt_document(id, None, title, sizes, annots, None, image);
                        w.mark_dirty();
                    }
                    Err(e) => w.error("The copy could not be opened.", &e.to_string()),
                });
            });
        });
    }

    /// Makes an engine document this window's document.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn adopt_document(
        &self,
        id: DocId,
        path: Option<PathBuf>,
        title: String,
        sizes: Vec<Size>,
        annots: Vec<crate::core::annot::Annotation>,
        password: Option<String>,
        image: Option<super::imaging::ImageMeta>,
    ) {
        self.release_document();
        let is_image = image.is_some();
        self.st.borrow_mut().image = image;
        self.ui.set_is_image(is_image);
        self.sync_location_ui();
        {
            let mut st = self.st.borrow_mut();
            st.current_page = 0;
            st.selected = vec![0];
            // Images show whole, as Preview does; documents fit the width.
            st.zoom_mode = if is_image { ZoomMode::FitPage } else { ZoomMode::FitWidth };
            st.layout = Default::default();
            st.doc = Some(OpenDoc { id, path: path.clone(), title: title.clone(), sizes: sizes.clone() });
            st.next_annot_id = annots.iter().map(|a| a.id).max().unwrap_or(0);
            st.annots = annots;
        }
        *self.open_password.borrow_mut() = password;
        self.ui.set_doc_title(title.into());
        self.ui.set_has_document(true);
        self.ui.set_has_file(path.is_some());
        self.ui.set_multi_page(sizes.len() > 1);
        self.ui.set_locked(self.is_locked());
        if let Some(path) = &path {
            super::super::update_settings(|s| s.add_recent(path));
        }
        super::super::refresh_window_menus();
        self.ui.set_sidebar_mode("thumbnails".into());
        self.load_outline(id);
        self.load_bookmarks();
        self.refresh_notes_list();
        self.load_fields();
        if self.ui.get_inspector_visible() {
            self.refresh_inspector();
        }
        self.relayout(None);
        // Small images are not enlarged past Actual Size.
        if is_image && self.st.borrow().zoom > 1.0 {
            self.set_zoom_mode(ZoomMode::Custom(1.0), None);
        }
        self.ui.invoke_focus_canvas();
        let opts = super::super::context().options;
        if let Some(q) = opts.search.as_deref() {
            self.ui.set_search_text(q.into());
            self.start_search(q);
        }
        for c in &opts.commands {
            self.command(c);
        }
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
