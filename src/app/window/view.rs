//! Layout, virtualised page rendering, detail tiles, thumbnails, navigation and zoom.

use std::path::Path;
use std::time::Duration;

use slint::{ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer, SharedString};

use super::{DocWindow, PageImage, Tile};
use crate::core::geometry::{Point, Rect, Size};
use crate::core::layout::{self, DisplayMode, ZoomMode};
use crate::pdf::document::RenderTarget;
use crate::pdf::engine::{DocId, Generation, Priority};
use crate::ui::{PageItem, ThumbItem};

/// Upper bound for one full-page bitmap. Beyond this the page is rendered at a capped
/// resolution and a detail tile covers the visible region at full resolution.
pub(crate) const MAX_PAGE_PIXELS: u64 = 12_000_000;
/// Extra band rendered above and below the viewport, in logical pixels.
const OVERSCAN: f32 = 300.0;
const THUMB_LABEL_H: f32 = 30.0;
const THUMB_PAD: f32 = 12.0;

fn sf(w: &DocWindow) -> f32 {
    w.ui.window().scale_factor()
}

impl DocWindow {
    pub(crate) fn viewport(&self) -> Size {
        Size::new(self.ui.get_view_width(), self.ui.get_view_height())
    }

    pub(crate) fn scroll(&self) -> Point {
        Point::new(self.ui.get_scroll_x(), self.ui.get_scroll_y())
    }

    /// Recomputes the layout. `anchor` (viewport-relative) keeps the point under the
    /// cursor fixed when zooming; otherwise the top-centre of the view stays put.
    pub(crate) fn relayout(&self, anchor: Option<Point>) {
        let vp = self.viewport();
        let scroll = self.scroll();
        let new_scroll;
        {
            let mut st = self.st.borrow_mut();
            let Some(doc) = st.doc.as_ref() else { return };
            let sizes = doc.sizes.clone();
            let current = st.current_page;
            let zoom = match st.zoom_mode {
                ZoomMode::Custom(z) => layout::clamp_zoom(z),
                fit => layout::fit_zoom(&sizes, st.mode, fit, vp, current),
            };
            let old = std::mem::take(&mut st.layout);
            let new = layout::layout(&sizes, st.mode, zoom, vp, current);
            let comparable = old.rects.len() == new.rects.len() && !old.rects.is_empty() && old.mode == new.mode;
            new_scroll = if comparable {
                let a = anchor.unwrap_or(Point::new(vp.w / 2.0, 0.0));
                layout::anchored_scroll(&old, &new, scroll, a)
            } else {
                Point::new(0.0, new.scroll_to_page(current))
            };
            st.zoom = zoom;
            st.layout = new;
            st.last_viewport = vp;
            self.ui.set_content_width(st.layout.content.w);
            self.ui.set_content_height(st.layout.content.h);
            self.ui.set_zoom_label(format!("{:.0}%", zoom * 100.0).into());
            st.pending_pages.clear();
        }
        self.render_gen.bump();
        self.ui.invoke_scroll_canvas_to(new_scroll.x.max(0.0), new_scroll.y.max(0.0));
        self.update_view();
        self.update_thumbs();
    }

    pub(crate) fn viewport_changed(&self) {
        let vp = self.viewport();
        let (has_doc, last, fit) = {
            let st = self.st.borrow();
            (st.doc.is_some(), st.last_viewport, !matches!(st.zoom_mode, ZoomMode::Custom(_)))
        };
        if !has_doc {
            return;
        }
        let resized = (vp.w - last.w).abs() > 0.5 || (vp.h - last.h).abs() > 0.5;
        if resized && (fit || last.w == 0.0) {
            self.relayout(None);
        } else {
            if resized {
                self.st.borrow_mut().last_viewport = vp;
            }
            self.update_view();
        }
    }

