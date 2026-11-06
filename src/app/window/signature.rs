//! Signatures: the Signatures panel (draw, import, choose, delete), protected storage
//! and placing a signature on a page or into a signature field.

use slint::{ComponentHandle, ModelRc, VecModel};

use super::{DocWindow, Tool};
use crate::core::annot::{self, Annotation, Color, Seg, Shape, Style};
use crate::core::geometry::{Affine, Point, Rect};
use crate::storage::signatures::{self, Signature};
use crate::ui::SignatureView;

/// Default placed signature height, in points.
const PLACED_HEIGHT: f32 = 40.0;
/// Imported images are traced at this height (pixels).
const TRACE_HEIGHT: u32 = 120;

#[derive(Default)]
pub(crate) struct SignatureState {
    pub loaded: bool,
    pub list: Vec<Signature>,
    pub chosen: Option<u64>,
    pub pad: Vec<Vec<Point>>,
    /// A signature field waiting for a signature.
    pub pending_field: Option<(usize, Rect)>,
}

fn svg_for(strokes: &[Vec<Point>], scale: f32) -> String {
    let segs: Vec<Seg> = strokes
        .iter()
        .flat_map(|s| {
            s.iter().enumerate().map(|(i, p)| {
                let p = Point::new(p.x * scale, p.y * scale);
                if i == 0 { Seg::Move(p) } else { Seg::Line(p) }
            })
        })
        .collect();
    annot::svg_path(&segs, &Affine::IDENTITY)
}

impl DocWindow {
    fn ensure_signatures(&self) {
        let mut st = self.sig.borrow_mut();
        if !st.loaded {
            st.list = signatures::load_from(&signatures::path());
            st.loaded = true;
        }
    }

    fn refresh_signature_list(&self) {
        self.ensure_signatures();
        let st = self.sig.borrow();
        let views: Vec<SignatureView> = st
            .list
            .iter()
            .map(|s| SignatureView {
                id: s.id as i32,
                name: s.name.as_str().into(),
                commands: svg_for(&s.strokes, 100.0).into(),
                aspect: s.aspect,
                chosen: st.chosen == Some(s.id),
            })
            .collect();
        drop(st);
        self.ui.set_signatures(ModelRc::new(VecModel::from(views)));
    }

    pub fn open_signatures(&self) {
        self.refresh_signature_list();
        self.ui.set_sig_drawing(false);
        self.ui.set_signatures_visible(true);
    }

    pub fn signatures_closed(&self) {
        if self.ui.get_sig_drawing() {
            self.ui.set_sig_drawing(false);
            self.sig.borrow_mut().pad.clear();
            return;
        }
        self.ui.set_signatures_visible(false);
        self.sig.borrow_mut().pending_field = None;
        self.ui.invoke_focus_canvas();
    }

    /// Chooses a saved signature for placing; signs a waiting field immediately.
    pub fn signature_chosen(&self, id: u64) {
        self.sig.borrow_mut().chosen = Some(id);
        self.ui.set_signatures_visible(false);
        let pending = self.sig.borrow_mut().pending_field.take();
        match pending {
            Some((page, rect)) => self.sign_into(page, rect),
            None => self.set_tool(Tool::Signature),
        }
    }

    pub fn signature_delete(&self, id: u64) {
        self.ensure_signatures();
        let list = {
            let mut st = self.sig.borrow_mut();
            st.list.retain(|s| s.id != id);
            if st.chosen == Some(id) {
                st.chosen = None;
            }
            st.list.clone()
        };
        if let Err(e) = signatures::save_to(&signatures::path(), &list) {
            self.error("The signature couldn’t be deleted.", &e);
        }
        self.refresh_signature_list();
    }

    pub fn signature_start_drawing(&self) {
        self.sig.borrow_mut().pad.clear();
        self.ui.set_pad_commands(Default::default());
        let n = self.sig.borrow().list.len() + 1;
        self.ui.set_sig_name(format!("Signature {n}").into());
        self.ui.set_sig_drawing(true);
    }

    pub fn pad_pointer(&self, kind: i32, x: f32, y: f32) {
        {
            let mut st = self.sig.borrow_mut();
            match kind {
                0 => st.pad.push(vec![Point::new(x, y)]),
                1 => {
                    if let Some(s) = st.pad.last_mut()
                        && s.last().is_none_or(|q| q.distance(Point::new(x, y)) >= 0.8) {
                            s.push(Point::new(x, y));
                        }
                }
                _ => {}
            }
        }
        // Only strokes that are being drawn (mouse down) update the preview.
        if kind == 0 || kind == 1 {
            let cmd = svg_for(&self.sig.borrow().pad, 1.0);
            self.ui.set_pad_commands(cmd.into());
        }
    }

    pub fn pad_clear(&self) {
        self.sig.borrow_mut().pad.clear();
        self.ui.set_pad_commands(Default::default());
    }

    /// Saves the drawn signature (encrypted) and chooses it.
    pub fn pad_save(&self) {
        let strokes: Vec<Vec<Point>> =
            self.sig.borrow().pad.iter().filter(|s| !s.is_empty()).map(|s| annot::smooth_stroke(s)).collect();
        let name = self.ui.get_sig_name().to_string();
        if let Some(id) = self.store_signature(&strokes, 0.018, name) {
            self.ui.set_sig_drawing(false);
            self.sig.borrow_mut().pad.clear();
            self.sig.borrow_mut().chosen = Some(id);
            self.refresh_signature_list();
        }
    }

