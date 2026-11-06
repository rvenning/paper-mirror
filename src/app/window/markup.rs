//! Markup: creating, selecting, moving, resizing, styling and editing annotations,
//! applying text markup to selections, undo/redo, and the overlay model that draws
//! annotations on the canvas.

use slint::{Color as SColor, ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, SharedString};

use super::{DocWindow, Tool};
use crate::core::annot::{self, Align, Annotation, Color, Prim, Shape, Style};
use crate::core::geometry::{Affine, Point, Rect};
use crate::core::textpath::{FAMILIES, FontStack};
use crate::core::textsel::TextSelection;
use crate::pdf::document::RenderTarget;
use crate::pdf::engine::Priority;
use crate::ui::AnnotPrim;

/// Pick tolerance around annotations, in logical pixels.
const PICK_TOLERANCE_PX: f32 = 4.0;
/// Handle hit radius, in logical pixels.
const HANDLE_PX: f32 = 6.0;
/// Undo history depth.
const UNDO_DEPTH: usize = 100;
const LENS_MAGNIFICATION: f32 = 2.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Line,
    Arrow,
    Rect,
    Rounded,
    Oval,
    Bubble,
    Star,
    Lens,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkupKind {
    #[default]
    Highlight,
    Underline,
    StrikeOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DragMode {
    Move,
    /// Handle index: 0 top-left, then clockwise.
    Resize(u8),
}

#[derive(Clone, Debug)]
pub(crate) struct AnnotDrag {
    pub id: u64,
    pub mode: DragMode,
    pub start: Point,
    pub original: Annotation,
    pub moved: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Creating {
    pub page: usize,
    pub start: Point,
    pub current: Point,
    pub points: Vec<Point>,
}

pub fn tool_id(t: Tool) -> &'static str {
    match t {
        Tool::Text => "tool-text",
        Tool::Rect => "tool-rect",
        Tool::Sketch => "tool-sketch",
        Tool::Draw => "tool-draw",
        Tool::Shape(_) => "tool-shape",
        Tool::TextBox => "tool-textbox",
        Tool::Highlight(_) => "tool-highlight",
        Tool::Signature => "tool-signature",
        Tool::Note => "tool-note",
        Tool::Redact => "tool-redact",
    }
}

fn scolor(c: Color, opacity: f32) -> SColor {
    SColor::from_argb_u8((c.a as f32 * opacity.clamp(0.0, 1.0)).round() as u8, c.r, c.g, c.b)
}

pub fn from_scolor(c: SColor) -> Color {
    Color { r: c.red(), g: c.green(), b: c.blue(), a: c.alpha() }
}

/// Handle positions (fractions of the box) in the order used by the UI.
const HANDLES: [(f32, f32); 8] =
    [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (1.0, 0.5), (1.0, 1.0), (0.5, 1.0), (0.0, 1.0), (0.0, 0.5)];

fn resized(r: Rect, handle: u8, d: Point) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.right(), r.bottom());
    let (hx, hy) = HANDLES[handle as usize];
    if hx == 0.0 {
        x0 += d.x;
    } else if hx == 1.0 {
        x1 += d.x;
    }
    if hy == 0.0 {
        y0 += d.y;
    } else if hy == 1.0 {
        y1 += d.y;
    }
    Rect::from_points(Point::new(x0, y0), Point::new(x1, y1))
}

impl DocWindow {
    // ----- Tools and style ----------------------------------------------------------

    pub(crate) fn finish_pending_edit(&self) {
        if self.st.borrow().editing.is_some() {
            let text = self.ui.get_edit_text().to_string();
            self.finish_edit(&text);
        }
    }

    pub(crate) fn select_shape_tool(&self, kind: ShapeKind) {
        self.set_tool(Tool::Shape(kind));
    }

    pub fn annotations(&self) -> Vec<Annotation> {
        self.st.borrow().annots.clone()
    }

    pub fn selected_annotation(&self) -> Option<u64> {
        self.st.borrow().selected_annot
    }

