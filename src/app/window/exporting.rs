//! Export as PDF (effects, size reduction, password, flattening), Duplicate,
//! Revert To and Lock.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ModelRc, SharedString, VecModel};

use super::{DocWindow, file_name};
use crate::images::encode::{EncodeOptions, ImageTarget, SaveFormat};
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

    /// File > Export… (`pdf` false) or Export as PDF… (`pdf` true).
    pub(crate) fn show_export(&self, pdf: bool) {
        self.finish_pending_edit();
        if self.is_model() {
            let choices = DocWindow::model_export_choices();
            let names: Vec<SharedString> = choices
                .iter()
                .map(|c| match c {
                    super::model::ModelChoice::Mesh(f) => SharedString::from(f.name()),
                    super::model::ModelChoice::Image(f) => {
                        SharedString::from(format!("{} image of the view", f.name()))
                    }
                })
                .collect();
            self.st.borrow_mut().model_export = choices;
            self.ui.set_export_formats(ModelRc::new(VecModel::from(names)));
            self.ui.set_export_format(0);
            self.ui.set_export_title("Export".into());
            self.ui.set_export_quality(crate::app::window::imaging::DEFAULT_QUALITY as f32);
            self.export_format_changed();
            self.ui.set_export_error(Default::default());
            self.ui.set_export_visible(true);
            return;
        }
        // Images list their own format first; PDFs list PDF first.
        let source = self.image_save_format();
        let mut formats: Vec<Option<SaveFormat>> = Vec::new();
        if self.is_image() {
            formats.extend(source.map(Some));
            formats.extend(SaveFormat::ALL.into_iter().filter(|f| Some(*f) != source).map(Some));
            formats.push(None);
        } else {
            formats.push(None);
            formats.extend([SaveFormat::Png, SaveFormat::Jpeg, SaveFormat::Heic, SaveFormat::Tiff].map(Some));
        }
        let names: Vec<SharedString> =
            formats.iter().map(|f| SharedString::from(f.map(|f| f.name()).unwrap_or("PDF"))).collect();
        let index = if pdf { formats.iter().position(Option::is_none).unwrap_or(0) } else { 0 };
        self.st.borrow_mut().export_formats = formats;
        self.ui.set_export_formats(ModelRc::new(VecModel::from(names)));
        self.ui.set_export_format(index as i32);
        self.ui.set_export_title(if pdf { "Export as PDF" } else { "Export" }.into());
        self.ui.set_export_quality(crate::app::window::imaging::DEFAULT_QUALITY as f32);
        self.ui.set_export_resolution("150".into());
        self.export_format_changed();
        self.ui.set_export_error(Default::default());
        self.ui.set_export_password(Default::default());
        self.ui.set_export_password2(Default::default());
        self.ui.set_export_visible(true);
    }

    fn export_format(&self) -> Option<SaveFormat> {
        let i = self.ui.get_export_format().max(0) as usize;
        self.st.borrow().export_formats.get(i).copied().flatten()
    }

    /// Shows the options that apply to the chosen format.
    pub(crate) fn export_format_changed(&self) {
        if self.is_model() {
            let i = self.ui.get_export_format().max(0) as usize;
            let c = self.st.borrow().model_export.get(i).copied();
            self.ui.set_export_is_pdf(false);
            self.ui.set_export_has_resolution(false);
            self.ui.set_export_has_quality(matches!(c, Some(super::model::ModelChoice::Image(f)) if f.has_quality()));
            return;
        }
        let f = self.export_format();
        self.ui.set_export_is_pdf(f.is_none());
        self.ui.set_export_has_quality(f.is_some_and(SaveFormat::has_quality));
        self.ui.set_export_has_resolution(f.is_some() && !self.is_image());
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
        if self.is_model() {
            let i = self.ui.get_export_format().max(0) as usize;
            let quality = self.ui.get_export_quality().round().clamp(1.0, 100.0) as u8;
            if let Some(c) = self.st.borrow().model_export.get(i).copied() {
                self.model_export_confirm(c, quality);
            }
            return;
        }
        if let Some(format) = self.export_format() {
            self.export_image_confirm(format);
            return;
        }
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

    fn export_image_confirm(&self, format: SaveFormat) {
        let quality = self.ui.get_export_quality().round().clamp(1.0, 100.0) as u8;
        let target = if self.is_image() {
            self.image_target(format, quality)
        } else {
            let dpi = match self.ui.get_export_resolution().trim().parse::<f32>() {
                Ok(d) if (10.0..=1200.0).contains(&d) => d,
                _ => {
                    self.ui.set_export_error("Enter a resolution from 10 to 1200 pixels/inch.".into());
                    return;
                }
            };
            Some(ImageTarget {
                format,
                page: Some(self.current_page()),
                options: EncodeOptions { quality, dpi: (dpi, dpi), ..Default::default() },
                delays: Vec::new(),
            })
        };
        let Some(target) = target else { return };
        let stem = self
            .doc_path()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| self.title());
        let current = self.doc_path().unwrap_or_default();
        let Some(path) = self.choose_image_path(&current, &stem, format) else { return };
        if self.is_image() && !self.confirm_frames_dropped(format) {
            return;
        }
        self.ui.set_export_visible(false);
        self.export_image_to(path, target);
    }

    /// Writes an image copy (export); the window keeps editing the current document.
    pub fn export_image_to(&self, path: PathBuf, mut target: ImageTarget) {
        if self
            .doc_path()
            .is_some_and(|p| crate::storage::settings::normalise(&p) == crate::storage::settings::normalise(&path))
        {
            self.error("Choose a different name.", "Export never replaces the document you are editing.");
            return;
        }
        if let Some(f) = path.extension().and_then(|e| SaveFormat::from_extension(&e.to_string_lossy())) {
            target.format = f;
        }
        self.save_to_copy(path, SaveOptions { image: Some(target), ..Default::default() });
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
        let image = self.st.borrow().image.clone();
        let title = match &image {
            Some(_) => format!(
                "{} copy",
                self.doc_path()
                    .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| self.title())
            ),
            None => format!("{} copy", self.title().trim_end_matches(".pdf")),
        };
        let win = self.id;
        self.engine.submit(Priority::Interactive, None, move |s| {
            let result = s
                .doc(doc_id)
                .ok_or_else(|| "closed".to_string())
                .and_then(|d| annot_io::save(d, &annots, &SaveOptions::default()));
            let _ = slint::invoke_from_event_loop(move || match result {
                Ok(bytes) => match DocWindow::open_new(None) {
                    Ok(w) => w.open_memory(Arc::new(bytes), title, image),
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
