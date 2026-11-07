//! The 3D view (ADR 0009): loading off the UI thread, a render thread per request
//! (coalesced so only the latest camera is drawn), orbit/pan/zoom, orientation edits,
//! Save and Export.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use super::{DocWindow, file_name};
use crate::core::filetype::ModelFormat;
use crate::core::geometry::Point;
use crate::images::encode::{self, EncodeOptions, SaveFormat};
use crate::model3d::export::ModelExport;
use crate::model3d::raster::{self, Camera};
use crate::model3d::{self, Mesh, ModelInfo};
use crate::ui::PageItem;

pub(crate) struct ModelState {
    pub mesh: Arc<Mesh>,
    pub camera: Camera,
    pub format: ModelFormat,
    pub info: ModelInfo,
    pub drag: Option<(Point, bool)>,
    pub image: Option<Image>,
    /// A render is running; `dirty` asks for another when it finishes.
    pub rendering: bool,
    pub dirty: bool,
    pub generation: u64,
    pub first_logged: bool,
}

/// Export choices for a model: a mesh format or an image of the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelChoice {
    Mesh(ModelExport),
    Image(SaveFormat),
}

fn info_for(mesh: &Mesh, format: ModelFormat) -> ModelInfo {
    let (lo, hi) = mesh.bounds();
    ModelInfo {
        format: model3d::format_name(format),
        triangles: mesh.triangles.len(),
        vertices: mesh.positions.len(),
        has_colors: mesh.colors.is_some(),
        size: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
    }
}

impl DocWindow {
    pub fn is_model(&self) -> bool {
        self.st.borrow().model.is_some()
    }