    pub fn is_dirty(&self) -> bool {
        self.st.borrow().dirty
    }

    pub(crate) fn current_style(&self) -> Style {
        self.st.borrow().style.clone()
    }

    pub(crate) fn mark_dirty(&self) {
        self.st.borrow_mut().dirty = true;
        self.ui.set_edited(true);
    }

    /// Applies a style change to the selected annotation (undoable) and to new ones.
    fn restyle(&self, f: impl Fn(&mut Style)) {
        f(&mut self.st.borrow_mut().style);
        let sel = self.st.borrow().selected_annot;
        if let Some(id) = sel {
            self.push_undo();
            if let Some(a) = self.st.borrow_mut().annots.iter_mut().find(|a| a.id == id) {
                f(&mut a.style);
            }
            self.mark_dirty();
        }
        let s = self.st.borrow().style.clone();
        super::super::update_settings(|set| {
            set.markup_color = s.stroke.hex();
            set.markup_line_width = s.width;
            set.markup_font_size = s.font_size;
        });
        self.sync_style_ui();
        self.refresh_annotations();
    }

    pub fn set_stroke_color(&self, c: Color) {
        self.restyle(|s| {
            s.stroke = c;
        });
    }

    pub fn set_fill_color(&self, c: Option<Color>) {
        self.restyle(|s| s.fill = c);
    }

    pub fn set_line_style(&self, width: f32, dashed: bool) {
        self.restyle(|s| {
            s.width = width;
            s.dashed = dashed;
        });
    }

    pub fn set_text_style(&self, font: &str, size: f32, color: Color, align: i32) {
        let font = font.to_string();
        self.restyle(|s| {
            s.font = font.clone();
            s.font_size = size.clamp(4.0, 144.0);
            s.text_color = color;
            s.align = match align {
                1 => Align::Center,
                2 => Align::Right,
                _ => Align::Left,
            };
        });
    }

    pub(crate) fn sync_style_ui(&self) {
        let st = self.st.borrow();
        // Show the selected annotation's style, otherwise the defaults.
        let s = st
            .selected_annot
            .and_then(|id| st.annots.iter().find(|a| a.id == id))
            .map(|a| &a.style)
            .unwrap_or(&st.style);
        self.ui.set_markup_stroke(scolor(s.stroke, 1.0));
        self.ui.set_markup_fill(scolor(s.fill.unwrap_or(Color::WHITE), 1.0));
        self.ui.set_markup_fill_none(s.fill.is_none());
        self.ui.set_markup_line_width(s.width);
        self.ui.set_markup_dashed(s.dashed);
        self.ui.set_markup_font(s.font.as_str().into());
        self.ui.set_markup_font_size(s.font_size);
        self.ui.set_markup_text_color(scolor(s.text_color, 1.0));
        self.ui.set_markup_align(match s.align {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        });
    }

    pub(crate) fn init_markup_ui(&self) {
        let fonts: Vec<SharedString> = FAMILIES.iter().map(|(f, _)| SharedString::from(*f)).collect();
        self.ui.set_markup_fonts(slint::ModelRc::new(slint::VecModel::from(fonts)));
        let s = super::super::context().settings.borrow().clone();
        {
            let mut st = self.st.borrow_mut();
            st.style.stroke = Color::parse(&s.markup_color).unwrap_or(Color::RED);
            st.style.width = s.markup_line_width;
            st.style.font_size = s.markup_font_size;
        }
        self.sync_style_ui();
    }

    // ----- Undo ---------------------------------------------------------------------

    pub(crate) fn push_undo(&self) {
        let mut st = self.st.borrow_mut();
        let snap = super::Snapshot { annots: st.annots.clone(), pdf: None };
        st.undo.push(snap);
        if st.undo.len() > UNDO_DEPTH {
            st.undo.remove(0);
        }
        st.redo.clear();
        drop(st);
        self.sync_undo_ui();
    }

