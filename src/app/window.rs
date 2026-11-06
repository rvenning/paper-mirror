//! A document window: binds the Slint `MainWindow` to a document and drives
//! virtualised rendering through the engine thread.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::core::cache::ByteLru;
use crate::core::geometry::{Point, Rect, Size};
use crate::core::layout::{self, DisplayMode, Layout, ZoomMode};
use crate::pdf::document::{PdfDoc, RenderTarget};
use crate::pdf::engine::{DocId, Engine, GenerationCounter, Priority};
use crate::pdf::ffi::Source;
use crate::ui::{MainWindow, PageItem, ThumbItem};

/// Upper bound for one full-page bitmap. Beyond this the page is rendered at a capped
/// resolution and a detail tile covers the visible region at full resolution.
const MAX_PAGE_PIXELS: u64 = 12_000_000;
const PAGE_CACHE_BYTES: usize = 160 * 1024 * 1024;
const THUMB_CACHE_BYTES: usize = 24 * 1024 * 1024;
/// Extra band rendered above and below the viewport, in logical pixels.
const OVERSCAN: f32 = 300.0;
const THUMB_LABEL_H: f32 = 30.0;
const THUMB_PAD: f32 = 12.0;

static NEXT_WINDOW_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct OpenDoc {
    id: DocId,
    path: PathBuf,
    title: String,
    sizes: Vec<Size>,
}

struct PageImage {
    width_px: u32,
    image: Image,
}

struct Tile {
    page: usize,
    key: (u32, i32, i32, u32, u32),
    view: Rect,
    image: Image,
}

#[derive(Default)]
struct ViewState {
    doc: Option<OpenDoc>,
    zoom_mode: ZoomMode,
    zoom: f32,
    mode: DisplayMode,
    layout: Layout,
    last_viewport: Size,
    current_page: usize,
    selected: Vec<usize>,
    page_cache: Option<ByteLru<usize, PageImage>>,
    thumb_cache: Option<ByteLru<usize, Image>>,
    pending_pages: HashSet<(usize, u32)>,
    pending_thumbs: HashSet<usize>,
    tile: Option<Tile>,
    pending_tile: Option<(usize, (u32, i32, i32, u32, u32))>,
    thumb_rects: Vec<Rect>,
    first_render_logged: bool,
}

pub struct DocWindow {
    pub id: u64,
    pub ui: MainWindow,
    engine: Engine,
    st: RefCell<ViewState>,
    pages_model: Rc<VecModel<PageItem>>,
    thumbs_model: Rc<VecModel<ThumbItem>>,
    render_gen: GenerationCounter,
    thumb_gen: GenerationCounter,
    tile_timer: slint::Timer,
}

fn sf(ui: &MainWindow) -> f32 {
    ui.window().scale_factor()
}

impl DocWindow {
    /// Creates and shows a window, optionally opening a file in it.
    pub fn open_new(path: Option<PathBuf>) -> Result<Rc<DocWindow>, String> {
        let ui = MainWindow::new().map_err(|e| e.to_string())?;
        let ctx = super::context();
        let w = Rc::new(DocWindow {
            id: NEXT_WINDOW_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ui,
            engine: ctx.engine.clone(),
            st: RefCell::new(ViewState {
                zoom: 1.0,
                page_cache: Some(ByteLru::new(PAGE_CACHE_BYTES)),
                thumb_cache: Some(ByteLru::new(THUMB_CACHE_BYTES)),
                ..Default::default()
            }),
            pages_model: Rc::new(VecModel::default()),
            thumbs_model: Rc::new(VecModel::default()),
            render_gen: GenerationCounter::default(),
            thumb_gen: GenerationCounter::default(),
            tile_timer: slint::Timer::default(),
        });
        w.ui.set_pages(ModelRc::from(w.pages_model.clone()));
        w.ui.set_thumbs(ModelRc::from(w.thumbs_model.clone()));
        w.ui.set_app_version(crate::VERSION.into());
        w.bind();
        super::register_window(w.clone());
        w.ui.show().map_err(|e| e.to_string())?;
        if let Some(p) = path {
            w.open_path(p);
        }
        Ok(w)
    }

