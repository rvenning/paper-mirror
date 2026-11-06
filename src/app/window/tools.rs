//! Canvas pointer input: rectangular selection, Zoom to Selection, the magnifier
//! loupe, link hover and activation, and the overlay layer drawn above pages.

use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use super::{DocWindow, Drag, Tool, ViewState};
use crate::core::geometry::{Point, Rect};
use crate::core::layout::{self, PX_PER_PT, ZoomMode};
use crate::core::textsel::TextSelection;
use crate::pdf::document::RenderTarget;
use crate::pdf::engine::{DocId, Priority};
use crate::pdf::outline::{self, LinkTarget};
use crate::ui::OverlayRect;

/// Loupe diameter in logical pixels and its magnification relative to the view.
pub(crate) const LOUPE_SIZE: f32 = 220.0;
const LOUPE_MAGNIFICATION: f32 = 2.5;
/// Pointer travel (logical px) before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 3.0;

pub(crate) const POINTER_DOWN: i32 = 0;
pub(crate) const POINTER_MOVE: i32 = 1;
pub(crate) const POINTER_UP: i32 = 2;
pub(crate) const POINTER_CANCEL: i32 = 3;
pub(crate) const POINTER_DOUBLE: i32 = 4;

impl DocWindow {
    pub(crate) fn set_tool(&self, tool: Tool) {
        self.finish_pending_edit();
        {
            let mut st = self.st.borrow_mut();
            st.tool = tool;
            st.drag = None;
            st.creating = None;
        }
        self.ui.set_tool(super::markup::tool_id(tool).into());
        // Creation tools open the markup toolbar, as in Preview.
        if !matches!(tool, Tool::Text | Tool::Rect) {
            self.ui.set_markup_visible(true);
        }
        let cursor = match tool {
            Tool::Text | Tool::Highlight(_) => "default",
            _ => "crosshair",
        };
        self.ui.set_canvas_cursor(cursor.into());
    }

    pub(crate) fn toggle_loupe(&self) {
        let on = {
            let mut st = self.st.borrow_mut();
            st.loupe_on = !st.loupe_on;
            st.loupe_on
        };
        self.ui.set_loupe_on(on);
        if !on {
            self.loupe_gen.bump();
            self.ui.set_loupe_visible(false);
        }
    }

    /// Canvas pointer events in content coordinates.
    pub fn pointer(&self, kind: i32, p: Point, shift: bool, _ctrl: bool, button: i32) {
        if !self.has_document() || button != 0 {
            return;
        }
        match kind {
            POINTER_DOWN => self.pointer_down(p, shift),
            POINTER_MOVE => self.pointer_move(p),
            POINTER_UP => self.pointer_up(p, shift),
            POINTER_CANCEL => {
                self.st.borrow_mut().drag = None;
                self.ui.set_loupe_visible(false);
            }
            POINTER_DOUBLE
                if !self.markup_double_click(p) && self.st.borrow().tool.selects_text() => {
                    self.select_word_at(p);
                }
            _ => {}
        }
    }

    fn pointer_down(&self, p: Point, shift: bool) {
        if self.markup_pointer_down(p) {
            return;
        }
        let caret = self.caret_at_content(p);
        let mut st = self.st.borrow_mut();
        let Some(page) = st.layout.page_at(p).map(|(i, _)| i).or_else(|| st.layout.nearest_page(p)) else { return };
        let pp = clamp_to_page(&st, page, st.layout.to_page(page, p));
        st.drag = Some(Drag { page, start: pp, moved: false });
        if st.tool.selects_text() {
            st.link_click = link_under(&st, page, pp).then_some((page, pp));
            // Shift-click extends an existing selection from its anchor.
            st.sel_anchor = match (shift, st.text_sel) {
                (true, Some(sel)) => Some(sel.anchor),
                _ => caret,
            };
            if shift && let (Some(anchor), Some(focus)) = (st.sel_anchor, caret) {
                drop(st);
                self.set_text_selection(Some(TextSelection { anchor, focus }));
            }
        }
    }

