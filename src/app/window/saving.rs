//! Saving: Save, Save As, Export as Flattened PDF, redaction confirmation, the
//! unsaved-changes prompt and original-file preservation.

use std::path::PathBuf;

use slint::ComponentHandle;

use super::{DocWindow, file_name};
use crate::core::annot::{Annotation, Shape};
use crate::core::geometry::Rect;
use crate::pdf::annot_io::{self, SaveOptions};
use crate::pdf::engine::{DocId, Priority};
use crate::pdf::redact;

/// What happens once a save finishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterSave {
    Nothing,
    Close,
}

impl DocWindow {
    /// Save (in place) or Save As. Redactions ask first and default to a new file.
    pub(crate) fn save(&self, save_as: bool) {
        self.finish_pending_edit();
        // An untitled document (Duplicate) is saved with Save As.
        let (path, mut save_as) = match self.doc_path() {
            Some(p) => (p, save_as),
            None => (PathBuf::from(format!("{}.pdf", self.title())), true),
        };
        if !save_as && self.is_locked() {
            self.error(
                &format!("“{}” is locked.", file_name(&path)),
                "Unlock it with File > Unlock, or use Save As to save a copy.",
            );
            return;
        }
        if self.has_redactions() {
            match self.ask_redaction_save() {
                Some(true) => save_as = true,
                Some(false) => {}
                None => return,
            }
        }
        let target = if save_as {
            let suggested = if self.has_redactions() { redacted_name(&path) } else { file_name(&path) };
            match self.choose_save_path(&path, &suggested) {
                Some(p) => p,
                None => return,
            }
        } else {
            path
        };
        self.save_to(target, SaveOptions::default(), AfterSave::Nothing);
    }

    pub(crate) fn choose_save_path(&self, current: &std::path::Path, suggested: &str) -> Option<PathBuf> {
        if !super::super::context().winit {
            return None;
        }
        let mut d = rfd::FileDialog::new()
            .set_title("Save As")
            .add_filter("PDF document", &["pdf"])
            .set_file_name(suggested)
            .set_parent(&self.ui.window().window_handle());
        if let Some(dir) = current.parent() {
            d = d.set_directory(dir);
        }
        d.save_file().map(|p| if p.extension().is_none() { p.with_extension("pdf") } else { p })
    }