    fn bind(self: &Rc<Self>) {
        let id = self.id;
        self.ui.on_command(move |cmd| super::with_window(id, |w| w.command(cmd.as_str())));
        self.ui.on_canvas_viewport_changed(move || super::with_window(id, |w| w.viewport_changed()));
        self.ui.on_thumbs_viewport_changed(move || super::with_window(id, |w| w.update_thumbs()));
        self.ui.on_canvas_ctrl_wheel(move |delta, x, y| super::with_window(id, |w| w.wheel_zoom(delta, x, y)));
        self.ui.on_thumb_clicked(move |i, _ctrl, _shift| super::with_window(id, |w| w.go_to_page(i as usize)));
        self.ui.on_canvas_key(move |e| {
            let mut handled = false;
            super::with_window(id, |w| handled = w.canvas_key(&e));
            handled
        });
        self.ui.on_thumbs_key(move |e| {
            let mut handled = false;
            super::with_window(id, |w| handled = w.thumbs_key(&e));
            handled
        });
        self.ui.window().on_close_requested(move || {
            // Defer removal: the window must not be dropped inside its own callback.
            slint::Timer::single_shot(Duration::ZERO, move || super::unregister_window(id));
            slint::CloseRequestResponse::HideWindow
        });
        let id2 = self.id;
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
        self.ui.window().on_winit_window_event(move |_w, ev| {
            if let winit::event::WindowEvent::DroppedFile(path) = ev {
                let path = path.clone();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    super::with_window(id2, |w| w.open_dropped(path));
                });
                return EventResult::PreventDefault;
            }
            EventResult::Propagate
        });
    }

    pub fn command(&self, cmd: &str) {
        match cmd {
            "open" => self.choose_and_open(),
            "close" => {
                let id = self.id;
                let _ = self.ui.hide();
                slint::Timer::single_shot(Duration::ZERO, move || super::unregister_window(id));
            }
            "toggle-sidebar" => {
                self.ui.set_sidebar_visible(!self.ui.get_sidebar_visible());
                self.update_thumbs();
            }
            "zoom-in" => self.zoom_step(true),
            "zoom-out" => self.zoom_step(false),
            "zoom-actual" => self.set_zoom_mode(ZoomMode::Custom(1.0), None),
            "zoom-fit-page" => self.set_zoom_mode(ZoomMode::FitPage, None),
            "zoom-fit-width" => self.set_zoom_mode(ZoomMode::FitWidth, None),
            "show-notices" => show_notices(),
            _ => eprintln!("paper-mirror: unhandled command {cmd}"),
        }
    }

    fn choose_and_open(&self) {
        let dialog = rfd::FileDialog::new()
            .set_title("Open")
            .add_filter("PDF documents", &["pdf"])
            .add_filter("All files", &["*"])
            .set_parent(&self.ui.window().window_handle());
        if let Some(path) = dialog.pick_file() {
            self.open_dropped(path);
        }
    }

    /// Opens a file here if this window is empty, otherwise in a new window.
    fn open_dropped(&self, path: PathBuf) {
        if self.st.borrow().doc.is_none() {
            self.open_path(path);
        } else if let Err(e) = DocWindow::open_new(Some(path)) {
            self.error("Could not open a new window", &e);
        }
    }

    pub fn open_path(&self, path: PathBuf) {
        let engine = self.engine.clone();
        let id = engine.new_doc_id();
        let win = self.id;
        let p = path.clone();
        engine.submit(Priority::Interactive, None, move |st| {
            let result = PdfDoc::open(Source::File(p.clone()), None).map(|doc| {
                let sizes: Vec<Size> = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
                st.docs.insert(id, doc);
                sizes
            });
            let msg = result.map_err(|e| e.to_string());
            let _ = slint::invoke_from_event_loop(move || {
                super::with_window(win, |w| w.on_opened(id, p, msg));
            });
        });
    }

    fn on_opened(&self, id: DocId, path: PathBuf, result: Result<Vec<Size>, String>) {
        let sizes = match result {
            Ok(s) => s,
            Err(e) => {
                self.error(&format!("“{}” could not be opened.", file_name(&path)), &e);
                return;
            }
        };
        super::measure("document-opened");
        let title = file_name(&path);
        {
            let mut st = self.st.borrow_mut();
            if let Some(old) = st.doc.take() {
                self.engine.submit(Priority::Interactive, None, move |s| {
                    s.docs.remove(&old.id);
                });
            }
            st.page_cache.as_mut().unwrap().clear();
            st.thumb_cache.as_mut().unwrap().clear();
            st.pending_pages.clear();
            st.pending_thumbs.clear();
            st.tile = None;
            st.current_page = 0;
            st.selected = vec![0];
            st.zoom_mode = ZoomMode::FitWidth;
            st.doc = Some(OpenDoc { id, path, title: title.clone(), sizes });
        }
        self.render_gen.bump();
        self.thumb_gen.bump();
        self.ui.set_doc_title(title.into());
        self.ui.set_has_document(true);
        self.relayout(None);
        self.ui.window().request_redraw();
    }

    fn error(&self, title: &str, detail: &str) {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Paper Mirror")
            .set_description(format!("{title}\n\n{detail}"))
            .set_parent(&self.ui.window().window_handle())
            .show();
    }

    fn viewport(&self) -> Size {
        Size::new(self.ui.get_view_width(), self.ui.get_view_height())
    }

    fn scroll(&self) -> Point {
        Point::new(self.ui.get_scroll_x(), self.ui.get_scroll_y())
    }

    /// Recomputes the layout. `anchor` (viewport-relative) keeps the point under the
    /// cursor fixed when zooming; otherwise the current page stays at the top.
    fn relayout(&self, anchor: Option<Point>) {
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
            new_scroll = if old.rects.len() == new.rects.len() && !old.rects.is_empty() {
                match anchor {
                    Some(a) => layout::anchored_scroll(&old, &new, scroll, a),
                    None => {
                        // Keep the same relative position within the current page.
                        let top = Point::new(scroll.x + vp.w / 2.0, scroll.y);
                        let a = Point::new(vp.w / 2.0, 0.0);
                        let _ = top;
                        layout::anchored_scroll(&old, &new, scroll, a)
                    }
                }
            } else {
                Point::new(0.0, new.scroll_to_page(current))
            };
            st.zoom = zoom;
            st.layout = new;
            st.last_viewport = vp;
            self.ui.set_content_width(st.layout.content.w);
            self.ui.set_content_height(st.layout.content.h);
            self.ui.set_zoom_label(format!("{:.0}%", zoom * 100.0).into());
        }
        self.render_gen.bump();
        self.st.borrow_mut().pending_pages.clear();
        self.ui.invoke_scroll_canvas_to(new_scroll.x.max(0.0), new_scroll.y.max(0.0));
        self.update_view();
        self.update_thumbs();
    }

    fn viewport_changed(&self) {
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
    fn update_view(&self) {
        let vp = self.viewport();
        let scroll = self.scroll();
        let scale_factor = sf(&self.ui);
        let mut to_render: Vec<(usize, u32, u32, f32)> = Vec::new();
        let mut items = Vec::new();
        let (doc_id, subtitle);
        {
            let mut st = self.st.borrow_mut();
            let Some(doc) = st.doc.as_ref() else { return };
            doc_id = doc.id;
            let n = doc.sizes.len();
            let visible = st.layout.visible(scroll.y - OVERSCAN, scroll.y + vp.h + OVERSCAN);
            let current = st.layout.current_page(scroll.y, vp.h);
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
        set_model(&self.pages_model, items);
        let generation = self.render_gen.current();
        for (page, w, h, scale) in to_render {
            self.request_page(doc_id, page, w, h, scale, generation.clone());
        }
        self.schedule_tile();
        self.sync_thumb_selection();
    }

    fn request_page(&self, doc_id: DocId, page: usize, w: u32, h: u32, scale: f32, generation: crate::pdf::engine::Generation) {
        let win = self.id;
        self.engine.submit(Priority::Render, Some(generation), move |st| {
            let Some(doc) = st.doc(doc_id) else { return };
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
            let ok = doc.render(page, scale, 0.0, 0.0, RenderTarget { pixels: buf.make_mut_bytes(), width: w, height: h }, true);
            if ok {
                let _ = slint::invoke_from_event_loop(move || {
                    super::with_window(win, |win| win.on_page_rendered(doc_id, page, w, buf));
                });
            }
        });
    }

    fn on_page_rendered(&self, doc_id: DocId, page: usize, width_px: u32, buf: SharedPixelBuffer<Rgba8Pixel>) {
        {
            let mut st = self.st.borrow_mut();
            if st.doc.as_ref().is_none_or(|d| d.id != doc_id) {
                return;
            }
            st.pending_pages.remove(&(page, width_px));
            let cost = (buf.width() * buf.height() * 4) as usize;
            st.page_cache.as_mut().unwrap().insert(page, PageImage { width_px, image: Image::from_rgba8(buf) }, cost);
            if !st.first_render_logged {
                st.first_render_logged = true;
                drop(st);
                super::measure("first-page-rendered");
                if super::context().options.exit_after_render {
                    // Let the frame paint before quitting.
                    let win = self.id;
                    slint::Timer::single_shot(Duration::from_millis(600), move || {
                        super::measure("after-first-paint");
                        if let Some(path) = super::context().options.snapshot {
                            super::with_window(win, |w| w.save_snapshot(&path));
                        }
                        let _ = slint::quit_event_loop();
                    });
                }
            }
        }
        self.update_view();
    }

    /// After scrolling settles, renders a full-resolution tile for the visible part of
    /// a page whose full bitmap would exceed `MAX_PAGE_PIXELS`.
    fn schedule_tile(&self) {
        let id = self.id;
        self.tile_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(120), move || {
            super::with_window(id, |w| w.request_tile());
        });
    }

    fn request_tile(&self) {
        let vp = self.viewport();
        let scroll = self.scroll();
        let scale_factor = sf(&self.ui);
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
        let view = Rect::new(r.x + ox as f32 / scale_factor, r.y + oy as f32 / scale_factor, w as f32 / scale_factor, h as f32 / scale_factor);
        drop(st);
        let win = self.id;
        let generation = self.render_gen.current();
        self.engine.submit(Priority::Render, Some(generation), move |s| {
            let Some(doc) = s.doc(doc_id) else { return };
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
            let ok = doc.render(page, full_scale, ox as f32, oy as f32, RenderTarget { pixels: buf.make_mut_bytes(), width: w, height: h }, true);
            if ok {
                let _ = slint::invoke_from_event_loop(move || {
                    super::with_window(win, |win| {
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

    fn update_thumbs(&self) {
        if !self.ui.get_sidebar_visible() {
            return;
        }
        let scale_factor = sf(&self.ui);
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
                set_model(&self.thumbs_model, Vec::new());
                return;
            };
            doc_id = doc.id;
            let n = doc.sizes.len();
            let max_w = (view_w - 2.0 * THUMB_PAD - 12.0).clamp(40.0, 260.0);
            let max_h = max_w * 1.3;
            let mut y = 8.0;
            let mut rects = Vec::with_capacity(n);
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
        set_model(&self.thumbs_model, items);
        let generation = self.thumb_gen.current();
        let win = self.id;
        for (page, w) in requests {
            self.engine.submit(Priority::Thumbnail, Some(generation.clone()), move |s| {
                let Some(doc) = s.doc(doc_id) else { return };
                let Some((w, h, px)) = doc.render_to_vec(page, w) else { return };
                let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px, w, h);
                let _ = slint::invoke_from_event_loop(move || {
                    super::with_window(win, |win| win.on_thumb_rendered(doc_id, page, buf));
                });
            });
        }
    }

    fn on_thumb_rendered(&self, doc_id: DocId, page: usize, buf: SharedPixelBuffer<Rgba8Pixel>) {
        {
            let mut st = self.st.borrow_mut();
            if st.doc.as_ref().is_none_or(|d| d.id != doc_id) {
                return;
            }
            st.pending_thumbs.remove(&page);
            let cost = (buf.width() * buf.height() * 4) as usize;
            st.thumb_cache.as_mut().unwrap().insert(page, Image::from_rgba8(buf), cost);
        }
        // Patch the row in place rather than rebuilding the model.
        for row in 0..self.thumbs_model.row_count() {
            let mut item = self.thumbs_model.row_data(row).unwrap();
            if item.index as usize == page {
                if let Some(img) = self.st.borrow_mut().thumb_cache.as_mut().unwrap().peek(&page) {
                    item.image = img.clone();
                    item.has_image = true;
                }
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
        let (y, single) = {
            let mut st = self.st.borrow_mut();
            let Some(doc) = st.doc.as_ref() else { return };
            let page = page.min(doc.sizes.len().saturating_sub(1));
            st.current_page = page;
            st.selected = vec![page];
            (st.layout.scroll_to_page(page), st.mode == DisplayMode::SinglePage)
        };
        if single {
            self.relayout(None);
        } else {
            self.ui.invoke_scroll_canvas_to(self.scroll().x, y);
            self.update_view();
        }
        self.ensure_thumb_visible(page);
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

    fn set_zoom_mode(&self, mode: ZoomMode, anchor: Option<Point>) {
        self.st.borrow_mut().zoom_mode = mode;
        self.relayout(anchor);
    }

    fn zoom_step(&self, zoom_in: bool) {
        let z = self.st.borrow().zoom;
        let nz = if zoom_in { layout::zoom_in_step(z) } else { layout::zoom_out_step(z) };
        let vp = self.viewport();
        self.set_zoom_mode(ZoomMode::Custom(nz), Some(Point::new(vp.w / 2.0, vp.h / 2.0)));
    }

    fn wheel_zoom(&self, delta: f32, x: f32, y: f32) {
        let z = self.st.borrow().zoom;
        let nz = layout::clamp_zoom(z * 1.0015f32.powf(delta));
        if (nz - z).abs() > 1e-4 {
            self.set_zoom_mode(ZoomMode::Custom(nz), Some(Point::new(x, y)));
        }
    }

    fn canvas_key(&self, e: &slint::language::KeyEvent) -> bool {
        use slint::platform::Key;
        let t = e.text.as_str();
        let current = self.st.borrow().current_page;
        let vp = self.viewport();
        let s = self.scroll();
        let is = |k: Key| t == SharedString::from(k).as_str();
        if is(Key::PageDown) || (t == " " && !e.modifiers.shift) {
            self.ui.invoke_scroll_canvas_to(s.x, s.y + vp.h * 0.9);
        } else if is(Key::PageUp) || (t == " " && e.modifiers.shift) {
            self.ui.invoke_scroll_canvas_to(s.x, (s.y - vp.h * 0.9).max(0.0));
        } else if is(Key::DownArrow) {
            self.ui.invoke_scroll_canvas_to(s.x, s.y + 40.0);
        } else if is(Key::UpArrow) {
            self.ui.invoke_scroll_canvas_to(s.x, (s.y - 40.0).max(0.0));
        } else if is(Key::RightArrow) {
            self.go_to_page(current + 1);
        } else if is(Key::LeftArrow) {
            self.go_to_page(current.saturating_sub(1));
        } else if is(Key::Home) {
            self.go_to_page(0);
        } else if is(Key::End) {
            self.go_to_page(usize::MAX);
        } else {
            return false;
        }
        true
    }

    fn thumbs_key(&self, e: &slint::language::KeyEvent) -> bool {
        use slint::platform::Key;
        let t = e.text.as_str();
        let current = self.st.borrow().current_page;
        let is = |k: Key| t == SharedString::from(k).as_str();
        if is(Key::DownArrow) || is(Key::RightArrow) {
            self.go_to_page(current + 1);
        } else if is(Key::UpArrow) || is(Key::LeftArrow) {
            self.go_to_page(current.saturating_sub(1));
        } else {
            return false;
        }
        true
    }
}

/// Device-pixel render size for a page rect, capped to `MAX_PAGE_PIXELS`.
fn target_size(r: Rect, layout_scale: f32, scale_factor: f32) -> (u32, u32, f32) {
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

fn set_model<T: Clone + 'static>(model: &VecModel<T>, items: Vec<T>) {
    model.set_vec(items);
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn show_notices() {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let path = exe_dir.map(|d| d.join("THIRD-PARTY-NOTICES.txt"));
    match path.filter(|p| p.is_file()) {
        Some(p) => {
            let _ = std::process::Command::new("notepad.exe").arg(p).spawn();
        }
        None => {
            rfd::MessageDialog::new()
                .set_title("Third-party notices")
                .set_description("THIRD-PARTY-NOTICES.txt is missing from the installation folder.")
                .show();
        }
    }
}