    pub(crate) fn sync_undo_ui(&self) {
        let st = self.st.borrow();
        self.ui.set_can_undo(!st.undo.is_empty());
        self.ui.set_can_redo(!st.redo.is_empty());
    }

    pub fn undo(&self) {
        if self.forward_to_field("undo") {
            return;
        }
        self.restore_snapshot(true);
    }

    pub fn redo(&self) {
        if self.forward_to_field("redo") {
            return;
        }
        self.restore_snapshot(false);
    }

    fn restore_snapshot(&self, undo: bool) {
        self.finish_pending_edit();
        let snap = {
            let mut st = self.st.borrow_mut();
            let from = if undo { st.undo.pop() } else { st.redo.pop() };
            let Some(snap) = from else { return };
            snap
        };
        if snap.pdf.is_some() {
            // Page operations swap whole documents on the engine thread.
            self.restore_pdf_snapshot(snap, undo);
            return;
        }
        {
            let mut st = self.st.borrow_mut();
            let current = super::Snapshot { annots: st.annots.clone(), pdf: None };
            if undo {
                st.redo.push(current);
            } else {
                st.undo.push(current);
            }
        }
        {
            let mut st = self.st.borrow_mut();
            st.annots = snap.annots;
            if st.selected_annot.is_some_and(|id| !st.annots.iter().any(|a| a.id == id)) {
                st.selected_annot = None;
            }
        }
        self.mark_dirty();
        self.sync_undo_ui();
        self.refresh_annotations();
    }

    // ----- Adding, removing ------------------------------------------------------------

    pub fn add_annotation(&self, mut a: Annotation) -> u64 {
        self.push_undo();
        let id = {
            let mut st = self.st.borrow_mut();
            st.next_annot_id += 1;
            a.id = st.next_annot_id;
            let id = a.id;
            st.annots.push(a);
            st.selected_annot = Some(id);
            id
        };
        self.mark_dirty();
        self.sync_style_ui();
        self.refresh_annotations();
        self.refresh_notes_list();
        id
    }

    pub fn delete_selected_annotation(&self) -> bool {
        let Some(id) = self.st.borrow().selected_annot else { return false };
        self.push_undo();
        {
            let mut st = self.st.borrow_mut();
            st.annots.retain(|a| a.id != id);
            st.selected_annot = None;
            st.editing = None;
        }
        self.ui.set_editing(false);
        self.mark_dirty();
        self.refresh_annotations();
        self.refresh_notes_list();
        true
    }

    pub fn select_annotation(&self, id: Option<u64>) {
        self.st.borrow_mut().selected_annot = id;
        self.ui.set_annot_selected(id.is_some());
        self.sync_style_ui();
        self.refresh_annotations();
    }

    /// Topmost annotation at a page point.
    fn annotation_at(&self, page: usize, pp: Point) -> Option<u64> {
        let st = self.st.borrow();
        let tol = PICK_TOLERANCE_PX / st.layout.scale.max(0.01);
        st.annots.iter().rev().find(|a| a.page == page && a.hit(pp, tol)).map(|a| a.id)
    }

    fn handle_at(&self, p: Point) -> Option<u8> {
        let st = self.st.borrow();
        let id = st.selected_annot?;
        let a = st.annots.iter().find(|a| a.id == id)?;
        if a.shape.is_markup() || matches!(a.shape, Shape::Note { .. }) {
            return None;
        }
        let v = st.layout.rect_to_view(a.page, a.bounds);
        HANDLES
            .iter()
            .position(|(fx, fy)| Point::new(v.x + fx * v.w, v.y + fy * v.h).distance(p) <= HANDLE_PX)
            .map(|i| i as u8)
    }

    // ----- Pointer --------------------------------------------------------------------

