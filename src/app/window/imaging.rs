//! Images in a document window (ADR 0007): opening (decoding runs on its own
//! thread), saving back to image formats, Flip, Adjust Size, Adjust Color, Instant
//! Alpha, location metadata and New from Clipboard.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use super::{AfterSave, DocWindow, Snapshot, Tool, file_name};
use crate::core::filetype::ImageFormat;
use crate::core::geometry::{Affine, Point, Size};
use crate::core::imagesize::{SizeModel, Unit};
use crate::images::edit::{self, ColorAdjust};
use crate::images::encode::{EncodeOptions, ImageTarget, SaveFormat};
use crate::images::{Decoded, DisplayFilter, Frame, ImageInfo, PT_PER_PX, metadata};
use crate::pdf::annot_io::SaveOptions;
use crate::pdf::engine::{DocId, EngineState, Priority};
use crate::pdf::imagedoc;

/// What the window knows about an open image besides its pages.
#[derive(Clone)]
pub(crate) struct ImageMeta {
    pub info: ImageInfo,
    /// Frame delays (animated GIF), by page.
    pub delays: Vec<u32>,
    /// The user chose to see the location (it is hidden until then).
    pub show_location: bool,
    /// Remove location information on the next save.
    pub strip_location: bool,
    pub icc: Option<Arc<moxcms::Transform8BitExecutor>>,
}

impl ImageMeta {
    pub fn format(&self) -> ImageFormat {
        self.info.format.unwrap_or(ImageFormat::Png)
    }

    fn has_location(&self) -> bool {
        self.info.exif.as_deref().is_some_and(metadata::has_location)
    }

    /// Encoder options for saving: metadata kept, orientation reset, location removed
    /// if asked.
    fn encode_options(&self, quality: u8) -> EncodeOptions {
        EncodeOptions {
            quality,
            icc: self.info.icc.clone(),
            exif: self.info.exif.as_deref().and_then(|e| metadata::rewrite(e, self.strip_location)),
            dpi: self.info.dpi,
        }
    }
}

/// An Instant Alpha drag: the page's pixels, the seed and the current selection.
pub(crate) struct AlphaDrag {
    pub page: usize,
    pub seed: (u32, u32),
    pub start: Point,
    pub frame: Arc<Frame>,
    pub tolerance: f32,
    pub mask: Arc<Vec<bool>>,
}

/// Tolerance at the click, and how fast dragging widens it (per logical pixel).
const ALPHA_BASE_TOLERANCE: f32 = 0.06;
const ALPHA_TOLERANCE_PER_PX: f32 = 1.0 / 250.0;
/// Default JPEG/HEIC quality for Save.
pub(crate) const DEFAULT_QUALITY: u8 = 90;

type Opened = (Vec<Size>, ImageMeta);
/// Undo bytes, page sizes before, page sizes after.
type PixelEditResult = Result<(Arc<Vec<u8>>, Vec<Size>, Vec<Size>), String>;

/// Engine side of opening: builds the page document and registers it.
fn install(st: &mut EngineState, id: DocId, path: Option<PathBuf>, d: Decoded) -> Result<Opened, String> {
    let delays = d.frames.iter().map(|f| f.delay_ms).collect();
    let mut doc = imagedoc::build(d.frames)?;
    doc.path = path;
    let icc = d.info.icc.as_deref().and_then(crate::images::display_transform);
    if let Some(t) = &icc {
        doc.filter = Some(Arc::new(DisplayFilter { adjust: None, icc: Some(t.clone()) }));
    }
    doc.checkerboard = d.info.has_alpha;
    let sizes = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
    st.docs.insert(id, doc);
    Ok((sizes, ImageMeta { info: d.info, delays, show_location: false, strip_location: false, icc }))
}

impl DocWindow {
    pub fn is_image(&self) -> bool {
        self.st.borrow().image.is_some()
    }

