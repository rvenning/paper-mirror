//! Export as PDF (effects, size reduction, password, flattening), Duplicate,
//! Revert To and Lock.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ModelRc, SharedString, VecModel};

use super::{DocWindow, file_name};
use crate::pdf::annot_io::{self, SaveOptions};
use crate::pdf::effects::Effect;
use crate::pdf::engine::Priority;
use crate::pdf::security::Protection;

impl DocWindow {
    pub(crate) fn init_export_ui(&self) {
        let mut names: Vec<SharedString> = vec!["None".into()];
        names.extend(Effect::ALL.iter().map(|e| SharedString::from(e.name())));
        self.ui.set_export_effects(ModelRc::new(VecModel::from(names)));
    }

    pub(crate) fn show_export(&self) {
        self.finish_pending_edit();
        self.ui.set_export_error(Default::default());
        self.ui.set_export_password(Default::default());
        self.ui.set_export_password2(Default::default());
        self.ui.set_export_visible(true);
    }

    /// Options chosen in the Export panel, or an error to show there.
    pub(crate) fn export_options(&self) -> Result<SaveOptions, String> {
        let effect = match self.ui.get_export_effect() {
            i if i >= 1 => Effect::ALL.get(i as usize - 1).copied(),
            _ => None,
        };
        let protection = if self.ui.get_export_encrypt() {
            let p1 = self.ui.get_export_password().to_string();
            let p2 = self.ui.get_export_password2().to_string();
            if p1.is_empty() {
                return Err("Enter a password.".into());
            }
            if p1 != p2 {
                return Err("The passwords don’t match.".into());
            }
            Some(Protection {
                open_password: p1,
                allow_printing: true,
                allow_copying: true,
                allow_editing: true,
                ..Default::default()
            })
        } else {
            None
        };
        Ok(SaveOptions {
            flatten: self.ui.get_export_flatten(),
            protection,
            effect,
            reduce_size: self.ui.get_export_reduce(),
            ..Default::default()
        })
    }

    pub(crate) fn export_confirm(&self) {
        let opts = match self.export_options() {
            Ok(o) => o,
            Err(e) => {
                self.ui.set_export_error(e.into());
                return;
            }
        };
        let stem = self
            .doc_path()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| self.title().trim_end_matches(".pdf").to_string());
        let current = self.doc_path().unwrap_or_default();
        let Some(target) = self.choose_save_path(&current, &format!("{stem} (Exported).pdf")) else { return };
        self.ui.set_export_visible(false);
        self.export_to(target, opts);
    }

    /// Writes an exported copy; the window keeps editing the current document.
    pub fn export_to(&self, target: PathBuf, opts: SaveOptions) {
        if self
            .doc_path()
            .is_some_and(|p| crate::storage::settings::normalise(&p) == crate::storage::settings::normalise(&target))
        {
            self.error("Choose a different name.", "Export never replaces the document you are editing.");
            return;
        }
        self.save_to_copy(target, opts);
    }

    /// File > Duplicate: a new, untitled window with the current state.
    pub fn duplicate(&self) {
        self.finish_pending_edit();
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let annots = self.annotations();
        let title = format!("{} copy", self.title().trim_end_matches(".pdf"));
        let win = self.id;
        self.engine.submit(Priority::Interactive, None, move |s| {
            let result = s
                .doc(doc_id)
                .ok_or_else(|| "closed".to_string())
                .and_then(|d| annot_io::save(d, &annots, &SaveOptions::default()));
            let _ = slint::invoke_from_event_loop(move || match result {
                Ok(bytes) => match DocWindow::open_new(None) {
                    Ok(w) => w.open_memory(Arc::new(bytes), title),
                    Err(e) => super::super::with_window(win, |w| w.error("Couldn’t duplicate.", &e)),
                },
                Err(e) => super::super::with_window(win, |w| w.error("Couldn’t duplicate.", &e)),
            });
        });
    }

    /// File > Revert To > Last Saved Version.
    pub fn revert_to_saved(&self) {
        let Some(path) = self.doc_path() else { return };
        if self.is_dirty() && !self.confirm("Revert to the last saved version?", "Your unsaved changes will be lost.") {
            return;
        }
        self.reload(path);
    }

    /// File > Revert To > Original: restores the copy kept before the first save.
    pub fn revert_to_original(&self) {
        let Some(path) = self.doc_path() else { return };
        let Some(original) = crate::storage::originals::original_for(&path) else {
            self.error(
                "There’s no earlier original.",
                "Paper Mirror keeps the original of a file the first time it saves over it.",
            );
            return;
        };
        if !self.confirm(
            "Revert to the original?",
            &format!(
                "“{}” will be replaced by the version you first opened. Your later changes will be lost.",
                file_name(&path)
            ),
        ) {
            return;
        }
        let bytes = match std::fs::read(&original) {
            Ok(b) => b,
            Err(e) => {
                self.error("The original couldn’t be read.", &e.to_string());
                return;
            }
        };
        self.release_document();
        if let Err(e) = crate::storage::write_atomic(&path, &bytes) {
            self.error("The original couldn’t be restored.", &e.to_string());
        }
        self.open_path(path);
    }

    fn reload(&self, path: PathBuf) {
        self.release_document();
        self.open_path(path);
    }

    /// File > Lock / Unlock: the read-only attribute, like Finder's lock.
    pub fn toggle_lock(&self) {
        let Some(path) = self.doc_path() else { return };
        let Ok(meta) = std::fs::metadata(&path) else { return };
        let mut perms = meta.permissions();
        let lock = !perms.readonly();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(lock);
        match std::fs::set_permissions(&path, perms) {
            Ok(()) => self.ui.set_locked(lock),
            Err(e) => self.error("The lock couldn’t be changed.", &e.to_string()),
        }
    }

    pub fn is_locked(&self) -> bool {
        self.doc_path().and_then(|p| std::fs::metadata(p).ok()).is_some_and(|m| m.permissions().readonly())
    }
}