    /// Returns true when the markup layer consumed the press.
    pub(crate) fn markup_pointer_down(&self, p: Point) -> bool {
        self.finish_pending_edit();
        let (tool, page, pp) = {
            let st = self.st.borrow();
            let Some((page, pp)) = st.layout.page_at(p) else { return false };
            (st.tool, page, pp)
        };
        // Selection handles first, then existing annotations, then the tool.
        if let Some(h) = self.handle_at(p) {
            let id = self.st.borrow().selected_annot.unwrap();
            let original = self.st.borrow().annots.iter().find(|a| a.id == id).cloned().unwrap();
            self.st.borrow_mut().annot_drag =
                Some(AnnotDrag { id, mode: DragMode::Resize(h), start: pp, original, moved: false });
            return true;
        }
        let creates = !matches!(tool, Tool::Text | Tool::Rect | Tool::Highlight(_));
        if let Some(id) = self.annotation_at(page, pp).filter(|_| !matches!(tool, Tool::Draw | Tool::Sketch)) {
            self.select_annotation(Some(id));
            let original = self.st.borrow().annots.iter().find(|a| a.id == id).cloned().unwrap();
            self.st.borrow_mut().annot_drag =
                Some(AnnotDrag { id, mode: DragMode::Move, start: pp, original, moved: false });
            return true;
        }
        if self.st.borrow().selected_annot.is_some() {
            self.select_annotation(None);
        }
        if !creates {
            return false;
        }
        if tool == Tool::Note {
            let style = self.current_style();
            let id = self.add_annotation(Annotation::new(
                0,
                page,
                Rect::new(pp.x - 10.0, pp.y - 10.0, 20.0, 20.0),
                Shape::Note { text: String::new() },
                style,
            ));
            self.begin_edit(id);
            return true;
        }
        if tool == Tool::Signature {
            self.place_signature(page, pp);
            return true;
        }
        self.st.borrow_mut().creating = Some(Creating { page, start: pp, current: pp, points: vec![pp] });
        true
    }

    pub(crate) fn markup_pointer_move(&self, p: Point) -> bool {
        let scale = self.st.borrow().layout.scale.max(0.01);
        let drag = self.st.borrow().annot_drag.clone();
        if let Some(mut d) = drag {
            let pp = self.st.borrow().layout.to_page(d.original.page, p);
            let delta = Point::new(pp.x - d.start.x, pp.y - d.start.y);
            if !d.moved && (delta.x.abs() + delta.y.abs()) * scale < 3.0 {
                return true;
            }
            if !d.moved {
                d.moved = true;
                self.push_undo();
            }
            let mut a = d.original.clone();
            match d.mode {
                DragMode::Move if !a.shape.is_markup() => a.translate(delta.x, delta.y),
                DragMode::Resize(h) => a.resize_to(resized(d.original.bounds, h, delta)),
                _ => {}
            }
            {
                let mut st = self.st.borrow_mut();
                if let Some(slot) = st.annots.iter_mut().find(|x| x.id == d.id) {
                    *slot = a;
                }
                st.annot_drag = Some(d);
            }
            self.refresh_annotations();
            return true;
        }
        let creating = self.st.borrow().creating.is_some();
        if creating {
            {
                let mut st = self.st.borrow_mut();
                let page = st.creating.as_ref().unwrap().page;
                let size = st.doc.as_ref().map(|d| d.sizes[page]).unwrap_or_default();
                let pp = st.layout.to_page(page, p);
                let pp = Point::new(pp.x.clamp(0.0, size.w), pp.y.clamp(0.0, size.h));
                let c = st.creating.as_mut().unwrap();
                c.current = pp;
                c.points.push(pp);
            }
            self.refresh_annotations();
            return true;
        }
        false
    }