    fn pointer_move(&self, p: Point) {
        if self.markup_pointer_move(p) {
            self.update_hover(p);
            return;
        }
        let mut rect_changed = false;
        let mut text_anchor = None;
        {
            let mut st = self.st.borrow_mut();
            if let Some(mut drag) = st.drag {
                let pp = clamp_to_page(&st, drag.page, st.layout.to_page(drag.page, p));
                let start_view = st.layout.to_view(drag.page, drag.start);
                if !drag.moved && start_view.distance(p) >= DRAG_THRESHOLD {
                    drag.moved = true;
                    st.link_click = None;
                }
                if drag.moved {
                    match st.tool {
                        Tool::Rect => {
                            st.rect_selection = Some((drag.page, Rect::from_points(drag.start, pp)));
                            rect_changed = true;
                        }
                        t if t.selects_text() => text_anchor = st.sel_anchor,
                        _ => {}
                    }
                }
                st.drag = Some(drag);
            }
        }
        if rect_changed {
            self.ui.set_has_rect_selection(true);
            self.refresh_overlays();
        }
        if let Some(anchor) = text_anchor
            && let Some(focus) = self.caret_at_content(p)
        {
            self.set_text_selection(Some(TextSelection { anchor, focus }));
        }
        self.update_hover(p);
    }

    fn pointer_up(&self, p: Point, shift: bool) {
        if self.markup_pointer_up(p) {
            return;
        }
        let (drag, link, tool) = {
            let mut st = self.st.borrow_mut();
            (st.drag.take(), st.link_click.take(), st.tool)
        };
        let Some(drag) = drag else { return };
        if !drag.moved {
            if let Some((page, start)) = link {
                // Follow the link only if the release is still over it.
                let pp = self.st.borrow().layout.to_page(page, p);
                if start.distance(pp) < 4.0 {
                    self.activate_link(page, pp);
                }
                return;
            }
            // A plain click clears selections.
            if tool.selects_text() && !shift && self.text_selection().is_some() {
                self.set_text_selection(None);
            }
            let had = self.st.borrow_mut().rect_selection.take().is_some();
            if had {
                self.ui.set_has_rect_selection(false);
                self.refresh_overlays();
            }
        } else if let Tool::Highlight(kind) = tool {
            // In highlight mode, releasing a text drag marks it up immediately.
            self.apply_markup(kind);
        } else if self.st.borrow().rect_selection.is_some_and(|(_, r)| r.w < 2.0 || r.h < 2.0) {
            self.st.borrow_mut().rect_selection = None;
            self.ui.set_has_rect_selection(false);
            self.refresh_overlays();
        }
    }

    /// Sets the rectangular selection programmatically (tests, keyboard workflows).
    pub fn set_rect_selection(&self, page: usize, r: Rect) {
        self.st.borrow_mut().rect_selection = Some((page, r));
        self.ui.set_has_rect_selection(true);
        self.refresh_overlays();
    }

    pub fn rect_selection(&self) -> Option<(usize, Rect)> {
        self.st.borrow().rect_selection
    }

    pub(crate) fn clear_selection(&self) -> bool {
        let had = self.st.borrow_mut().rect_selection.take().is_some();
        if had {
            self.ui.set_has_rect_selection(false);
            self.refresh_overlays();
        }
        had
    }

    /// Zooms so the rectangular selection fills the window, centred.
    pub(crate) fn zoom_to_selection(&self) {
        let Some((page, r)) = self.st.borrow().rect_selection else { return };
        let vp = self.viewport();
        let z = layout::clamp_zoom(
            ((vp.w - 2.0 * layout::MARGIN) / (r.w.max(1.0) * PX_PER_PT))
                .min((vp.h - 2.0 * layout::MARGIN) / (r.h.max(1.0) * PX_PER_PT)),
        );
        {
            let mut st = self.st.borrow_mut();
            st.current_page = page;
        }
        self.set_zoom_mode(ZoomMode::Custom(z), None);
        let v = self.st.borrow().layout.rect_to_view(page, r);
        let c = v.center();
        self.ui.invoke_scroll_canvas_to((c.x - vp.w / 2.0).max(0.0), (c.y - vp.h / 2.0).max(0.0));
        self.update_view();
    }