    pub(crate) fn image_dpi(&self) -> Option<(f32, f32)> {
        self.st.borrow().image.as_ref().map(|m| m.info.dpi)
    }

    /// Opens an image file: decoding runs on a worker thread, the page document is
    /// built on the engine thread.
    pub(crate) fn open_image(&self, path: PathBuf, format: ImageFormat) {
        let engine = self.engine.clone();
        let id = engine.new_doc_id();
        let win = self.id;
        self.ui.set_busy_text("Opening…".into());
        std::thread::spawn(move || {
            let read = std::fs::read(&path).map_err(|e| e.to_string());
            // JPEG 2000 needs PDFium, so it is decoded on the engine thread.
            let decoded: Result<Result<Decoded, Vec<u8>>, String> = read.and_then(|bytes| {
                if format == ImageFormat::Jpeg2000 {
                    Ok(Err(bytes))
                } else {
                    crate::images::decode::decode(&bytes, format).map(Ok)
                }
            });
            engine.submit(Priority::Interactive, None, move |st| {
                let result = decoded.and_then(|d| {
                    let d = match d {
                        Ok(d) => d,
                        Err(bytes) => imagedoc::decode_jpx(&bytes)?,
                    };
                    install(st, id, Some(path.clone()), d)
                });
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |w| {
                        let title = file_name(&path);
                        w.on_image_opened(id, Some(path), title, result);
                    });
                });
            });
        });
    }

    /// Opens decoded pixels with no file (New from Clipboard).
    pub fn open_image_memory(&self, frames: Vec<Frame>, title: String) {
        let id = self.engine.new_doc_id();
        let win = self.id;
        let info = ImageInfo {
            format: Some(ImageFormat::Png),
            dpi: (72.0, 72.0),
            bits_per_channel: 8,
            color_model: "RGB".into(),
            has_alpha: frames.iter().any(Frame::has_alpha),
            ..Default::default()
        };
        self.engine.submit(Priority::Interactive, None, move |st| {
            let result = install(st, id, None, Decoded { frames, info });
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| {
                    w.on_image_opened(id, None, title, result);
                    w.mark_dirty();
                });
            });
        });
    }

    fn on_image_opened(&self, id: DocId, path: Option<PathBuf>, title: String, result: Result<Opened, String>) {
        self.ui.set_busy_text(Default::default());
        match result {
            Ok((sizes, meta)) => {
                super::super::measure("document-opened");
                self.adopt_document(id, path, title, sizes, Vec::new(), None, Some(meta));
            }
            Err(e) => {
                let name = path.as_deref().map(file_name).unwrap_or(title);
                self.error(&format!("“{name}” could not be opened."), &e);
            }
        }
    }

    /// File > New from Clipboard.
    pub fn new_from_clipboard(&self) {
        let Some((w, h, rgba)) = crate::platform_windows::clipboard::get_image() else {
            self.error(
                "There’s no image on the clipboard.",
                "Copy an image, or take a screenshot with Windows+Shift+S, then try again.",
            );
            return;
        };
        let frames = vec![Frame::new(w, h, rgba)];
        if self.has_document() {
            match DocWindow::open_new(None) {
                Ok(win) => win.open_image_memory(frames, "Untitled".into()),
                Err(e) => self.error("Could not open a new window", &e),
            }
        } else {
            self.open_image_memory(frames, "Untitled".into());
        }
    }

    // ----- Saving -----

    /// The format Save writes for this image, if it can be written.
    pub(crate) fn image_save_format(&self) -> Option<SaveFormat> {
        self.st.borrow().image.as_ref().and_then(|m| SaveFormat::for_source(m.format()))
    }

    /// Save As dialog for images, with `first` as the default type.
    pub(crate) fn choose_image_path(
        &self,
        current: &std::path::Path,
        stem: &str,
        first: SaveFormat,
    ) -> Option<PathBuf> {
        if !super::super::context().winit {
            return None;
        }
        let mut d = rfd::FileDialog::new()
            .set_title("Save As")
            .set_file_name(format!("{stem}.{}", first.extension()))
            .set_parent(&self.ui.window().window_handle());
        d = d.add_filter(first.name(), first.extensions());
        for f in SaveFormat::ALL.into_iter().filter(|f| *f != first) {
            d = d.add_filter(f.name(), f.extensions());
        }
        if let Some(dir) = current.parent() {
            d = d.set_directory(dir);
        }
        d.save_file().map(|p| {
            let known = p.extension().and_then(|e| SaveFormat::from_extension(&e.to_string_lossy())).is_some();
            if known { p } else { p.with_extension(first.extension()) }
        })
    }

    /// Save / Save As for an image document.
    pub(crate) fn save_image(&self, save_as: bool) {
        let path = self.doc_path();
        let stem = path
            .as_ref()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| self.title());
        let source = self.image_save_format();
        let target = match (&path, source, save_as) {
            (Some(p), Some(_), false) => {
                if self.is_locked() {
                    self.error(
                        &format!("“{}” is locked.", file_name(p)),
                        "Unlock it with File > Unlock, or use Save As to save a copy.",
                    );
                    return;
                }
                p.clone()
            }
            _ => {
                let current = path.clone().unwrap_or_default();
                match self.choose_image_path(&current, &stem, source.unwrap_or(SaveFormat::Png)) {
                    Some(p) => p,
                    None => return,
                }
            }
        };
        self.save_image_to(target, DEFAULT_QUALITY, AfterSave::Nothing);
    }

    /// Writes the image to `target` in the format its extension names.
    pub fn save_image_to(&self, target: PathBuf, quality: u8, after: AfterSave) {
        let format = target
            .extension()
            .and_then(|e| SaveFormat::from_extension(&e.to_string_lossy()))
            .or_else(|| self.image_save_format())
            .unwrap_or(SaveFormat::Png);
        let Some(t) = self.image_target(format, quality) else { return };
        if !self.confirm_frames_dropped(format) {
            return;
        }
        let saved_format = format;
        self.save_to(target, SaveOptions { image: Some(t), ..Default::default() }, after);
        self.st.borrow_mut().pending_image_format = Some(saved_format);
    }

    /// Asks before a multi-page image is written to a single-image format.
    pub(crate) fn confirm_frames_dropped(&self, format: SaveFormat) -> bool {
        let n = self.page_count();
        n <= 1
            || format.keeps_all_frames()
            || self.confirm(
                &format!("Save only the first of {n} images?"),
                &format!("{} files hold one image. Choose TIFF or GIF to keep every page.", format.name()),
            )
    }

    pub fn image_target(&self, format: SaveFormat, quality: u8) -> Option<ImageTarget> {
        let st = self.st.borrow();
        let m = st.image.as_ref()?;
        Some(ImageTarget { format, page: None, options: m.encode_options(quality), delays: m.delays.clone() })
    }

    /// After a successful image save: the document now is that format, and removed
    /// location information is gone from memory too.
    pub(crate) fn after_image_saved(&self, copy: bool) {
        let mut st = self.st.borrow_mut();
        let format = st.pending_image_format.take();
        if copy {
            return;
        }
        if let Some(m) = st.image.as_mut() {
            if let Some(f) = format.and_then(image_format_for) {
                m.info.format = Some(f);
            }
            if m.strip_location {
                m.info.exif = m.info.exif.as_deref().and_then(|e| metadata::rewrite(e, true));
                m.strip_location = false;
            }
        }
        drop(st);
        self.sync_location_ui();
        if self.ui.get_inspector_visible() {
            self.refresh_inspector();
        }
    }

    // ----- Pixel edits -----

    /// Replaces `pages` (all when None) with `f(page, pixels)`, as one undo step.
    /// Annotations on pages whose size changes are scaled with them.
    pub(crate) fn pixel_edit(
        &self,
        busy: &str,
        pages: Option<Vec<usize>>,
        f: impl Fn(usize, Frame) -> Result<Frame, String> + Send + 'static,
    ) {
        self.finish_pending_edit();
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let win = self.id;
        self.ui.set_busy_text(busy.into());
        self.engine.submit(Priority::Interactive, None, move |s| {
            let result: PixelEditResult = (|| {
                let doc = s.doc_mut(doc_id).ok_or("The document is no longer open.")?;
                let before = Arc::new(doc.doc.save_to_vec(false).ok_or("The image could not be copied for undo.")?);
                let old: Vec<Size> = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
                let pages = pages.unwrap_or_else(|| (0..doc.page_count()).collect());
                let mut alpha = doc.checkerboard;
                for p in pages {
                    let frame = imagedoc::rasterise(doc, p, false).ok_or("The image could not be read.")?;
                    let new = f(p, frame)?;
                    alpha |= new.has_alpha();
                    imagedoc::replace_page(doc, p, new)?;
                }
                doc.checkerboard = alpha;
                let sizes = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
                Ok((before, old, sizes))
            })();
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_pixel_edit(doc_id, result));
            });
        });
    }

    fn on_pixel_edit(&self, doc_id: DocId, result: PixelEditResult) {
        self.ui.set_busy_text(Default::default());
        if self.st.borrow().doc.as_ref().is_none_or(|d| d.id != doc_id) {
            return;
        }
        let (before, old, sizes) = match result {
            Ok(r) => r,
            Err(e) => {
                self.error("The image couldn’t be changed.", &e);
                return;
            }
        };
        {
            let mut st = self.st.borrow_mut();
            let dpi = st.pending_dpi_undo.take().or_else(|| st.image.as_ref().map(|m| m.info.dpi));
            let annots = std::mem::take(&mut st.annots);
            st.undo.push(Snapshot { annots: annots.clone(), pdf: Some(before), dpi });
            st.redo.clear();
            st.annots = annots
                .into_iter()
                .map(|a| match (old.get(a.page), sizes.get(a.page)) {
                    (Some(o), Some(n)) if (o.w, o.h) != (n.w, n.h) => {
                        let mut moved = a.transformed(&Affine::scale(n.w / o.w, n.h / o.h));
                        moved.page = a.page;
                        moved
                    }
                    _ => a,
                })
                .collect();
            st.selected_annot = None;
        }
        self.trim_snapshots();
        self.apply_new_sizes(sizes);
        self.mark_dirty();
        self.sync_undo_ui();
        if self.ui.get_inspector_visible() {
            self.refresh_inspector();
        }
    }

    pub fn flip(&self, horizontal: bool) {
        if !self.is_image() {
            return;
        }
        let pages = self.selected_pages();
        self.pixel_edit("Flipping…", Some(pages), move |_, mut f| {
            if horizontal {
                edit::flip_horizontal(&mut f);
            } else {
                edit::flip_vertical(&mut f);
            }
            Ok(f)
        });
    }

    /// Crop rectangles on images snap outwards to whole pixels.
    pub(crate) fn snap_to_pixels(r: crate::core::geometry::Rect) -> crate::core::geometry::Rect {
        let x0 = (r.x / PT_PER_PX).floor() * PT_PER_PX;
        let y0 = (r.y / PT_PER_PX).floor() * PT_PER_PX;
        let x1 = (r.right() / PT_PER_PX).ceil() * PT_PER_PX;
        let y1 = (r.bottom() / PT_PER_PX).ceil() * PT_PER_PX;
        crate::core::geometry::Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    // ----- Adjust Size -----

    pub fn show_adjust_size(&self) {
        let Some(dpi) = self.image_dpi() else { return };
        let page = self.current_page();
        let Some(size) = self.page_sizes().get(page).copied() else { return };
        let (w, h) = (crate::images::pt_to_px(size.w), crate::images::pt_to_px(size.h));
        let m = SizeModel::new(w, h, dpi.0);
        self.st.borrow_mut().size_model = Some(m);
        let units: Vec<SharedString> = Unit::ALL.iter().map(|u| SharedString::from(u.name())).collect();
        self.ui.set_size_units(ModelRc::new(VecModel::from(units)));
        self.ui.set_size_unit(0);
        self.ui.set_size_proportional(true);
        self.ui.set_size_resample(true);
        self.sync_size_ui(None);
        self.ui.set_size_visible(true);
    }

    fn sync_size_ui(&self, skip: Option<&str>) {
        let Some(m) = self.st.borrow().size_model else { return };
        if skip != Some("width") {
            self.ui.set_size_width(m.width_text().into());
        }
        if skip != Some("height") {
            self.ui.set_size_height(m.height_text().into());
        }
        if skip != Some("resolution") {
            self.ui.set_size_resolution(m.resolution_text().into());
        }
        self.ui.set_size_editable(m.size_editable());
        self.ui.set_size_result(m.result_text().into());
    }

    /// A field in the Adjust Size panel changed.
    pub fn size_edited(&self, field: &str) {
        {
            let mut st = self.st.borrow_mut();
            let Some(m) = st.size_model.as_mut() else { return };
            match field {
                "width" => m.set_width(&self.ui.get_size_width()),
                "height" => m.set_height(&self.ui.get_size_height()),
                "resolution" => m.set_resolution(&self.ui.get_size_resolution()),
                "unit" => {
                    m.unit = Unit::ALL.get(self.ui.get_size_unit().max(0) as usize).copied().unwrap_or(Unit::Pixels)
                }
                "proportional" => m.set_proportional(self.ui.get_size_proportional()),
                "resample" => m.set_resample(self.ui.get_size_resample()),
                _ => {}
            }
        }
        self.sync_size_ui(Some(field));
    }

    pub fn size_confirm(&self) {
        let Some(m) = self.st.borrow_mut().size_model.take() else { return };
        self.ui.set_size_visible(false);
        self.ui.invoke_focus_canvas();
        if !m.changes_anything() {
            return;
        }
        let old_dpi = self.image_dpi();
        if let Some(meta) = self.st.borrow_mut().image.as_mut() {
            meta.info.dpi = (m.dpi, m.dpi);
        }
        if m.changes_pixels() {
            let (kx, ky) = (m.w as f32 / m.base_w as f32, m.h as f32 / m.base_h as f32);
            self.pixel_edit("Resizing…", None, move |_, f| {
                let w = ((f.width as f32 * kx).round().max(1.0)) as u32;
                let h = ((f.height as f32 * ky).round().max(1.0)) as u32;
                crate::images::decode::check_size(w, h)?;
                Ok(edit::resize(&f, w, h))
            });
            // The snapshot taken by pixel_edit records the new dpi; record the old one.
            self.st.borrow_mut().pending_dpi_undo = old_dpi;
        } else {
            // Resolution only: an annotation-level undo step that restores the dpi.
            let annots = self.annotations();
            let mut st = self.st.borrow_mut();
            st.undo.push(Snapshot { annots, pdf: None, dpi: old_dpi });
            st.redo.clear();
            drop(st);
            self.mark_dirty();
            self.sync_undo_ui();
            if self.ui.get_inspector_visible() {
                self.refresh_inspector();
            }
        }
    }

    pub fn size_cancel(&self) {
        self.st.borrow_mut().size_model = None;
        self.ui.set_size_visible(false);
        self.ui.invoke_focus_canvas();
    }

    // ----- Adjust Color -----

    pub fn show_adjust_color(&self) {
        if !self.is_image() {
            return;
        }
        self.set_adjust_ui(&ColorAdjust::default());
        self.ui.set_adjust_visible(true);
    }

    fn set_adjust_ui(&self, a: &ColorAdjust) {
        self.ui.set_adjust_black(a.black);
        self.ui.set_adjust_white(a.white);
        self.ui.set_adjust_exposure(a.exposure);
        self.ui.set_adjust_contrast(a.contrast);
        self.ui.set_adjust_saturation(a.saturation);
        self.ui.set_adjust_temperature(a.temperature);
        self.ui.set_adjust_tint(a.tint);
        self.ui.set_adjust_sepia(a.sepia);
        self.ui.set_adjust_sharpness(a.sharpness);
    }

    pub fn adjust_values(&self) -> ColorAdjust {
        let mut a = ColorAdjust {
            black: self.ui.get_adjust_black(),
            white: self.ui.get_adjust_white(),
            exposure: self.ui.get_adjust_exposure(),
            contrast: self.ui.get_adjust_contrast(),
            saturation: self.ui.get_adjust_saturation(),
            temperature: self.ui.get_adjust_temperature(),
            tint: self.ui.get_adjust_tint(),
            sepia: self.ui.get_adjust_sepia(),
            sharpness: self.ui.get_adjust_sharpness(),
        };
        if a.white <= a.black + 0.02 {
            a.white = (a.black + 0.02).min(1.0);
        }
        a
    }

    /// Live preview: the adjustment becomes part of the display filter.
    pub fn adjust_changed(&self) {
        let a = self.adjust_values();
        self.set_display_adjust((!a.is_identity()).then_some(a));
    }

    fn set_display_adjust(&self, adjust: Option<ColorAdjust>) {
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let icc = self.st.borrow().image.as_ref().and_then(|m| m.icc.clone());
        let filter = DisplayFilter { adjust, icc };
        let filter = (!filter.is_empty()).then(|| Arc::new(filter));
        self.engine.submit(Priority::Interactive, None, move |s| {
            if let Some(d) = s.doc_mut(doc_id) {
                d.filter = filter;
            }
        });
        self.invalidate_renders();
    }

    pub fn adjust_auto_levels(&self) {
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let levels = self.engine.call(move |s| {
            let d = s.doc(doc_id)?;
            let frames: Vec<Frame> = (0..d.page_count()).filter_map(|p| imagedoc::rasterise(d, p, false)).collect();
            Some(edit::auto_levels(&frames.iter().collect::<Vec<_>>()))
        });
        if let Some((black, white)) = levels {
            self.ui.set_adjust_black(black);
            self.ui.set_adjust_white(white);
            self.adjust_changed();
        }
    }

    pub fn adjust_reset(&self) {
        self.set_adjust_ui(&ColorAdjust::default());
        self.adjust_changed();
    }

    /// Done: the previewed adjustment is applied to the pixels (one undo step).
    pub fn adjust_done(&self) {
        if !self.ui.get_adjust_visible() {
            return;
        }
        self.ui.set_adjust_visible(false);
        self.ui.invoke_focus_canvas();
        let a = self.adjust_values();
        self.set_display_adjust(None);
        if a.is_identity() {
            return;
        }
        self.pixel_edit("Adjusting colour…", None, move |_, mut f| {
            a.apply(&mut f.rgba, f.width, f.height);
            Ok(f)
        });
    }

    // ----- Instant Alpha -----

    pub(crate) fn alpha_pointer_down(&self, p: Point) -> bool {
        if !matches!(self.st.borrow().tool, Tool::InstantAlpha) {
            return false;
        }
        let Some((page, pp)) = self.st.borrow().layout.page_at(p) else { return true };
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return true };
        let Some(frame) = self.engine.call(move |s| s.doc(doc_id).and_then(|d| imagedoc::rasterise(d, page, false)))
        else {
            return true;
        };
        let seed = (
            ((pp.x / PT_PER_PX) as u32).min(frame.width.saturating_sub(1)),
            ((pp.y / PT_PER_PX) as u32).min(frame.height.saturating_sub(1)),
        );
        let frame = Arc::new(frame);
        let mask = Arc::new(edit::similar_region(&frame, seed, ALPHA_BASE_TOLERANCE));
        self.st.borrow_mut().alpha =
            Some(AlphaDrag { page, seed, start: p, frame, tolerance: ALPHA_BASE_TOLERANCE, mask });
        self.show_alpha_mask();
        true
    }

    pub(crate) fn alpha_pointer_move(&self, p: Point) -> bool {
        let changed = {
            let mut st = self.st.borrow_mut();
            let Some(a) = st.alpha.as_mut() else { return false };
            let t = (ALPHA_BASE_TOLERANCE + a.start.distance(p) * ALPHA_TOLERANCE_PER_PX).min(0.9);
            if (t - a.tolerance).abs() < 0.005 {
                false
            } else {
                a.tolerance = t;
                a.mask = Arc::new(edit::similar_region(&a.frame, a.seed, t));
                true
            }
        };
        if changed {
            self.show_alpha_mask();
        }
        true
    }

    pub(crate) fn alpha_pointer_up(&self) -> bool {
        let Some(a) = self.st.borrow_mut().alpha.take() else { return false };
        self.ui.set_mask_visible(false);
        let mask = a.mask.clone();
        let (w, h) = (a.frame.width, a.frame.height);
        self.pixel_edit("Removing background…", Some(vec![a.page]), move |_, mut f| {
            if (f.width, f.height) != (w, h) {
                return Err("The image changed during the selection.".into());
            }
            edit::clear_masked(&mut f, &mask);
            Ok(f)
        });
        true
    }

    pub(crate) fn alpha_cancel(&self) -> bool {
        let had = self.st.borrow_mut().alpha.take().is_some();
        self.ui.set_mask_visible(false);
        had
    }

    /// Shows the selection as a translucent red overlay, at most 1024 px wide.
    fn show_alpha_mask(&self) {
        let st = self.st.borrow();
        let Some(a) = st.alpha.as_ref() else { return };
        let (w, h) = (a.frame.width, a.frame.height);
        let k = (1024.0 / w.max(h) as f32).min(1.0);
        let (pw, ph) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(pw, ph);
        let px = buf.make_mut_slice();
        for y in 0..ph {
            let sy = ((y as f32 / k) as u32).min(h - 1);
            for x in 0..pw {
                let sx = ((x as f32 / k) as u32).min(w - 1);
                if a.mask[(sy * w + sx) as usize] {
                    px[(y * pw + x) as usize] = Rgba8Pixel { r: 255, g: 40, b: 40, a: 130 };
                }
            }
        }
        let Some(r) = st.layout.rects.get(a.page).copied() else { return };
        drop(st);
        self.ui.set_mask_image(Image::from_rgba8(buf));
        self.ui.set_mask_x(r.x);
        self.ui.set_mask_y(r.y);
        self.ui.set_mask_w(r.w);
        self.ui.set_mask_h(r.h);
        self.ui.set_mask_visible(true);
    }

    /// Fraction of the page Instant Alpha currently selects (for tests).
    pub fn alpha_selection_fraction(&self) -> Option<f32> {
        let st = self.st.borrow();
        let a = st.alpha.as_ref()?;
        Some(a.mask.iter().filter(|m| **m).count() as f32 / a.mask.len().max(1) as f32)
    }

    // ----- Location -----

    pub fn show_location(&self) {
        if let Some(m) = self.st.borrow_mut().image.as_mut() {
            m.show_location = !m.show_location;
        }
        self.ui.set_inspector_visible(true);
        self.refresh_inspector();
    }

    pub fn remove_location(&self) {
        let has = self.st.borrow().image.as_ref().is_some_and(|m| m.has_location() && !m.strip_location);
        if !has {
            return;
        }
        if let Some(m) = self.st.borrow_mut().image.as_mut() {
            m.strip_location = true;
        }
        self.mark_dirty();
        self.sync_location_ui();
        if self.ui.get_inspector_visible() {
            self.refresh_inspector();
        }
    }

    pub(crate) fn sync_location_ui(&self) {
        let (has, shown, removed) = self
            .st
            .borrow()
            .image
            .as_ref()
            .map(|m| (m.has_location(), m.show_location, m.strip_location))
            .unwrap_or_default();
        self.ui.set_has_location(has);
        self.ui.set_location_shown(shown);
        self.ui.set_location_removed(removed);
    }

    /// Inspector rows for an image.
    pub(crate) fn image_info_rows(&self) -> Vec<(String, String, bool)> {
        let st = self.st.borrow();
        let Some(m) = st.image.as_ref() else { return Vec::new() };
        let mut rows: Vec<(String, String, bool)> = Vec::new();
        let mut row = |k: &str, v: String| rows.push((k.into(), v, false));
        let doc = st.doc.as_ref();
        row("Kind", format!("{} image", m.format().name()));
        if let Some(s) = doc.and_then(|d| d.sizes.get(st.current_page)) {
            row("Dimensions", format!("{} × {} pixels", crate::images::pt_to_px(s.w), crate::images::pt_to_px(s.h)));
        }
        let (dx, dy) = m.info.dpi;
        row(
            "Resolution",
            if (dx - dy).abs() < 0.5 {
                format!("{dx:.0} pixels/inch")
            } else {
                format!("{dx:.0} × {dy:.0} pixels/inch")
            },
        );
        row("Colour model", m.info.color_model.clone());
        row("Depth", format!("{} bits per channel", m.info.bits_per_channel));
        row("Transparency", if m.info.has_alpha { "Yes" } else { "No" }.into());
        row(
            "Colour profile",
            m.info.icc.as_deref().map(crate::images::profile_name).unwrap_or_else(|| "None (sRGB assumed)".into()),
        );
        let n = doc.map(|d| d.sizes.len()).unwrap_or(1);
        if n > 1 {
            row("Images", n.to_string());
        }
        let exif = m.info.exif.as_deref().map(metadata::summary).unwrap_or_default();
        if !exif.is_empty() {
            rows.push(("Camera".into(), String::new(), true));
            for (k, v) in exif {
                rows.push((k, v, false));
            }
        }
        if m.has_location() {
            rows.push(("Location".into(), String::new(), true));
            let v = if m.strip_location {
                "Will be removed when you save".to_string()
            } else if m.show_location {
                m.info
                    .exif
                    .as_deref()
                    .and_then(metadata::location)
                    .map(metadata::format_location)
                    .unwrap_or_else(|| "Present, but unreadable".into())
            } else {
                "Hidden. Choose Show Location to see it.".to_string()
            };
            rows.push(("GPS".into(), v, false));
        }
        rows
    }

    /// Remaps frame delays after a page operation.
    pub(crate) fn remap_image_frames(&self, map: &[Option<usize>], new_len: usize) {
        let mut st = self.st.borrow_mut();
        let Some(m) = st.image.as_mut() else { return };
        let mut delays = vec![100; new_len];
        for (old, new) in map.iter().enumerate() {
            if let (Some(n), Some(d)) = (new, m.delays.get(old))
                && *n < new_len
            {
                delays[*n] = *d;
            }
        }
        m.delays = delays;
    }
}

pub(crate) fn image_format_for(f: SaveFormat) -> Option<ImageFormat> {
    Some(match f {
        SaveFormat::Png => ImageFormat::Png,
        SaveFormat::Jpeg => ImageFormat::Jpeg,
        SaveFormat::Heic => ImageFormat::Heic,
        SaveFormat::Tiff => ImageFormat::Tiff,
        SaveFormat::Gif => ImageFormat::Gif,
        SaveFormat::WebP => ImageFormat::WebP,
        SaveFormat::Bmp => ImageFormat::Bmp,
        SaveFormat::Exr => ImageFormat::OpenExr,
        SaveFormat::Ico => ImageFormat::Ico,
    })
}