    pub(crate) fn markup_pointer_up(&self, _p: Point) -> bool {
        let drag = self.st.borrow_mut().annot_drag.take();
        if let Some(d) = drag {
            if d.moved {
                self.mark_dirty();
            }
            return true;
        }
        let Some(c) = self.st.borrow_mut().creating.take() else { return false };
        let tool = self.st.borrow().tool;
        let style = self.current_style();
        let r = Rect::from_points(c.start, c.current);
        let small = r.w < 4.0 && r.h < 4.0;
        let a = match tool {
            Tool::Draw | Tool::Sketch => {
                let pts = annot::smooth_stroke(&c.points);
                if pts.len() < 2 {
                    None
                } else if tool == Tool::Sketch {
                    match annot::recognise_sketch(&pts) {
                        Some((shape, bounds)) => Some(Annotation::new(0, c.page, bounds, shape, style)),
                        None => {
                            Some(Annotation::new(0, c.page, Rect::default(), Shape::Ink { strokes: vec![pts] }, style))
                        }
                    }
                } else {
                    Some(Annotation::new(0, c.page, Rect::default(), Shape::Ink { strokes: vec![pts] }, style))
                }
            }
            Tool::Shape(kind) => {
                // A click without dragging drops a default-sized shape.
                let r = if small { Rect::new(c.start.x - 50.0, c.start.y - 35.0, 100.0, 70.0) } else { r };
                let shape = match kind {
                    ShapeKind::Line | ShapeKind::Arrow => {
                        let (from, to) = if small {
                            (Point::new(r.x, r.bottom()), Point::new(r.right(), r.y))
                        } else {
                            (c.start, c.current)
                        };
                        Shape::Line { from, to, arrow: kind == ShapeKind::Arrow }
                    }
                    ShapeKind::Rect => Shape::Rectangle,
                    ShapeKind::Rounded => Shape::RoundedRectangle,
                    ShapeKind::Oval => Shape::Oval,
                    ShapeKind::Star => Shape::Star,
                    ShapeKind::Bubble => {
                        Shape::SpeechBubble { tail: Point::new(r.x + r.w * 0.2, r.bottom() + r.h * 0.4) }
                    }
                    ShapeKind::Lens => {
                        let side = r.w.max(r.h);
                        let sq = Rect::new(r.x, r.y, side, side);
                        let src_side = side / LENS_MAGNIFICATION;
                        let c0 = sq.center();
                        Shape::ZoomLens {
                            source: Rect::new(c0.x - src_side / 2.0, c0.y - src_side / 2.0, src_side, src_side),
                        }
                    }
                };
                let bounds = if let Shape::ZoomLens { .. } = shape {
                    Rect::new(r.x, r.y, r.w.max(r.h), r.w.max(r.h))
                } else {
                    r
                };
                Some(Annotation::new(0, c.page, bounds, shape, style))
            }
            Tool::TextBox => {
                let r =
                    if small || r.w < 30.0 { Rect::new(c.start.x, c.start.y, 180.0, style.font_size * 2.2) } else { r };
                let id = self.add_annotation(Annotation::new(
                    0,
                    c.page,
                    r,
                    Shape::TextBox { text: "Text".into() },
                    Style { fill: None, width: 0.0, ..style },
                ));
                self.begin_edit(id);
                return true;
            }
            Tool::Redact => (!small).then(|| Annotation::new(0, c.page, r, Shape::Redaction, style)),
            _ => None,
        };
        match a {
            Some(a) => {
                self.add_annotation(a);
            }
            None => self.refresh_annotations(),
        }
        true
    }

    pub(crate) fn markup_double_click(&self, p: Point) -> bool {
        let hit = {
            let st = self.st.borrow();
            st.layout.page_at(p)
        };
        let Some((page, pp)) = hit else { return false };
        let Some(id) = self.annotation_at(page, pp) else { return false };
        let editable = self.st.borrow().annots.iter().any(|a| a.id == id && a.shape.text().is_some());
        if editable {
            self.begin_edit(id);
        }
        editable
    }

    // ----- Text editing ---------------------------------------------------------------