    fn update_hover(&self, p: Point) {
        let (cursor, loupe) = {
            let st = self.st.borrow();
            let over = st.layout.page_at(p);
            let cursor = match (st.tool, over) {
                (t, Some((page, pp))) if t.selects_text() && link_under(&st, page, pp) => "pointer",
                (t, Some((page, pp)))
                    if t.selects_text() && st.page_text.get(&page).is_some_and(|t| t.is_over_text(pp, 1.0)) =>
                {
                    "text"
                }
                (Tool::Text, _) | (Tool::Highlight(_), _) => "default",
                (_, _) => "crosshair",
            };
            (cursor, st.loupe_on.then_some(over).flatten())
        };
        if self.ui.get_canvas_cursor() != cursor {
            self.ui.set_canvas_cursor(cursor.into());
        }
        match loupe {
            Some((page, pp)) => self.request_loupe(page, pp, p),
            None => self.ui.set_loupe_visible(false),
        }
    }

    fn request_loupe(&self, page: usize, pp: Point, content: Point) {
        let scale_factor = self.ui.window().scale_factor();
        let scroll = self.scroll();
        self.ui.set_loupe_x(content.x - scroll.x);
        self.ui.set_loupe_y(content.y - scroll.y);
        self.ui.set_loupe_visible(true);
        let (doc_id, layout_scale) = {
            let st = self.st.borrow();
            let Some(doc) = st.doc.as_ref() else { return };
            (doc.id, st.layout.scale)
        };
        let dev_scale = layout_scale * LOUPE_MAGNIFICATION * scale_factor;
        let size = (LOUPE_SIZE * scale_factor).round() as u32;
        let ox = pp.x * dev_scale - size as f32 / 2.0;
        let oy = pp.y * dev_scale - size as f32 / 2.0;
        let generation = self.loupe_gen.bump();
        let win = self.id;
        self.engine.submit(Priority::Interactive, Some(generation), move |s| {
            let Some(doc) = s.doc(doc_id) else { return };
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(size, size);
            let ok = doc.render(
                page,
                dev_scale,
                ox,
                oy,
                RenderTarget { pixels: buf.make_mut_bytes(), width: size, height: size },
                true,
            );
            if ok {
                super::view::circle_mask(buf.make_mut_bytes(), size, size);
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |w| w.ui.set_loupe_image(Image::from_rgba8(buf)));
                });
            }
        });
    }

    /// Requests link rectangles and text geometry for pages that just became visible.
    pub(crate) fn ensure_links(&self, pages: &[usize]) {
        let (doc_id, wanted) = {
            let mut st = self.st.borrow_mut();
            let Some(doc) = st.doc.as_ref() else { return };
            let id = doc.id;
            let wanted: Vec<usize> =
                pages.iter().copied().filter(|p| !st.links.contains_key(p) && !st.pending_links.contains(p)).collect();
            st.pending_links.extend(wanted.iter().copied());
            (id, wanted)
        };
        let win = self.id;
        for page in wanted {
            self.engine.submit(Priority::Render, None, move |s| {
                let Some(doc) = s.doc(doc_id) else { return };
                let rects = outline::link_rects(doc, page);
                let geometry = crate::pdf::text::page_geometry(doc, page);
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |w| {
                        if w.st.borrow().doc.as_ref().is_none_or(|d| d.id != doc_id) {
                            return;
                        }
                        {
                            let mut st = w.st.borrow_mut();
                            st.pending_links.remove(&page);
                            st.links.insert(page, rects);
                        }
                        w.insert_page_text(page, geometry);
                        // A Select All waiting on this page can now be drawn.
                        if w.st.borrow().text_sel.is_some() {
                            w.refresh_overlays();
                        }
                    });
                });
            });
        }
    }

    fn activate_link(&self, page: usize, pp: Point) {
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let win = self.id;
        self.engine.submit(Priority::Interactive, None, move |s| {
            let Some(doc) = s.doc(doc_id) else { return };
            let target = outline::link_at(doc, page, pp);
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.follow_link(doc_id, target));
            });
        });
    }

    fn follow_link(&self, doc_id: DocId, target: Option<LinkTarget>) {
        if self.st.borrow().doc.as_ref().is_none_or(|d| d.id != doc_id) {
            return;
        }
        match target {
            Some(LinkTarget::Page(p)) => self.go_to_page(p),
            Some(LinkTarget::Uri(uri)) => {
                if !crate::platform_windows::shell::is_safe_url(&uri) {
                    self.error(
                        "This link can’t be opened.",
                        &format!("Only web and e-mail links are opened.\n\n{uri}"),
                    );
                    return;
                }
                if self
                    .confirm("Open link?", &format!("This document links to:\n\n{uri}\n\nOpen it in your default app?"))
                {
                    *self.last_opened_link.borrow_mut() = Some(uri.clone());
                    if super::super::context().winit {
                        crate::platform_windows::shell::open_url(&uri);
                    }
                }
            }
            None => {}
        }
    }

    /// Recomputes the overlay rectangles for the visible pages.
    pub(crate) fn refresh_overlays(&self) {
        let pages: Vec<usize> = {
            let model = &self.pages_model;
            (0..slint::Model::row_count(model.as_ref()))
                .filter_map(|r| slint::Model::row_data(model.as_ref(), r).map(|p| p.index as usize))
                .collect()
        };
        let overlays = build_overlays(&self.st.borrow(), &pages);
        self.overlays_model.set_vec(overlays);
    }

    /// Clears transient tool state (Esc on the canvas).
    pub fn escape(&self) -> bool {
        if self.st.borrow_mut().creating.take().is_some() {
            self.refresh_annotations();
            return true;
        }
        if self.st.borrow().selected_annot.is_some() {
            self.select_annotation(None);
            return true;
        }
        if !matches!(self.st.borrow().tool, Tool::Text) {
            self.set_tool(Tool::Text);
            return true;
        }
        if self.clear_selection() {
            return true;
        }
        if self.text_selection().is_some() {
            self.set_text_selection(None);
            return true;
        }
        if self.st.borrow().loupe_on {
            self.toggle_loupe();
            return true;
        }
        false
    }
}