    /// Rebuilds the visible-page model and requests any renders it needs.
    pub(crate) fn update_view(&self) {
        let vp = self.viewport();
        let scroll = self.scroll();
        let scale_factor = sf(self);
        let mut to_render: Vec<(usize, u32, u32, f32)> = Vec::new();
        let mut items = Vec::new();
        let (doc_id, subtitle, current);
        {
            let mut st = self.st.borrow_mut();
            let Some(doc) = st.doc.as_ref() else { return };
            doc_id = doc.id;
            let n = doc.sizes.len();
            let visible = st.layout.visible(scroll.y - OVERSCAN, scroll.y + vp.h + OVERSCAN);
            current = if st.mode == DisplayMode::SinglePage {
                st.current_page
            } else {
                st.layout.current_page(scroll.y, vp.h)
            };
            st.current_page = current;
            subtitle = format!("Page {} of {}", current + 1, n);
            let view_rect = Rect::new(scroll.x, scroll.y, vp.w, vp.h);
            for i in visible {
                let r = st.layout.rects[i];
                let (want_w, want_h, render_scale) = target_size(r, st.layout.scale, scale_factor);
                let cached = st.page_cache.as_mut().unwrap().get(&i).map(|p| (p.width_px, p.image.clone()));
                if cached.as_ref().is_none_or(|(w, _)| *w != want_w) && !st.pending_pages.contains(&(i, want_w)) {
                    st.pending_pages.insert((i, want_w));
                    to_render.push((i, want_w, want_h, render_scale));
                }
                let mut item = PageItem {
                    index: i as i32,
                    x: r.x,
                    y: r.y,
                    width: r.w,
                    height: r.h,
                    label: format!("Page {} of {}", i + 1, n).into(),
                    ..Default::default()
                };
                if let Some((_, img)) = cached {
                    item.image = img;
                    item.has_image = true;
                } else if let Some(t) = st.thumb_cache.as_mut().unwrap().get(&i) {
                    // Show the blurry thumbnail until the page arrives.
                    item.image = t.clone();
                    item.has_image = true;
                }
                if let Some(t) = st.tile.as_ref().filter(|t| t.page == i && r.intersects(&view_rect)) {
                    item.tile = t.image.clone();
                    item.has_tile = true;
                    item.tile_x = t.view.x - r.x;
                    item.tile_y = t.view.y - r.y;
                    item.tile_width = t.view.w;
                    item.tile_height = t.view.h;
                }
                items.push(item);
            }
            if st.selected.len() <= 1 {
                st.selected = vec![current];
            }
        }
        self.ui.set_doc_subtitle(subtitle.into());
        self.pages_model.set_vec(items);
        let generation = self.render_gen.current();
        for (page, w, h, scale) in to_render {
            self.request_page(doc_id, page, w, h, scale, generation.clone());
        }
        self.schedule_tile();
        self.sync_thumb_selection();
    }