    pub fn begin_edit(&self, id: u64) {
        let (rect, text, note, style) = {
            let st = self.st.borrow();
            let Some(a) = st.annots.iter().find(|a| a.id == id) else { return };
            let v = st.layout.rect_to_view(a.page, a.bounds);
            let note = matches!(a.shape, Shape::Note { .. });
            let rect = if note { Rect::new(v.right() + 6.0, v.y, 240.0, 150.0) } else { v };
            (rect, a.shape.text().unwrap_or_default().to_string(), note, a.style.clone())
        };
        let scale = self.st.borrow().layout.scale;
        self.st.borrow_mut().editing = Some(id);
        self.ui.set_edit_x(rect.x);
        self.ui.set_edit_y(rect.y);
        self.ui.set_edit_w(rect.w.max(60.0));
        self.ui.set_edit_h(rect.h.max(24.0));
        self.ui.set_edit_text(text.into());
        self.ui.set_edit_note(note);
        self.ui.set_edit_font(if note { "Segoe UI".into() } else { style.font.as_str().into() });
        self.ui.set_edit_font_size(if note { 13.0 } else { style.font_size * scale });
        self.ui.set_edit_color(scolor(if note { Color::BLACK } else { style.text_color }, 1.0));
        self.ui.set_edit_align(match style.align {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        });
        self.ui.set_editing(true);
        self.refresh_annotations();
    }

    pub fn finish_edit(&self, text: &str) {
        let Some(id) = self.st.borrow_mut().editing.take() else { return };
        self.ui.set_editing(false);
        let changed = self.st.borrow().annots.iter().any(|a| a.id == id && a.shape.text() != Some(text));
        if changed {
            self.push_undo();
            let mut st = self.st.borrow_mut();
            if let Some(a) = st.annots.iter_mut().find(|a| a.id == id) {
                match &mut a.shape {
                    Shape::TextBox { text: t } | Shape::Note { text: t } => *t = text.to_string(),
                    _ => {}
                }
            }
            drop(st);
            self.mark_dirty();
        }
        // An empty new text box is removed rather than left invisible.
        let empty_box = self
            .st
            .borrow()
            .annots
            .iter()
            .any(|a| a.id == id && matches!(&a.shape, Shape::TextBox { text } if text.trim().is_empty()));
        if empty_box {
            self.st.borrow_mut().annots.retain(|a| a.id != id);
            self.st.borrow_mut().selected_annot = None;
        }
        self.refresh_annotations();
        self.refresh_notes_list();
        self.ui.invoke_focus_canvas();
    }

    // ----- Text markup and redaction from a text selection ----------------------------

    /// Highlights, underlines or strikes through the current text selection.
    pub fn apply_markup(&self, kind: MarkupKind) -> bool {
        let Some(rects) = self.selection_rects() else { return false };
        let style = Style {
            stroke: match kind {
                MarkupKind::Highlight => self.st.borrow().highlight_color,
                _ => self.st.borrow().style.stroke,
            },
            ..self.current_style()
        };
        for (page, rs) in rects {
            let shape = match kind {
                MarkupKind::Highlight => Shape::Highlight { rects: rs },
                MarkupKind::Underline => Shape::Underline { rects: rs },
                MarkupKind::StrikeOut => Shape::StrikeOut { rects: rs },
            };
            self.add_annotation(Annotation::new(0, page, Rect::default(), shape, style.clone()));
        }
        self.set_text_selection(None);
        true
    }

    /// Marks the selected text for redaction.
    pub fn redact_selection(&self) -> bool {
        let Some(rects) = self.selection_rects() else { return false };
        for (page, rs) in rects {
            for r in rs {
                self.add_annotation(Annotation::new(0, page, r, Shape::Redaction, self.current_style()));
            }
        }
        self.set_text_selection(None);
        true
    }

    /// Per-page line rectangles of the text selection (loaded pages only).
    fn selection_rects(&self) -> Option<Vec<(usize, Vec<Rect>)>> {
        let st = self.st.borrow();
        let sel: TextSelection = st.text_sel?;
        let (a, b) = sel.ordered();
        let mut out = Vec::new();
        for page in a.page..=b.page {
            let Some(t) = st.page_text.get(&page) else { continue };
            if let Some((s, e)) = sel.range_on(page, t.len()) {
                let rs = t.range_rects(s, e);
                if !rs.is_empty() {
                    out.push((page, rs));
                }
            }
        }
        (!out.is_empty()).then_some(out)
    }