    /// Some(true) = Save As, Some(false) = Save in place, None = cancel.
    fn ask_redaction_save(&self) -> Option<bool> {
        if !super::super::context().winit {
            return Some(false);
        }
        let r = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Redact and save?")
            .set_description(
                "Redaction permanently removes the marked text, images and graphics from the saved file. \
                 It can’t be undone.\n\nSave the redacted version as a new file to keep the original.",
            )
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom("Save As…".into(), "Save".into(), "Cancel".into()))
            .set_parent(&self.ui.window().window_handle())
            .show();
        match r {
            rfd::MessageDialogResult::Custom(s) if s == "Save As…" => Some(true),
            rfd::MessageDialogResult::Custom(s) if s == "Save" => Some(false),
            _ => None,
        }
    }

    /// Writes the document (with annotations and redactions applied) to `target`.
    pub fn save_to(&self, target: PathBuf, opts: SaveOptions, after: AfterSave) {
        self.run_save(target, opts, after, false);
    }

    /// Like `save_to` but the window keeps editing the current file.
    pub fn save_to_copy(&self, target: PathBuf, opts: SaveOptions) {
        self.run_save(target, opts, AfterSave::Nothing, true);
    }

    fn run_save(&self, target: PathBuf, mut opts: SaveOptions, after: AfterSave, copy: bool) {
        let Some((doc_id, path)) = self.st.borrow().doc.as_ref().map(|d| (d.id, d.path.clone())) else { return };
        let in_place = path
            .as_ref()
            .is_some_and(|p| crate::storage::settings::normalise(p) == crate::storage::settings::normalise(&target));
        // Keep the original before the first overwrite in this session.
        if in_place && !self.st.borrow().original_saved {
            if let Some(p) = &path
                && let Err(e) = crate::storage::originals::backup(p)
            {
                self.error("The original couldn’t be preserved, so the file was not saved.", &e.to_string());
                return;
            }
            self.st.borrow_mut().original_saved = true;
        }
        let annots: Vec<Annotation> = self.annotations();
        let redactions: Vec<(usize, Rect)> =
            annots.iter().filter(|a| matches!(a.shape, Shape::Redaction)).map(|a| (a.page, a.bounds)).collect();
        if opts.reopen_password.is_none() {
            opts.reopen_password = self.open_password.borrow().clone();
        }
        let win = self.id;
        self.ui.set_busy_text("Saving…".into());
        self.engine.submit(Priority::Interactive, None, move |s| {
            let result = (|| -> Result<(), String> {
                let doc = s.doc(doc_id).ok_or("The document is no longer open.")?;
                let mut by_page: std::collections::BTreeMap<usize, Vec<Rect>> = Default::default();
                for (p, r) in &redactions {
                    by_page.entry(*p).or_default().push(*r);
                }
                for (p, rs) in by_page {
                    redact::redact_page(doc, p, &rs)?;
                }
                let bytes = annot_io::save(doc, &annots, &opts)?;
                crate::storage::write_atomic(&target, &bytes).map_err(|e| e.to_string())?;
                Ok(())
            })();
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_saved(doc_id, target, result, after, copy));
            });
        });
    }

    fn on_saved(&self, doc_id: DocId, target: PathBuf, result: Result<(), String>, after: AfterSave, copy: bool) {
        self.ui.set_busy_text(Default::default());
        if self.st.borrow().doc.as_ref().is_none_or(|d| d.id != doc_id) {
            return;
        }
        if let Err(e) = result {
            self.error(&format!("“{}” couldn’t be saved.", file_name(&target)), &e);
            return;
        }
        let had_redactions = self.has_redactions();
        {
            let mut st = self.st.borrow_mut();
            // Applied redactions are now part of the document.
            st.annots.retain(|a| !matches!(a.shape, Shape::Redaction));
            if had_redactions {
                // Redaction cannot be undone past this point.
                st.undo.clear();
                st.redo.clear();
            }
            if !copy {
                st.dirty = false;
                if let Some(d) = st.doc.as_mut()
                    && d.path.as_ref() != Some(&target)
                {
                    d.title = file_name(&target);
                    d.path = Some(target.clone());
                    st.original_saved = false;
                }
            }
        }
        if had_redactions {
            self.invalidate_renders();
        }
        self.sync_undo_ui();
        if !copy {
            self.ui.set_edited(false);
            self.ui.set_doc_title(self.title().into());
            super::super::update_settings(|s| s.add_recent(&target));
            super::super::refresh_window_menus();
        }
        *self.last_saved.borrow_mut() = Some(target);
        self.refresh_annotations();
        if after == AfterSave::Close {
            self.st.borrow_mut().dirty = false;
            self.close();
        }
    }

    /// Drops every cached bitmap so pages re-render (after content changed).
    pub(crate) fn invalidate_renders(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.page_cache.as_mut().unwrap().clear();
            st.thumb_cache.as_mut().unwrap().clear();
            st.sheet_cache.as_mut().unwrap().clear();
            st.pending_pages.clear();
            st.pending_thumbs.clear();
            st.pending_sheet.clear();
            st.tile = None;
            st.pending_tile = None;
            st.links.clear();
            st.page_text.clear();
            st.lens_cache.clear();
        }
        self.render_gen.bump();
        self.thumb_gen.bump();
        self.update_view();
        self.update_thumbs();
    }

    /// Called when the window is asked to close. Returns true if it may close now.
    pub(crate) fn confirm_close(&self) -> bool {
        self.finish_pending_edit();
        if !self.is_dirty() {
            self.end_slideshow();
            self.release_document();
            return true;
        }
        if !super::super::context().winit {
            return false;
        }
        let r = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Paper Mirror")
            .set_description(format!("Do you want to save the changes you made to “{}”?", self.title()))
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom("Save".into(), "Don’t Save".into(), "Cancel".into()))
            .set_parent(&self.ui.window().window_handle())
            .show();
        match r {
            rfd::MessageDialogResult::Custom(s) if s == "Save" => {
                let Some(path) = self.doc_path() else { return false };
                let target = if self.has_redactions() {
                    match self.choose_save_path(&path, &redacted_name(&path)) {
                        Some(p) => p,
                        None => return false,
                    }
                } else {
                    path
                };
                self.save_to(target, SaveOptions::default(), AfterSave::Close);
                false
            }
            rfd::MessageDialogResult::Custom(s) if s == "Don’t Save" => {
                self.end_slideshow();
                self.st.borrow_mut().dirty = false;
                self.release_document();
                true
            }
            _ => false,
        }
    }
}

fn redacted_name(path: &std::path::Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Document".into());
    format!("{stem} (Redacted).pdf")
}