    fn request_page(&self, doc_id: DocId, page: usize, w: u32, h: u32, scale: f32, generation: Generation) {
        let win = self.id;
        self.engine.submit(Priority::Render, Some(generation), move |st| {
            let Some(doc) = st.doc(doc_id) else { return };
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
            let ok = doc.render(
                page,
                scale,
                0.0,
                0.0,
                RenderTarget { pixels: buf.make_mut_bytes(), width: w, height: h },
                true,
            );
            if ok {
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |win| win.on_page_rendered(doc_id, page, w, buf));
                });
            }
        });
    }

    fn on_page_rendered(&self, doc_id: DocId, page: usize, width_px: u32, buf: SharedPixelBuffer<Rgba8Pixel>) {
        let first;
        {
            let mut st = self.st.borrow_mut();
            if st.doc.as_ref().is_none_or(|d| d.id != doc_id) {
                return;
            }
            st.pending_pages.remove(&(page, width_px));
            let cost = (buf.width() * buf.height() * 4) as usize;
            st.page_cache.as_mut().unwrap().insert(page, PageImage { width_px, image: Image::from_rgba8(buf) }, cost);
            first = !st.first_render_logged;
            st.first_render_logged = true;
        }
        if first {
            super::super::measure("first-page-rendered");
            if super::super::context().options.exit_after_render {
                // Let the frame paint before quitting.
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
        self.update_view();
    }

    /// After scrolling settles, renders a full-resolution tile for the visible part of
    /// a page whose full bitmap would exceed `MAX_PAGE_PIXELS`.
    fn schedule_tile(&self) {
        let id = self.id;
        self.tile_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(120), move || {
            super::super::with_window(id, |w| w.request_tile());
        });
    }

    fn request_tile(&self) {
        let vp = self.viewport();
        let scroll = self.scroll();
        let scale_factor = sf(self);
        let mut st = self.st.borrow_mut();
        let Some(doc) = st.doc.as_ref() else { return };
        let doc_id = doc.id;
        let view_rect = Rect::new(scroll.x, scroll.y, vp.w, vp.h);
        let full_scale = st.layout.scale * scale_factor;
        let candidate = st.layout.visible(scroll.y, scroll.y + vp.h).into_iter().find(|i| {
            let r = st.layout.rects[*i];
            (r.w * scale_factor) as u64 * (r.h * scale_factor) as u64 > MAX_PAGE_PIXELS
        });
        let Some(page) = candidate else {
            st.tile = None;
            return;
        };
        let r = st.layout.rects[page];
        let Some(vis) = r.intersection(&view_rect) else { return };
        let ox = ((vis.x - r.x) * scale_factor).floor() as i32;
        let oy = ((vis.y - r.y) * scale_factor).floor() as i32;
        let w = (vis.w * scale_factor).ceil() as u32 + 1;
        let h = (vis.h * scale_factor).ceil() as u32 + 1;
        let key = (full_scale.to_bits(), ox, oy, w, h);
        if st.tile.as_ref().is_some_and(|t| t.page == page && t.key == key) || st.pending_tile == Some((page, key)) {
            return;
        }
        st.pending_tile = Some((page, key));
        let view = Rect::new(
            r.x + ox as f32 / scale_factor,
            r.y + oy as f32 / scale_factor,
            w as f32 / scale_factor,
            h as f32 / scale_factor,
        );
        drop(st);
        let win = self.id;
        let generation = self.render_gen.current();
        self.engine.submit(Priority::Render, Some(generation), move |s| {
            let Some(doc) = s.doc(doc_id) else { return };
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
            let ok = doc.render(
                page,
                full_scale,
                ox as f32,
                oy as f32,
                RenderTarget { pixels: buf.make_mut_bytes(), width: w, height: h },
                true,
            );
            if ok {
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |win| {
                        {
                            let mut st = win.st.borrow_mut();
                            if st.doc.as_ref().is_none_or(|d| d.id != doc_id) {
                                return;
                            }
                            st.pending_tile = None;
                            st.tile = Some(Tile { page, key, view, image: Image::from_rgba8(buf) });
                        }
                        win.update_view();
                    });
                });
            }
        });
    }

    /// Writes the current window contents to a PNG (visual verification).
    pub fn save_snapshot(&self, path: &Path) {
        match self.ui.window().take_snapshot() {
            Ok(buf) => {
                let r = image::RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec())
                    .map(|img| img.save(path));
                match r {
                    Some(Ok(())) => eprintln!("snapshot written to {}", path.display()),
                    other => eprintln!("snapshot failed: {other:?}"),
                }
            }
            Err(e) => eprintln!("snapshot failed: {e}"),
        }
    }

    // ----- Thumbnails -------------------------------------------------------------

    pub(crate) fn update_thumbs(&self) {
        if !self.ui.get_sidebar_visible() {
            return;
        }
        let scale_factor = sf(self);
        let view_w = self.ui.get_thumbs_view_width().max(60.0);
        let top = self.ui.get_thumbs_scroll_y();
        let bottom = top + self.ui.get_thumbs_view_height();
        let mut items = Vec::new();
        let mut requests = Vec::new();
        let doc_id;
        {
            let mut st = self.st.borrow_mut();
            let Some(doc) = st.doc.as_ref() else {
                drop(st);
                self.thumbs_model.set_vec(Vec::new());
                return;
            };
            doc_id = doc.id;
            let max_w = (view_w - 2.0 * THUMB_PAD - 12.0).clamp(40.0, 260.0);
            let max_h = max_w * 1.3;
            let mut y = 8.0;
            let mut rects = Vec::with_capacity(doc.sizes.len());
            for s in &doc.sizes {
                let k = (max_w / s.w).min(max_h / s.h);
                let (w, h) = ((s.w * k).round(), (s.h * k).round());
                rects.push(Rect::new((view_w - w - 12.0) / 2.0, y, w, h));
                y += h + THUMB_LABEL_H + 6.0;
            }
            self.ui.set_thumbs_content_height(y + 8.0);
            let selected = st.selected.clone();
            for (i, r) in rects.iter().enumerate() {
                if r.bottom() + THUMB_LABEL_H < top - 200.0 || r.y > bottom + 200.0 {
                    continue;
                }
                let mut item = ThumbItem {
                    index: i as i32,
                    x: r.x,
                    y: r.y,
                    width: r.w,
                    height: r.h,
                    label: (i + 1).to_string().into(),
                    selected: selected.contains(&i),
                    ..Default::default()
                };
                if let Some(img) = st.thumb_cache.as_mut().unwrap().get(&i) {
                    item.image = img.clone();
                    item.has_image = true;
                } else if !st.pending_thumbs.contains(&i) {
                    st.pending_thumbs.insert(i);
                    requests.push((i, (r.w * scale_factor).round().max(1.0) as u32));
                }
                items.push(item);
            }
            st.thumb_rects = rects;
        }
        self.thumbs_model.set_vec(items);
        let generation = self.thumb_gen.current();
        let win = self.id;
        for (page, w) in requests {
            self.engine.submit(Priority::Thumbnail, Some(generation.clone()), move |s| {
                let Some(doc) = s.doc(doc_id) else { return };
                let Some((w, h, px)) = doc.render_to_vec(page, w) else { return };
                let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px, w, h);
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |win| win.on_thumb_rendered(doc_id, page, buf));
                });
            });
        }
    }

    fn on_thumb_rendered(&self, doc_id: DocId, page: usize, buf: SharedPixelBuffer<Rgba8Pixel>) {
        let img = {
            let mut st = self.st.borrow_mut();
            if st.doc.as_ref().is_none_or(|d| d.id != doc_id) {
                return;
            }
            st.pending_thumbs.remove(&page);
            let cost = (buf.width() * buf.height() * 4) as usize;
            let img = Image::from_rgba8(buf);
            st.thumb_cache.as_mut().unwrap().insert(page, img.clone(), cost);
            img
        };
        // Patch the row in place rather than rebuilding the model.
        for row in 0..self.thumbs_model.row_count() {
            let mut item = self.thumbs_model.row_data(row).unwrap();
            if item.index as usize == page {
                item.image = img;
                item.has_image = true;
                self.thumbs_model.set_row_data(row, item);
                break;
            }
        }
    }

    fn sync_thumb_selection(&self) {
        let selected = self.st.borrow().selected.clone();
        for row in 0..self.thumbs_model.row_count() {
            let mut item = self.thumbs_model.row_data(row).unwrap();
            let sel = selected.contains(&(item.index as usize));
            if item.selected != sel {
                item.selected = sel;
                self.thumbs_model.set_row_data(row, item);
            }
        }
    }

    // ----- Navigation and zoom ----------------------------------------------------

    pub fn go_to_page(&self, page: usize) {
        let (single, page) = {
            let mut st = self.st.borrow_mut();
            let Some(doc) = st.doc.as_ref() else { return };
            let page = page.min(doc.sizes.len().saturating_sub(1));
            st.current_page = page;
            st.selected = vec![page];
            (st.mode == DisplayMode::SinglePage, page)
        };
        if single {
            self.relayout(None);
            self.ui.invoke_scroll_canvas_to(0.0, 0.0);
            self.update_view();
        } else {
            let y = self.st.borrow().layout.scroll_to_page(page);
            self.ui.invoke_scroll_canvas_to(self.scroll().x, y);
            self.update_view();
        }
        self.ensure_thumb_visible(page);
    }

    pub(crate) fn step_page(&self, delta: i64) {
        let (current, mode) = {
            let st = self.st.borrow();
            (st.current_page as i64, st.mode)
        };
        let step = if mode == DisplayMode::TwoPages { 2 * delta } else { delta };
        self.go_to_page((current + step).max(0) as usize);
    }

    fn ensure_thumb_visible(&self, page: usize) {
        let r = self.st.borrow().thumb_rects.get(page).copied();
        if let Some(r) = r {
            let top = self.ui.get_thumbs_scroll_y();
            let h = self.ui.get_thumbs_view_height();
            if r.y < top || r.bottom() + THUMB_LABEL_H > top + h {
                self.ui.invoke_scroll_thumbs_to((r.y - 8.0).max(0.0));
            }
        }
    }

    pub(crate) fn set_zoom_mode(&self, mode: ZoomMode, anchor: Option<Point>) {
        self.st.borrow_mut().zoom_mode = mode;
        self.relayout(anchor);
    }

    pub(crate) fn zoom_step(&self, zoom_in: bool) {
        let z = self.st.borrow().zoom;
        let nz = if zoom_in { layout::zoom_in_step(z) } else { layout::zoom_out_step(z) };
        let vp = self.viewport();
        self.set_zoom_mode(ZoomMode::Custom(nz), Some(Point::new(vp.w / 2.0, vp.h / 2.0)));
    }

    pub(crate) fn wheel_zoom(&self, delta: f32, x: f32, y: f32) {
        let z = self.st.borrow().zoom;
        let nz = layout::clamp_zoom(z * 1.0015f32.powf(delta));
        if (nz - z).abs() > 1e-4 {
            self.set_zoom_mode(ZoomMode::Custom(nz), Some(Point::new(x, y)));
        }
    }

    pub(crate) fn canvas_key(&self, e: &slint::language::KeyEvent) -> bool {
        use slint::platform::Key;
        let t = e.text.as_str();
        let vp = self.viewport();
        let s = self.scroll();
        let single = self.st.borrow().mode == DisplayMode::SinglePage;
        let is = |k: Key| t == SharedString::from(k).as_str();
        if is(Key::PageDown) || (t == " " && !e.modifiers.shift) {
            if single {
                self.step_page(1);
            } else {
                self.ui.invoke_scroll_canvas_to(s.x, s.y + vp.h * 0.9);
            }
        } else if is(Key::PageUp) || (t == " " && e.modifiers.shift) {
            if single {
                self.step_page(-1);
            } else {
                self.ui.invoke_scroll_canvas_to(s.x, (s.y - vp.h * 0.9).max(0.0));
            }
        } else if is(Key::DownArrow) {
            self.ui.invoke_scroll_canvas_to(s.x, s.y + 40.0);
        } else if is(Key::UpArrow) {
            self.ui.invoke_scroll_canvas_to(s.x, (s.y - 40.0).max(0.0));
        } else if is(Key::RightArrow) {
            self.step_page(1);
        } else if is(Key::LeftArrow) {
            self.step_page(-1);
        } else if is(Key::Home) {
            self.go_to_page(0);
        } else if is(Key::End) {
            self.go_to_page(usize::MAX);
        } else {
            return false;
        }
        true
    }

    pub(crate) fn thumbs_key(&self, e: &slint::language::KeyEvent) -> bool {
        use slint::platform::Key;
        let t = e.text.as_str();
        let is = |k: Key| t == SharedString::from(k).as_str();
        if is(Key::DownArrow) || is(Key::RightArrow) {
            self.step_page(1);
        } else if is(Key::UpArrow) || is(Key::LeftArrow) {
            self.step_page(-1);
        } else {
            return false;
        }
        true
    }
}