    pub fn has_redactions(&self) -> bool {
        self.st.borrow().annots.iter().any(|a| matches!(a.shape, Shape::Redaction))
    }

    // ----- Overlay model -----------------------------------------------------------------

    /// Rebuilds the drawn annotation primitives for the visible pages.
    pub(crate) fn refresh_annotations(&self) {
        let pages: Vec<usize> = {
            use slint::Model;
            (0..self.pages_model.row_count())
                .filter_map(|r| self.pages_model.row_data(r).map(|p| p.index as usize))
                .collect()
        };
        let mut prims = Vec::new();
        let mut lens_requests = Vec::new();
        let (sel, editing) = {
            let st = self.st.borrow();
            let preview = self.creation_preview();
            for a in st.annots.iter().chain(preview.iter()) {
                if !pages.contains(&a.page) || a.page >= st.layout.rects.len() {
                    continue;
                }
                let r = st.layout.rects[a.page];
                let to_view = Affine::translate(r.x, r.y).then(&Affine::scale(st.layout.scale, st.layout.scale));
                let hide_text = st.editing == Some(a.id);
                for (i, p) in a.prims().iter().enumerate() {
                    if hide_text && matches!(p, Prim::Text { .. }) {
                        continue;
                    }
                    let label = if i == 0 { a.shape.name() } else { "" };
                    if let Some(ap) = to_prim(p, &to_view, st.layout.scale, label) {
                        let mut ap = ap;
                        if let Prim::Lens { rect, source } = p {
                            let w =
                                (rect.w * st.layout.scale * self.ui.window().scale_factor()).round().max(1.0) as u32;
                            match st.lens_cache.get(&(a.id, w)) {
                                Some(img) => ap.image = img.clone(),
                                None => lens_requests.push((a.id, a.page, *source, w)),
                            }
                        }
                        prims.push(ap);
                    }
                }
            }
            let sel = st
                .selected_annot
                .and_then(|id| st.annots.iter().find(|a| a.id == id))
                .filter(|a| pages.contains(&a.page))
                .map(|a| {
                    let v =
                        st.layout.rect_to_view(a.page, if a.shape.is_markup() { a.visual_bounds() } else { a.bounds });
                    (v, !a.shape.is_markup() && !matches!(a.shape, Shape::Note { .. }))
                });
            (sel, st.editing.is_some())
        };
        self.annot_model.set_vec(prims);
        match sel {
            Some((v, resizable)) if !editing => {
                self.ui.set_sel_x(v.x);
                self.ui.set_sel_y(v.y);
                self.ui.set_sel_w(v.w);
                self.ui.set_sel_h(v.h);
                self.ui.set_annot_resizable(resizable);
                self.ui.set_annot_selected(true);
            }
            _ => self.ui.set_annot_selected(false),
        }
        for (id, page, source, w) in lens_requests {
            self.request_lens(id, page, source, w);
        }
    }

    /// The annotation being drawn, shown live before it is committed.
    fn creation_preview(&self) -> Option<Annotation> {
        let st = self.st.borrow();
        let c = st.creating.as_ref()?;
        let style = st.style.clone();
        let r = Rect::from_points(c.start, c.current);
        let shape = match st.tool {
            Tool::Draw | Tool::Sketch => Shape::Ink { strokes: vec![c.points.clone()] },
            Tool::Shape(ShapeKind::Line) => Shape::Line { from: c.start, to: c.current, arrow: false },
            Tool::Shape(ShapeKind::Arrow) => Shape::Line { from: c.start, to: c.current, arrow: true },
            Tool::Shape(ShapeKind::Oval) | Tool::Shape(ShapeKind::Lens) => Shape::Oval,
            Tool::Shape(ShapeKind::Rounded) => Shape::RoundedRectangle,
            Tool::Shape(ShapeKind::Star) => Shape::Star,
            Tool::Redact => Shape::Redaction,
            _ => Shape::Rectangle,
        };
        Some(Annotation::new(u64::MAX, c.page, r, shape, style))
    }