    pub(crate) fn open_model(&self, path: PathBuf, format: ModelFormat) {
        if let Some(reason) = model3d::unsupported_reason(format) {
            self.error(
                &format!("“{}” can’t be shown.", file_name(&path)),
                &format!("{} files aren’t supported. {reason}", model3d::format_name(format)),
            );
            return;
        }
        let win = self.id;
        self.ui.set_busy_text("Opening…".into());
        std::thread::spawn(move || {
            let result = model3d::load(&path, format);
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_model_opened(path, format, result));
            });
        });
    }

    fn on_model_opened(&self, path: PathBuf, format: ModelFormat, result: Result<Mesh, String>) {
        self.ui.set_busy_text(Default::default());
        let mesh = match result {
            Ok(m) => m,
            Err(e) => {
                self.error(&format!("“{}” could not be opened.", file_name(&path)), &e);
                return;
            }
        };
        super::super::measure("document-opened");
        self.release_document();
        let camera = Camera::fit(&mesh);
        let info = info_for(&mesh, format);
        {
            let mut st = self.st.borrow_mut();
            st.model = Some(ModelState {
                mesh: Arc::new(mesh),
                camera,
                format,
                info,
                drag: None,
                image: None,
                rendering: false,
                dirty: false,
                generation: 0,
                first_logged: false,
            });
            st.model_path = Some(path.clone());
        }
        let title = file_name(&path);
        self.ui.set_doc_title(title.into());
        self.ui.set_is_model(true);
        self.ui.set_has_document(true);
        self.ui.set_has_file(true);
        self.ui.set_multi_page(false);
        self.ui.set_doc_subtitle("Drag to turn · Shift+drag to move · scroll or pinch to zoom".into());
        super::super::update_settings(|s| s.add_recent(&path));
        super::super::refresh_window_menus();
        if self.ui.get_inspector_visible() {
            self.refresh_inspector();
        }
        self.request_model_render();
        self.ui.invoke_focus_canvas();
        for c in &super::super::context().options.commands {
            self.command(c);
        }
    }

    /// Draws the latest camera at the view size: full quality when idle, a quicker
    /// pass while dragging.
    pub(crate) fn request_model_render(&self) {
        let (mesh, camera, generation, quick) = {
            let mut st = self.st.borrow_mut();
            let Some(m) = st.model.as_mut() else { return };
            if m.rendering {
                m.dirty = true;
                return;
            }
            m.rendering = true;
            m.dirty = false;
            m.generation += 1;
            (m.mesh.clone(), m.camera, m.generation, m.drag.is_some())
        };
        let vp = self.viewport();
        let sf = self.ui.window().scale_factor();
        let (w, h) = (((vp.w * sf).round() as u32).max(1), ((vp.h * sf).round() as u32).max(1));
        // While dragging, draw at half resolution without supersampling.
        let (rw, rh, ss) = if quick { ((w / 2).max(1), (h / 2).max(1), 1) } else { (w, h, 2) };
        let win = self.id;
        std::thread::spawn(move || {
            let px = raster::render(&mesh, &camera, rw, rh, ss);
            let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px, rw, rh);
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_model_rendered(generation, buf));
            });
        });
    }

    fn on_model_rendered(&self, generation: u64, buf: SharedPixelBuffer<Rgba8Pixel>) {
        let (again, first) = {
            let mut st = self.st.borrow_mut();
            let Some(m) = st.model.as_mut() else { return };
            if m.generation != generation {
                return;
            }
            m.rendering = false;
            m.image = Some(Image::from_rgba8(buf));
            let first = !m.first_logged;
            m.first_logged = true;
            (m.dirty, first)
        };
        self.show_model_image();
        if again {
            self.request_model_render();
        }
        if first {
            super::super::measure("first-page-rendered");
            if super::super::context().options.exit_after_render {
                let win = self.id;
                slint::Timer::single_shot(Duration::from_millis(600), move || {
                    super::super::measure("after-first-paint");
                    if let Some(path) = super::super::context().options.snapshot {
                        super::super::with_window(win, |w| w.save_snapshot(&path));
                    }
                    let _ = slint::quit_event_loop();
                });
            }
        }
    }

    /// The rendered view fills the canvas as a single "page".
    fn show_model_image(&self) {
        let vp = self.viewport();
        let image = self.st.borrow().model.as_ref().and_then(|m| m.image.clone());
        let Some(image) = image else { return };
        self.ui.set_content_width(vp.w);
        self.ui.set_content_height(vp.h);
        self.pages_model.set_vec(vec![PageItem {
            index: 0,
            x: 0.0,
            y: 0.0,
            width: vp.w,
            height: vp.h,
            image,
            has_image: true,
            label: "3D model".into(),
            ..Default::default()
        }]);
    }

    pub(crate) fn model_viewport_changed(&self) {
        self.show_model_image();
        self.request_model_render();
    }

    /// Pointer input: drag orbits, Shift- or right-drag pans.
    pub(crate) fn model_pointer(&self, kind: i32, p: Point, shift: bool, ctrl: bool, button: i32) {
        use super::tools::{POINTER_CANCEL, POINTER_DOWN, POINTER_MOVE, POINTER_UP};
        let changed = {
            let mut st = self.st.borrow_mut();
            let Some(m) = st.model.as_mut() else { return };
            match kind {
                POINTER_DOWN => {
                    m.drag = Some((p, shift || ctrl || button != 0));
                    false
                }
                POINTER_MOVE => match m.drag {
                    Some((last, pan)) => {
                        let (dx, dy) = (p.x - last.x, p.y - last.y);
                        if pan {
                            let h = self.ui.get_view_height().max(1.0);
                            m.camera.pan(dx, dy, h);
                        } else {
                            m.camera.orbit(dx, dy);
                        }
                        m.drag = Some((p, pan));
                        true
                    }
                    None => false,
                },
                POINTER_UP | POINTER_CANCEL => m.drag.take().is_some(),
                _ => false,
            }
        };
        if changed {
            self.request_model_render();
        }
    }

    pub(crate) fn model_zoom(&self, factor: f32) {
        if let Some(m) = self.st.borrow_mut().model.as_mut() {
            m.camera.zoom(factor);
        }
        self.request_model_render();
    }

    pub(crate) fn model_reset_view(&self) {
        if let Some(m) = self.st.borrow_mut().model.as_mut() {
            m.camera = Camera::fit(&m.mesh);
        }
        self.request_model_render();
    }

    pub(crate) fn model_orbit_by(&self, dx: f32, dy: f32) {
        if let Some(m) = self.st.borrow_mut().model.as_mut() {
            m.camera.orbit(dx, dy);
        }
        self.request_model_render();
    }

    /// Orientation edits (Rotate Left/Right, Flip Up Axis) change the mesh itself.
    pub(crate) fn model_edit(&self, f: impl FnOnce(&mut Mesh)) {
        {
            let mut st = self.st.borrow_mut();
            let Some(m) = st.model.as_mut() else { return };
            let mut mesh = (*m.mesh).clone();
            f(&mut mesh);
            m.info = info_for(&mesh, m.format);
            m.camera = Camera::fit(&mesh);
            m.mesh = Arc::new(mesh);
        }
        self.mark_dirty();
        self.request_model_render();
        if self.ui.get_inspector_visible() {
            self.refresh_inspector();
        }
    }

    /// The mesh as it is now (for tests and export).
    pub fn model_mesh(&self) -> Option<Arc<Mesh>> {
        self.st.borrow().model.as_ref().map(|m| m.mesh.clone())
    }

    pub fn model_camera(&self) -> Option<Camera> {
        self.st.borrow().model.as_ref().map(|m| m.camera)
    }

    /// Save: back to the same format when Paper Mirror can write it (glTF JSON is
    /// saved as GLB via Save As).
    pub(crate) fn save_model(&self, save_as: bool) {
        let path = self.st.borrow().model_path.clone();
        let Some(path) = path else { return };
        let format = self.st.borrow().model.as_ref().map(|m| m.format);
        let writable = match format {
            Some(ModelFormat::Stl) => Some(ModelExport::Stl),
            Some(ModelFormat::Obj) => Some(ModelExport::Obj),
            Some(ModelFormat::Ply) => Some(ModelExport::Ply),
            Some(ModelFormat::Glb) => Some(ModelExport::Glb),
            _ => None,
        };
        let target = match writable {
            Some(_) if !save_as => {
                if self.is_locked() {
                    self.error(&format!("“{}” is locked.", file_name(&path)), "Use Save As to save a copy.");
                    return;
                }
                path.clone()
            }
            _ => match self.choose_model_path(&path, writable.unwrap_or(ModelExport::Glb)) {
                Some(p) => p,
                None => return,
            },
        };
        self.save_model_to(target, false);
    }

    fn choose_model_path(&self, current: &std::path::Path, first: ModelExport) -> Option<PathBuf> {
        if !super::super::context().winit {
            return None;
        }
        let stem = current.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Model".into());
        let mut d = rfd::FileDialog::new()
            .set_title("Save As")
            .set_file_name(format!("{stem}.{}", first.extension()))
            .set_parent(&self.ui.window().window_handle())
            .add_filter(first.name(), &[first.extension()]);
        for f in ModelExport::ALL.into_iter().filter(|f| *f != first) {
            d = d.add_filter(f.name(), &[f.extension()]);
        }
        if let Some(dir) = current.parent() {
            d = d.set_directory(dir);
        }
        d.save_file().map(|p| {
            let known = p.extension().and_then(|e| ModelExport::from_extension(&e.to_string_lossy())).is_some();
            if known { p } else { p.with_extension(first.extension()) }
        })
    }

    /// Writes the mesh in the format the extension names. `copy` keeps the window on
    /// its current file (Export).
    pub fn save_model_to(&self, target: PathBuf, copy: bool) {
        let Some(mesh) = self.model_mesh() else { return };
        let format = target
            .extension()
            .and_then(|e| ModelExport::from_extension(&e.to_string_lossy()))
            .unwrap_or(ModelExport::Glb);
        let current = self.st.borrow().model_path.clone();
        let in_place = current
            .as_ref()
            .is_some_and(|p| crate::storage::settings::normalise(p) == crate::storage::settings::normalise(&target));
        if in_place && !self.st.borrow().original_saved {
            if let Err(e) = crate::storage::originals::backup(&target) {
                self.error("The original couldn’t be preserved, so the file was not saved.", &e.to_string());
                return;
            }
            self.st.borrow_mut().original_saved = true;
        }
        let win = self.id;
        self.ui.set_busy_text("Saving…".into());
        std::thread::spawn(move || {
            let bytes = format.write(&mesh);
            let result = crate::storage::write_atomic(&target, &bytes).map_err(|e| e.to_string());
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_model_saved(target, result, copy));
            });
        });
    }

    fn on_model_saved(&self, target: PathBuf, result: Result<(), String>, copy: bool) {
        self.ui.set_busy_text(Default::default());
        if let Err(e) = result {
            self.error(&format!("“{}” couldn’t be saved.", file_name(&target)), &e);
            return;
        }
        if !copy {
            let mut st = self.st.borrow_mut();
            st.dirty = false;
            if st.model_path.as_ref() != Some(&target) {
                st.original_saved = false;
            }
            st.model_path = Some(target.clone());
            if let (Some(m), Some(f)) = (st.model.as_mut(), target.extension()) {
                m.format = match f.to_string_lossy().to_ascii_lowercase().as_str() {
                    "stl" => ModelFormat::Stl,
                    "obj" => ModelFormat::Obj,
                    "ply" => ModelFormat::Ply,
                    _ => ModelFormat::Glb,
                };
            }
            drop(st);
            self.ui.set_edited(false);
            self.ui.set_doc_title(file_name(&target).into());
            super::super::update_settings(|s| s.add_recent(&target));
            super::super::refresh_window_menus();
        }
        *self.last_saved.borrow_mut() = Some(target);
    }

    /// Writes an image of the current view at twice the window size.
    pub fn export_model_image(&self, target: PathBuf, format: SaveFormat, quality: u8) {
        let Some((mesh, camera)) = self.st.borrow().model.as_ref().map(|m| (m.mesh.clone(), m.camera)) else {
            return;
        };
        let vp = self.viewport();
        let (w, h) = (((vp.w * 2.0) as u32).clamp(64, 8000), ((vp.h * 2.0) as u32).clamp(64, 8000));
        let win = self.id;
        self.ui.set_busy_text("Exporting…".into());
        std::thread::spawn(move || {
            let px = raster::render(&mesh, &camera, w, h, 2);
            let frame = crate::images::Frame::new(w, h, px);
            let opts = EncodeOptions { quality, dpi: (144.0, 144.0), ..Default::default() };
            let result = encode::encode(&[frame], format, &opts)
                .and_then(|b| crate::storage::write_atomic(&target, &b).map_err(|e| e.to_string()));
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_model_saved(target, result, true));
            });
        });
    }

    pub(crate) fn model_export_choices() -> Vec<ModelChoice> {
        let mut v: Vec<ModelChoice> = ModelExport::ALL.into_iter().map(ModelChoice::Mesh).collect();
        v.extend([SaveFormat::Png, SaveFormat::Jpeg, SaveFormat::Tiff, SaveFormat::Heic].map(ModelChoice::Image));
        v
    }

    pub(crate) fn model_export_confirm(&self, choice: ModelChoice, quality: u8) {
        let current = self.st.borrow().model_path.clone().unwrap_or_default();
        let stem = current.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Model".into());
        match choice {
            ModelChoice::Mesh(f) => {
                let Some(target) = self.choose_model_path(&current, f) else { return };
                self.ui.set_export_visible(false);
                if crate::storage::settings::normalise(&target) == crate::storage::settings::normalise(&current) {
                    self.error("Choose a different name.", "Export never replaces the model you are viewing.");
                    return;
                }
                self.save_model_to(target, true);
            }
            ModelChoice::Image(f) => {
                let Some(target) = self.choose_image_path(&current, &stem, f) else { return };
                self.ui.set_export_visible(false);
                self.export_model_image(target, f, quality);
            }
        }
    }

    pub(crate) fn model_info_rows(&self) -> Vec<(String, String)> {
        let st = self.st.borrow();
        let Some(m) = st.model.as_ref() else { return Vec::new() };
        let i = &m.info;
        let fmt = |v: f32| {
            let s = format!("{v:.3}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        };
        vec![
            ("Kind".into(), format!("{} 3D model", i.format)),
            ("Triangles".into(), group(i.triangles)),
            ("Vertices".into(), group(i.vertices)),
            ("Size".into(), format!("{} × {} × {} (model units)", fmt(i.size[0]), fmt(i.size[1]), fmt(i.size[2]))),
            ("Colours".into(), if i.has_colors { "Yes" } else { "No" }.into()),
        ]
    }
}

/// 1234567 -> "1,234,567".
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn grouping() {
        assert_eq!(super::group(7), "7");
        assert_eq!(super::group(1234567), "1,234,567");
    }
}