/// Device-pixel render size for a page rect, capped to `MAX_PAGE_PIXELS`.
pub(crate) fn target_size(r: Rect, layout_scale: f32, scale_factor: f32) -> (u32, u32, f32) {
    let mut w = (r.w * scale_factor).round().max(1.0);
    let mut h = (r.h * scale_factor).round().max(1.0);
    let px = w as f64 * h as f64;
    if px > MAX_PAGE_PIXELS as f64 {
        let k = (MAX_PAGE_PIXELS as f64 / px).sqrt() as f32;
        w = (w * k).floor().max(1.0);
        h = (h * k).floor().max(1.0);
    }
    // Device pixels per point for the (possibly capped) bitmap.
    let scale = w / (r.w / layout_scale);
    (w as u32, h as u32, scale)
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_caps_huge_pages() {
        let (w, h, s) = target_size(Rect::new(0.0, 0.0, 800.0, 1000.0), 2.0, 1.5);
        assert_eq!((w, h), (1200, 1500));
        assert!((s - 3.0).abs() < 1e-4);
        let (w, h, _) = target_size(Rect::new(0.0, 0.0, 20000.0, 26000.0), 25.0, 1.0);
        assert!(w as u64 * h as u64 <= MAX_PAGE_PIXELS);
        assert!((w as f32 / h as f32 - 20000.0 / 26000.0).abs() < 0.01);
    }
}
