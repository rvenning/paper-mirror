//! Contact sheet (grid of pages in place of the canvas) and the full-screen slideshow.

use std::cell::Cell;
use std::time::Duration;

use slint::{ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer, SharedString};

use super::DocWindow;
use crate::core::geometry::Rect;
use crate::pdf::engine::{DocId, GenerationCounter, Priority};
use crate::ui::{SheetItem, SlideshowWindow};

const SHEET_CELL: f32 = 180.0;
const SHEET_GAP: f32 = 16.0;
const SHEET_LABEL: f32 = 34.0;

pub(crate) struct Slideshow {
    pub window: SlideshowWindow,
    pub page: Cell<usize>,
    pub doc_id: DocId,
    pub pages: usize,
    pub generation: GenerationCounter,
    pub caption_timer: slint::Timer,
}

impl DocWindow {
    pub(crate) fn toggle_contact_sheet(&self) {
        let on = !self.ui.get_contact_sheet();
        self.ui.set_contact_sheet(on);
        if on {
            self.update_sheet();
        } else {
            let page = self.current_page();
            self.relayout(None);
            self.go_to_page(page);
        }
    }

    /// Lays out the grid and requests images for the visible cells.
    pub(crate) fn update_sheet(&self) {
        if !self.ui.get_contact_sheet() {
            return;
        }
        let scale_factor = self.ui.window().scale_factor();
        let view_w = self.ui.get_view_width().max(SHEET_CELL + 2.0 * SHEET_GAP);
        let top = self.ui.get_sheet_scroll_y();
        let bottom = top + self.ui.get_view_height();
        let mut items = Vec::new();
        let mut requests = Vec::new();
        let doc_id;
        {
            let mut st = self.st.borrow_mut();
            let Some(doc) = st.doc.as_ref() else { return };
            doc_id = doc.id;
            let n = doc.sizes.len();
            let cols = (((view_w - SHEET_GAP) / (SHEET_CELL + 16.0 + SHEET_GAP)).floor() as usize).max(1);
            let row_h = SHEET_CELL * 1.3 + SHEET_LABEL + SHEET_GAP;
            let grid_w = cols as f32 * (SHEET_CELL + 16.0 + SHEET_GAP) - SHEET_GAP;
            let left = ((view_w - grid_w) / 2.0).max(SHEET_GAP);
            let rows = n.div_ceil(cols);
            self.ui.set_sheet_content_height(rows as f32 * row_h + 2.0 * SHEET_GAP);
            let sizes = doc.sizes.clone();
            let current = st.current_page;
            for (i, s) in sizes.iter().enumerate() {
                let (row, col) = (i / cols, i % cols);
                let k = (SHEET_CELL / s.w).min(SHEET_CELL * 1.3 / s.h);
                let (w, h) = ((s.w * k).round(), (s.h * k).round());
                let cell_x = left + col as f32 * (SHEET_CELL + 16.0 + SHEET_GAP);
                let cell_y = SHEET_GAP + row as f32 * row_h;
                let r = Rect::new(cell_x + (SHEET_CELL - w) / 2.0, cell_y + (SHEET_CELL * 1.3 - h), w, h);
                if r.bottom() + SHEET_LABEL < top - 200.0 || r.y > bottom + 200.0 {
                    continue;
                }
                let mut item = SheetItem {
                    index: i as i32,
                    x: r.x,
                    y: r.y,
                    width: r.w,
                    height: r.h,
                    label: format!("{}", i + 1).into(),
                    selected: i == current,
                    ..Default::default()
                };
                if let Some(img) = st.sheet_cache.as_mut().unwrap().get(&i) {
                    item.image = img.clone();
                    item.has_image = true;
                } else if !st.pending_sheet.contains(&i) {
                    st.pending_sheet.insert(i);
                    requests.push((i, (w * scale_factor).round().max(1.0) as u32));
                }
                items.push(item);
            }
        }
        self.sheet_model.set_vec(items);
        let generation = self.thumb_gen.current();
        let win = self.id;
        for (page, w) in requests {
            self.engine.submit(Priority::Thumbnail, Some(generation.clone()), move |s| {
                let Some(doc) = s.doc(doc_id) else { return };
                let Some((w, h, px)) = doc.render_to_vec(page, w) else { return };
                let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px, w, h);
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |win| win.on_sheet_rendered(doc_id, page, buf));
                });
            });
        }
    }

    fn on_sheet_rendered(&self, doc_id: DocId, page: usize, buf: SharedPixelBuffer<Rgba8Pixel>) {
        let img = {
            let mut st = self.st.borrow_mut();
            if st.doc.as_ref().is_none_or(|d| d.id != doc_id) {
                return;
            }
            st.pending_sheet.remove(&page);
            let cost = (buf.width() * buf.height() * 4) as usize;
            let img = Image::from_rgba8(buf);
            st.sheet_cache.as_mut().unwrap().insert(page, img.clone(), cost);
            img
        };
        for row in 0..self.sheet_model.row_count() {
            let mut item = self.sheet_model.row_data(row).unwrap();
            if item.index as usize == page {
                item.image = img;
                item.has_image = true;
                self.sheet_model.set_row_data(row, item);
                break;
            }
        }
    }

    pub(crate) fn sheet_select(&self, page: usize) {
        self.st.borrow_mut().current_page = page;
        self.st.borrow_mut().selected = vec![page];
        for row in 0..self.sheet_model.row_count() {
            let mut item = self.sheet_model.row_data(row).unwrap();
            let sel = item.index as usize == page;
            if item.selected != sel {
                item.selected = sel;
                self.sheet_model.set_row_data(row, item);
            }
        }
    }

    /// Double-click in the contact sheet: leave it and show that page.
    pub(crate) fn sheet_open(&self, page: usize) {
        self.sheet_select(page);
        self.ui.set_contact_sheet(false);
        self.relayout(None);
        self.go_to_page(page);
    }

    // ----- Slideshow ---------------------------------------------------------------

    pub(crate) fn start_slideshow(&self) {
        let Some((doc_id, pages)) = self.st.borrow().doc.as_ref().map(|d| (d.id, d.sizes.len())) else { return };
        if self.slideshow.borrow().is_some() {
            return;
        }
        let Ok(window) = SlideshowWindow::new() else { return };
        let page = self.current_page();
        let id = self.id;
        window.on_key(move |e| {
            let mut handled = false;
            super::super::with_window(id, |w| handled = w.slideshow_key(&e));
            handled
        });
        window.on_clicked(move |right| super::super::with_window(id, |w| w.slideshow_step(if right { -1 } else { 1 })));
        window.on_resized(move || super::super::with_window(id, |w| w.render_slide()));
        window.window().on_close_requested(move || {
            slint::Timer::single_shot(Duration::ZERO, move || super::super::with_window(id, |w| w.end_slideshow()));
            slint::CloseRequestResponse::HideWindow
        });
        let _ = window.show();
        window.window().set_fullscreen(true);
        *self.slideshow.borrow_mut() = Some(Slideshow {
            window,
            page: Cell::new(page),
            doc_id,
            pages,
            generation: GenerationCounter::default(),
            caption_timer: slint::Timer::default(),
        });
        self.render_slide();
    }

    pub(crate) fn end_slideshow(&self) {
        if let Some(s) = self.slideshow.borrow_mut().take() {
            s.generation.bump();
            let page = s.page.get();
            let _ = s.window.hide();
            drop(s);
            self.go_to_page(page);
        }
    }

    pub fn slideshow_page(&self) -> Option<usize> {
        self.slideshow.borrow().as_ref().map(|s| s.page.get())
    }

    pub fn slideshow_step(&self, delta: i64) {
        let done = {
            let ss = self.slideshow.borrow();
            let Some(s) = ss.as_ref() else { return };
            let next = s.page.get() as i64 + delta;
            if next >= s.pages as i64 {
                true // past the last slide: end, as Preview does
            } else {
                s.page.set(next.max(0) as usize);
                false
            }
        };
        if done {
            self.end_slideshow();
        } else {
            self.render_slide();
        }
    }

    pub fn slideshow_key(&self, e: &slint::language::KeyEvent) -> bool {
        use slint::platform::Key;
        let t = e.text.as_str();
        let is = |k: Key| t == SharedString::from(k).as_str();
        if is(Key::Escape) {
            self.end_slideshow();
        } else if is(Key::RightArrow) || is(Key::DownArrow) || is(Key::PageDown) || t == " " || t == "\n" || t == "\r" {
            self.slideshow_step(1);
        } else if is(Key::LeftArrow) || is(Key::UpArrow) || is(Key::PageUp) || is(Key::Backspace) {
            self.slideshow_step(-1);
        } else if is(Key::Home) {
            self.slideshow_step(-(i64::MAX / 2));
        } else if is(Key::End) {
            let n = self.slideshow.borrow().as_ref().map(|s| s.pages).unwrap_or(1);
            let cur = self.slideshow_page().unwrap_or(0);
            self.slideshow_step(n as i64 - 1 - cur as i64);
        } else {
            return false;
        }
        true
    }

    fn render_slide(&self) {
        let ss = self.slideshow.borrow();
        let Some(s) = ss.as_ref() else { return };
        let page = s.page.get();
        let doc_id = s.doc_id;
        let sf = s.window.window().scale_factor();
        let vw = (s.window.get_view_width() * sf).max(64.0);
        let vh = (s.window.get_view_height() * sf).max(64.0);
        s.window.set_caption(format!("{} of {}", page + 1, s.pages).into());
        s.window.set_caption_visible(true);
        let id = self.id;
        s.caption_timer.start(slint::TimerMode::SingleShot, Duration::from_secs(2), move || {
            super::super::with_window(id, |w| {
                if let Some(s) = w.slideshow.borrow().as_ref() {
                    s.window.set_caption_visible(false);
                }
            });
        });
        let size = self.st.borrow().doc.as_ref().and_then(|d| d.sizes.get(page).copied());
        let Some(size) = size else { return };
        let k = (vw / size.w).min(vh / size.h);
        let (w, h) = ((size.w * k).round().max(1.0) as u32, (size.h * k).round().max(1.0) as u32);
        let generation = s.generation.bump();
        drop(ss);
        let win = self.id;
        self.engine.submit(Priority::Interactive, Some(generation), move |st| {
            let Some(doc) = st.doc(doc_id) else { return };
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
            let ok = doc.render(
                page,
                k,
                0.0,
                0.0,
                crate::pdf::document::RenderTarget { pixels: buf.make_mut_bytes(), width: w, height: h },
                true,
            );
            if ok {
                let _ = slint::invoke_from_event_loop(move || {
                    super::super::with_window(win, |w| {
                        if let Some(s) = w.slideshow.borrow().as_ref().filter(|s| s.page.get() == page) {
                            s.window.set_slide(Image::from_rgba8(buf));
                            s.window.set_has_slide(true);
                        }
                    });
                });
            }
        });
    }
}