    fn request_lens(&self, id: u64, page: usize, source: Rect, w: u32) {
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let win = self.id;
        self.engine.submit(Priority::Render, None, move |s| {
            let Some(doc) = s.doc(doc_id) else { return };
            let scale = w as f32 / source.w.max(0.01);
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, w);
            let ok = doc.render(
                page,
                scale,
                source.x * scale,
                source.y * scale,
                RenderTarget { pixels: buf.make_mut_bytes(), width: w, height: w },
                true,
            );
            if ok {
                super::view::circle_mask(buf.make_mut_bytes(), w, w);
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |win| {
                        win.st.borrow_mut().lens_cache.insert((id, w), Image::from_rgba8(buf));
                        win.refresh_annotations();
                    });
                });
            }
        });
    }
}

/// Converts one primitive to an overlay item positioned in content coordinates.
fn to_prim(p: &Prim, to_view: &Affine, scale: f32, label: &str) -> Option<AnnotPrim> {
    let place = |segs: &[annot::Seg], pad: f32| -> Option<(Rect, String)> {
        let b = annot::segs_bounds(segs)?;
        let vb = to_view.apply_rect(b).inflate(pad.max(1.0));
        let local = Affine::translate(-vb.x, -vb.y).then(to_view);
        Some((vb, annot::svg_path(segs, &local)))
    };
    match p {
        Prim::Stroke { segs, color, width, dashed, opacity, .. } => {
            let w = (width * scale).max(0.75);
            let segs = if *dashed { annot::dash(segs, width * 3.0, width * 2.0) } else { segs.clone() };
            let (vb, cmd) = place(&segs, w)?;
            Some(AnnotPrim {
                kind: 0,
                x: vb.x,
                y: vb.y,
                width: vb.w,
                height: vb.h,
                commands: cmd.into(),
                color: scolor(*color, *opacity),
                stroke_width: w,
                label: label.into(),
                ..Default::default()
            })
        }
        Prim::Fill { segs, color, opacity, multiply } => {
            // No blend modes in the overlay: show multiply highlights as translucent.
            let alpha = if *multiply { 0.42 * opacity } else { *opacity };
            let (vb, cmd) = place(segs, 1.0)?;
            Some(AnnotPrim {
                kind: 1,
                x: vb.x,
                y: vb.y,
                width: vb.w,
                height: vb.h,
                commands: cmd.into(),
                color: scolor(*color, alpha),
                label: label.into(),
                ..Default::default()
            })
        }
        Prim::Text { rect, text, color, font, size, align } => {
            let lines = FontStack::new(font)
                .map(|f| f.wrap(text, *size, rect.w.max(*size)))
                .unwrap_or_else(|| vec![text.clone()]);
            let v = to_view.apply_rect(*rect);
            Some(AnnotPrim {
                kind: 2,
                x: v.x,
                y: v.y,
                width: v.w.max(1.0),
                height: v.h.max(1.0) + size * scale,
                text: lines.join("\n").into(),
                font_family: font.as_str().into(),
                font_size: size * scale,
                color: scolor(*color, 1.0),
                align: match align {
                    Align::Left => 0,
                    Align::Center => 1,
                    Align::Right => 2,
                },
                label: label.into(),
                ..Default::default()
            })
        }
        Prim::Lens { rect, .. } => {
            let v = to_view.apply_rect(*rect);
            Some(AnnotPrim {
                kind: 3,
                x: v.x,
                y: v.y,
                width: v.w,
                height: v.h,
                label: label.into(),
                ..Default::default()
            })
        }
    }
}