    fn store_signature(&self, strokes: &[Vec<Point>], width: f32, name: String) -> Option<u64> {
        let (norm, aspect) = signatures::normalise(strokes)?;
        self.ensure_signatures();
        // Small sequential ids: they round-trip through the UI as 32-bit ints.
        let id = self.sig.borrow().list.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        let name = if name.trim().is_empty() { "Signature".to_string() } else { name.trim().to_string() };
        let list = {
            let mut st = self.sig.borrow_mut();
            st.list.push(Signature { id, name, strokes: norm, aspect, width });
            st.list.clone()
        };
        match signatures::save_to(&signatures::path(), &list) {
            Ok(()) => Some(id),
            Err(e) => {
                self.error("The signature couldn’t be saved.", &e);
                None
            }
        }
    }

    /// Imports a photo or scan of a signature: dark ink is traced into strokes.
    pub fn signature_import(&self) {
        if !super::super::context().winit {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import Signature Image")
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .set_parent(&self.ui.window().window_handle())
            .pick_file()
        else {
            return;
        };
        match trace_signature_image(&path) {
            Ok(strokes) => {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(id) = self.store_signature(&strokes, 1.15 / TRACE_HEIGHT as f32, name) {
                    self.sig.borrow_mut().chosen = Some(id);
                    self.refresh_signature_list();
                }
            }
            Err(e) => self.error("The image couldn’t be used as a signature.", &e),
        }
    }

    /// Places the chosen signature centred on `at`, or opens the panel to choose one.
    pub(crate) fn place_signature(&self, page: usize, at: Point) {
        self.ensure_signatures();
        let chosen =
            self.sig.borrow().chosen.and_then(|id| self.sig.borrow().list.iter().find(|s| s.id == id).cloned());
        let Some(sig) = chosen else {
            self.open_signatures();
            return;
        };
        let h = PLACED_HEIGHT;
        let w = h * sig.aspect;
        self.add_signature_annotation(page, Rect::new(at.x - w / 2.0, at.y - h / 2.0, w, h), &sig);
    }

    /// Signs into a field rectangle, fitting the signature inside it.
    pub(crate) fn sign_into(&self, page: usize, field: Rect) {
        self.ensure_signatures();
        let chosen =
            self.sig.borrow().chosen.and_then(|id| self.sig.borrow().list.iter().find(|s| s.id == id).cloned());
        let Some(sig) = chosen else {
            self.sig.borrow_mut().pending_field = Some((page, field));
            self.open_signatures();
            return;
        };
        let inner = field.inflate(-2.0);
        let h = (inner.w / sig.aspect).min(inner.h);
        let w = h * sig.aspect;
        let r = Rect::new(inner.x + (inner.w - w) / 2.0, inner.y + (inner.h - h) / 2.0, w, h);
        self.add_signature_annotation(page, r, &sig);
    }

    fn add_signature_annotation(&self, page: usize, r: Rect, sig: &Signature) {
        let strokes: Vec<Vec<Point>> = sig
            .strokes
            .iter()
            .map(|s| s.iter().map(|p| Point::new(r.x + p.x * r.h, r.y + p.y * r.h)).collect())
            .collect();
        let style = Style { stroke: Color::rgb(20, 20, 60), width: (sig.width * r.h).max(0.5), ..Style::default() };
        self.add_annotation(Annotation::new(0, page, r, Shape::Signature { strokes }, style));
        self.set_tool(Tool::Text);
    }

    /// For tests: stores a signature from raw strokes and chooses it.
    pub fn add_signature_for_test(&self, strokes: &[Vec<Point>], name: &str) -> Option<u64> {
        let id = self.store_signature(strokes, 0.018, name.into())?;
        self.sig.borrow_mut().chosen = Some(id);
        Some(id)
    }
}

/// Thresholds a signature image and turns each run of dark pixels into a short
/// horizontal stroke (one per pixel row), at `TRACE_HEIGHT` rows.
pub fn trace_signature_image(path: &std::path::Path) -> Result<Vec<Vec<Point>>, String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_luma8();
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return Err("The image is too small.".into());
    }
    let tw = ((w as f32 / h as f32) * TRACE_HEIGHT as f32).round().max(1.0) as u32;
    let small = image::imageops::resize(&img, tw, TRACE_HEIGHT, image::imageops::FilterType::Triangle);
    // Ink is anything well below the image's average brightness.
    let mean = small.pixels().map(|p| p.0[0] as u32).sum::<u32>() / (tw * TRACE_HEIGHT).max(1);
    let threshold = (mean as f32 * 0.6) as u8;
    let mut strokes = Vec::new();
    for y in 0..TRACE_HEIGHT {
        let mut run: Option<u32> = None;
        for x in 0..=tw {
            let ink = x < tw && small.get_pixel(x, y).0[0] < threshold;
            match (ink, run) {
                (true, None) => run = Some(x),
                (false, Some(start)) => {
                    strokes.push(vec![Point::new(start as f32, y as f32), Point::new((x - 1) as f32 + 0.6, y as f32)]);
                    run = None;
                }
                _ => {}
            }
        }
    }
    if strokes.is_empty() {
        return Err("No signature was found in the image. Use dark ink on a light background.".into());
    }
    Ok(strokes)
}