fn clamp_to_page(st: &ViewState, page: usize, p: Point) -> Point {
    let size = st.doc.as_ref().map(|d| d.sizes[page]).unwrap_or_default();
    Point::new(p.x.clamp(0.0, size.w), p.y.clamp(0.0, size.h))
}

fn link_under(st: &ViewState, page: usize, pp: Point) -> bool {
    st.links.get(&page).is_some_and(|rs| rs.iter().any(|r| r.contains(pp)))
}

/// Search matches, the current match, the text selection and the rectangular
/// selection on `pages`.
pub(crate) fn build_overlays(st: &ViewState, pages: &[usize]) -> Vec<OverlayRect> {
    let mut out = Vec::new();
    let hits = &st.search.hits;
    for &page in pages {
        if page >= st.layout.rects.len() {
            continue;
        }
        let start = hits.partition_point(|h| h.page < page);
        for (i, h) in hits.iter().enumerate().skip(start).take_while(|(_, h)| h.page == page) {
            let kind = if st.search.current == Some(i) { 1 } else { 0 };
            for r in &h.rects {
                let v = st.layout.rect_to_view(page, r.inflate(0.5));
                out.push(OverlayRect { x: v.x, y: v.y, width: v.w, height: v.h, kind });
            }
        }
        if let Some(sel) = st.text_sel
            && let Some(text) = st.page_text.get(&page)
            && let Some((a, b)) = sel.range_on(page, text.len())
        {
            for r in text.range_rects(a, b) {
                let v = st.layout.rect_to_view(page, r);
                out.push(OverlayRect { x: v.x, y: v.y, width: v.w, height: v.h, kind: 2 });
            }
        }
        if let Some((p, r)) = st.rect_selection.filter(|(p, _)| *p == page) {
            let v = st.layout.rect_to_view(p, r);
            out.push(OverlayRect { x: v.x, y: v.y, width: v.w, height: v.h, kind: 3 });
        }
    }
    out
}
