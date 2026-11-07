//! A document window: binds the Slint `MainWindow` to one document and routes
//! commands. Feature logic lives in the submodules:
//! - `view`: layout, virtualised rendering, thumbnails, navigation, zoom
//! - `open`: file dialogs, type detection, passwords, recents
//! - `sidebar`: outline, bookmarks, search
//! - `tools`: pointer input, rectangular selection, loupe, links
//! - `sheet`: contact sheet and slideshow
//! - `inspector`: document information
//! - `markup`: annotations, styles, text editing, undo
//! - `saving`: save, export, unsaved-changes prompts, originals

mod exporting;
mod forms;
mod imaging;
mod inspector;
mod markup;
mod model;
mod open;
mod organize;
mod pages;
mod platform;
mod saving;
mod selection;
mod sheet;
mod sidebar;
mod signature;
mod tools;
mod view;

pub use markup::{MarkupKind, ShapeKind};
pub use saving::AfterSave;
pub use signature::trace_signature_image;
pub use view::file_name;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::core::annot::{Annotation, Color, Style};
use crate::core::cache::ByteLru;
use crate::core::commands;
use crate::core::geometry::{Point, Rect, Size};
use crate::core::layout::{DisplayMode, Layout, ZoomMode};
use crate::pdf::engine::{DocId, Engine, GenerationCounter, Priority};
use crate::pdf::outline::OutlineItem;
use crate::pdf::text::SearchHit;
use crate::ui::{
    AnnotPrim, FormFieldView, InfoRow, ListRow, MainWindow, OverlayRect, PageItem, SheetItem, ShortcutRow, ThumbItem,
};

const PAGE_CACHE_BYTES: usize = 160 * 1024 * 1024;
const THUMB_CACHE_BYTES: usize = 24 * 1024 * 1024;
const SHEET_CACHE_BYTES: usize = 32 * 1024 * 1024;

static NEXT_WINDOW_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct OpenDoc {
    pub id: DocId,
    pub path: Option<PathBuf>,
    pub title: String,
    pub sizes: Vec<Size>,
}

pub(crate) struct PageImage {
    pub width_px: u32,
    pub image: slint::Image,
}

/// Identity of a detail tile: (scale bits, x offset, y offset, width, height).
pub(crate) type TileKey = (u32, i32, i32, u32, u32);

pub(crate) struct Tile {
    pub page: usize,
    pub key: TileKey,
    pub view: Rect,
    pub image: slint::Image,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Text,
    Rect,
    Sketch,
    Draw,
    Shape(ShapeKind),
    TextBox,
    Highlight(MarkupKind),
    Signature,
    Note,
    Redact,
    /// Instant Alpha: drag from a colour to make similar connected pixels transparent.
    InstantAlpha,
}

impl Tool {
    /// Tools that select text by dragging.
    pub fn selects_text(self) -> bool {
        matches!(self, Tool::Text | Tool::Highlight(_))
    }
}

/// One undo step: the annotation list and, for page operations, the document bytes.
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub annots: Vec<Annotation>,
    pub pdf: Option<std::sync::Arc<Vec<u8>>>,
    /// Image resolution at the time (Adjust Size changes it).
    pub dpi: Option<(f32, f32)>,
}

#[derive(Default)]
pub(crate) struct SearchState {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub current: Option<usize>,
    pub pages_done: usize,
    pub finished: bool,
}

/// An in-progress pointer drag on the canvas.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drag {
    pub page: usize,
    pub start: Point,
    pub moved: bool,
}

#[derive(Default)]
pub(crate) struct ViewState {
    pub doc: Option<OpenDoc>,
    pub zoom_mode: ZoomMode,
    pub zoom: f32,
    pub mode: DisplayMode,
    pub layout: Layout,
    pub last_viewport: Size,
    pub current_page: usize,
    pub selected: Vec<usize>,
    pub page_cache: Option<ByteLru<usize, PageImage>>,
    pub thumb_cache: Option<ByteLru<usize, slint::Image>>,
    pub sheet_cache: Option<ByteLru<usize, slint::Image>>,
    pub pending_pages: HashSet<(usize, u32)>,
    pub pending_thumbs: HashSet<usize>,
    pub pending_sheet: HashSet<usize>,
    pub tile: Option<Tile>,
    pub pending_tile: Option<(usize, TileKey)>,
    pub thumb_rects: Vec<Rect>,
    pub first_render_logged: bool,
    /// File waiting for a password, with the number of failed attempts.
    pub password_for: Option<(PathBuf, u32)>,
    pub outline: Vec<OutlineItem>,
    pub outline_collapsed: HashSet<usize>,
    pub bookmarks: Vec<usize>,
    pub search: SearchState,
    pub tool: Tool,
    pub drag: Option<Drag>,
    /// Rectangular selection: page index and rectangle in page display space.
    pub rect_selection: Option<(usize, Rect)>,
    pub loupe_on: bool,
    /// Link rectangles per page (page display space), fetched lazily.
    pub links: HashMap<usize, Vec<Rect>>,
    pub pending_links: HashSet<usize>,
    pub link_click: Option<(usize, Point)>,
    /// Page the sidebar lists were last highlighted for.
    pub list_page: Option<usize>,
    /// Character geometry per page for text selection, fetched with the links.
    pub page_text: HashMap<usize, Rc<crate::core::textsel::PageText>>,
    pub pending_text: HashSet<usize>,
    pub text_sel: Option<crate::core::textsel::TextSelection>,
    /// Where the current text drag started.
    pub sel_anchor: Option<crate::core::textsel::Caret>,
    pub annots: Vec<Annotation>,
    pub next_annot_id: u64,
    pub selected_annot: Option<u64>,
    pub annot_drag: Option<markup::AnnotDrag>,
    pub creating: Option<markup::Creating>,
    pub editing: Option<u64>,
    pub style: Style,
    pub highlight_color: Color,
    pub undo: Vec<Snapshot>,
    pub redo: Vec<Snapshot>,
    pub dirty: bool,
    pub lens_cache: HashMap<(u64, u32), slint::Image>,
    /// The original file has been backed up for this session (first in-place save).
    pub original_saved: bool,
    pub fields: Vec<crate::pdf::forms::Field>,
    /// Anchor for Shift-click range selection of thumbnails.
    pub page_anchor: usize,
    pub thumb_drag: Option<organize::ThumbDrag>,
    /// Scroll position at the last view update (to tell navigation from scrolling).
    pub last_scroll_y: Option<f32>,
    /// Set when the document is an image (ADR 0007).
    pub image: Option<imaging::ImageMeta>,
    pub alpha: Option<imaging::AlphaDrag>,
    pub size_model: Option<crate::core::imagesize::SizeModel>,
    /// Format of the image save in flight.
    pub pending_image_format: Option<crate::images::encode::SaveFormat>,
    /// Resolution before an Adjust Size that resamples (for its undo step).
    pub pending_dpi_undo: Option<(f32, f32)>,
    /// Formats listed in the Export panel (None = PDF).
    pub export_formats: Vec<Option<crate::images::encode::SaveFormat>>,
    /// A copy being written for File > Share.
    pub share_after_save: Option<PathBuf>,
    /// The Print dialog is open (on its own thread).
    pub printing: bool,
    /// The open 3D model (ADR 0009), instead of a PDF or image.
    pub model: Option<model::ModelState>,
    pub model_path: Option<PathBuf>,
    /// Export choices while the Export panel shows a model.
    pub model_export: Vec<model::ModelChoice>,
}

pub struct DocWindow {
    pub id: u64,
    pub ui: MainWindow,
    pub(crate) engine: Engine,
    pub(crate) st: RefCell<ViewState>,
    pub(crate) pages_model: Rc<VecModel<PageItem>>,
    pub(crate) thumbs_model: Rc<VecModel<ThumbItem>>,
    pub(crate) overlays_model: Rc<VecModel<OverlayRect>>,
    pub(crate) outline_model: Rc<VecModel<ListRow>>,
    pub(crate) bookmarks_model: Rc<VecModel<ListRow>>,
    pub(crate) search_model: Rc<VecModel<ListRow>>,
    pub(crate) sheet_model: Rc<VecModel<SheetItem>>,
    pub(crate) info_model: Rc<VecModel<InfoRow>>,
    pub(crate) annot_model: Rc<VecModel<AnnotPrim>>,
    pub(crate) notes_model: Rc<VecModel<ListRow>>,
    pub(crate) forms_model: Rc<VecModel<FormFieldView>>,
    pub(crate) sig: RefCell<signature::SignatureState>,
    pub(crate) render_gen: GenerationCounter,
    pub(crate) thumb_gen: GenerationCounter,
    pub(crate) search_gen: GenerationCounter,
    pub(crate) loupe_gen: GenerationCounter,
    pub(crate) tile_timer: slint::Timer,
    pub(crate) slideshow: RefCell<Option<sheet::Slideshow>>,
    /// Last error shown to the user (kept for tests and diagnostics).
    pub last_error: RefCell<Option<String>>,
    /// Last external link the user chose to open (kept for tests).
    pub last_opened_link: RefCell<Option<String>>,
    /// Last text copied (kept for tests; the test backend never touches the clipboard).
    pub last_copied: RefCell<Option<String>>,
    /// Last file written by a save (kept for tests).
    pub last_saved: RefCell<Option<PathBuf>>,
    /// Last print result (kept for tests).
    pub last_printed: RefCell<Option<Result<usize, String>>>,
    /// Last file handed to the Share sheet (kept for tests).
    pub last_shared: RefCell<Option<PathBuf>>,
    /// Password the document was opened with, kept in memory only so a re-save stays
    /// protected. Never written to disk or logs.
    pub(crate) open_password: RefCell<Option<String>>,
}

pub fn display_mode_from(s: &str) -> DisplayMode {
    match s {
        "single" => DisplayMode::SinglePage,
        "two-pages" => DisplayMode::TwoPages,
        _ => DisplayMode::Continuous,
    }
}

pub fn display_mode_name(m: DisplayMode) -> &'static str {
    match m {
        DisplayMode::Continuous => "continuous",
        DisplayMode::SinglePage => "single",
        DisplayMode::TwoPages => "two-pages",
    }
}

impl DocWindow {
    /// Creates and shows a window, optionally opening a file in it.
    pub fn open_new(path: Option<PathBuf>) -> Result<Rc<DocWindow>, String> {
        let ui = MainWindow::new().map_err(|e| e.to_string())?;
        let ctx = super::context();
        let (mode, sidebar_visible, sidebar_width) = {
            let s = ctx.settings.borrow();
            (display_mode_from(&s.display_mode), s.sidebar_visible, s.sidebar_width)
        };
        let w = Rc::new(DocWindow {
            id: NEXT_WINDOW_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ui,
            engine: ctx.engine.clone(),
            st: RefCell::new(ViewState {
                zoom: 1.0,
                mode,
                page_cache: Some(ByteLru::new(PAGE_CACHE_BYTES)),
                thumb_cache: Some(ByteLru::new(THUMB_CACHE_BYTES)),
                sheet_cache: Some(ByteLru::new(SHEET_CACHE_BYTES)),
                highlight_color: Color::HIGHLIGHT_YELLOW,
                ..Default::default()
            }),
            pages_model: Rc::new(VecModel::default()),
            thumbs_model: Rc::new(VecModel::default()),
            overlays_model: Rc::new(VecModel::default()),
            outline_model: Rc::new(VecModel::default()),
            bookmarks_model: Rc::new(VecModel::default()),
            search_model: Rc::new(VecModel::default()),
            sheet_model: Rc::new(VecModel::default()),
            info_model: Rc::new(VecModel::default()),
            annot_model: Rc::new(VecModel::default()),
            notes_model: Rc::new(VecModel::default()),
            forms_model: Rc::new(VecModel::default()),
            sig: RefCell::new(Default::default()),
            render_gen: GenerationCounter::default(),
            thumb_gen: GenerationCounter::default(),
            search_gen: GenerationCounter::default(),
            loupe_gen: GenerationCounter::default(),
            tile_timer: slint::Timer::default(),
            slideshow: RefCell::new(None),
            last_error: RefCell::new(None),
            last_opened_link: RefCell::new(None),
            last_copied: RefCell::new(None),
            last_saved: RefCell::new(None),
            last_printed: RefCell::new(None),
            last_shared: RefCell::new(None),
            open_password: RefCell::new(None),
        });
        w.ui.set_pages(ModelRc::from(w.pages_model.clone()));
        w.ui.set_thumbs(ModelRc::from(w.thumbs_model.clone()));
        w.ui.set_overlays(ModelRc::from(w.overlays_model.clone()));
        w.ui.set_outline_rows(ModelRc::from(w.outline_model.clone()));
        w.ui.set_bookmark_rows(ModelRc::from(w.bookmarks_model.clone()));
        w.ui.set_search_rows(ModelRc::from(w.search_model.clone()));
        w.ui.set_sheet_items(ModelRc::from(w.sheet_model.clone()));
        w.ui.set_info_rows(ModelRc::from(w.info_model.clone()));
        w.ui.set_annot_prims(ModelRc::from(w.annot_model.clone()));
        w.ui.set_note_rows(ModelRc::from(w.notes_model.clone()));
        w.ui.set_form_fields(ModelRc::from(w.forms_model.clone()));
        w.init_markup_ui();
        w.init_export_ui();
        w.ui.set_app_version(crate::VERSION.into());
        w.ui.set_display_mode(display_mode_name(mode).into());
        w.ui.set_sidebar_visible(sidebar_visible);
        w.ui.set_sidebar_width(sidebar_width);
        let rows: Vec<ShortcutRow> = commands::shortcut_rows()
            .into_iter()
            .map(|(menu, title, keys)| ShortcutRow { menu: menu.into(), title: title.into(), keys: keys.into() })
            .collect();
        w.ui.set_shortcut_rows(ModelRc::new(VecModel::from(rows)));
        w.bind();
        super::register_window(w.clone());
        w.ui.show().map_err(|e| e.to_string())?;
        super::refresh_window_menus();
        if let Some(p) = path {
            w.open_path(p);
        }
        Ok(w)
    }

    fn bind(self: &Rc<Self>) {
        let id = self.id;
        self.ui.on_command(move |cmd| {
            super::with_window(id, |w| {
                w.command(cmd.as_str());
            })
        });
        self.ui.on_canvas_viewport_changed(move || super::with_window(id, |w| w.viewport_changed()));
        self.ui.on_thumbs_viewport_changed(move || super::with_window(id, |w| w.update_thumbs()));
        self.ui.on_canvas_ctrl_wheel(move |delta, x, y| super::with_window(id, |w| w.wheel_zoom(delta, x, y)));
        self.ui.on_canvas_pointer(move |kind, x, y, shift, ctrl, button| {
            super::with_window(id, |w| w.pointer(kind, Point::new(x, y), shift, ctrl, button))
        });
        self.ui.on_thumb_clicked(move |i, ctrl, shift| {
            super::with_window(id, |w| w.thumb_clicked(i as usize, ctrl, shift))
        });
        self.ui.on_thumb_drag(move |i, y| super::with_window(id, |w| w.thumb_drag(i as usize, y)));
        self.ui.on_thumb_drop(move || super::with_window(id, |w| w.thumb_drop()));
        self.ui.on_export_confirm(move || super::with_window(id, |w| w.export_confirm()));
        self.ui.on_export_format_changed(move || super::with_window(id, |w| w.export_format_changed()));
        self.ui.on_size_edited(move |f| super::with_window(id, |w| w.size_edited(f.as_str())));
        self.ui.on_size_confirm(move || super::with_window(id, |w| w.size_confirm()));
        self.ui.on_size_cancel(move || super::with_window(id, |w| w.size_cancel()));
        self.ui.on_adjust_changed(move || super::with_window(id, |w| w.adjust_changed()));
        self.ui.on_adjust_auto(move || super::with_window(id, |w| w.adjust_auto_levels()));
        self.ui.on_adjust_reset(move || super::with_window(id, |w| w.adjust_reset()));
        self.ui.on_adjust_done(move || super::with_window(id, |w| w.adjust_done()));
        self.ui.on_open_recent(move |i| super::with_window(id, |w| w.open_recent(i as usize)));
        self.ui.on_activate_window(move |i| super::activate_window_at(i as usize));
        self.ui.on_prompt_accepted(move |kind, value| {
            super::with_window(id, |w| w.prompt_accepted(kind.as_str(), value.as_str()))
        });
        self.ui.on_prompt_cancelled(move |kind| super::with_window(id, |w| w.prompt_cancelled(kind.as_str())));
        self.ui.on_sidebar_resized(move || {
            super::with_window(id, |w| {
                let width = w.ui.get_sidebar_width();
                super::update_settings(|s| s.sidebar_width = width);
                w.update_thumbs();
            })
        });
        self.ui.on_sidebar_mode_selected(move |m| super::with_window(id, |w| w.set_sidebar_mode(m.as_str())));
        self.ui
            .on_list_row_clicked(move |mode, row| super::with_window(id, |w| w.list_row_clicked(mode.as_str(), row)));
        self.ui
            .on_list_row_toggled(move |mode, row| super::with_window(id, |w| w.list_row_toggled(mode.as_str(), row)));
        self.ui.on_search_submitted(move |t| super::with_window(id, |w| w.start_search(t.as_str())));
        self.ui.on_search_edited(move |t| {
            super::with_window(id, |w| {
                if t.is_empty() {
                    w.start_search("");
                }
            })
        });
        self.ui.on_edit_finished(move |t| super::with_window(id, |w| w.finish_edit(t.as_str())));
        self.ui.on_markup_set_stroke(move |c| super::with_window(id, |w| w.set_stroke_color(markup::from_scolor(c))));
        self.ui.on_markup_set_fill(move |c, none| {
            super::with_window(id, |w| w.set_fill_color((!none).then(|| markup::from_scolor(c))))
        });
        self.ui.on_markup_set_line(move |width, dashed| super::with_window(id, |w| w.set_line_style(width, dashed)));
        self.ui.on_markup_set_text_style(move |font, size, color, align| {
            super::with_window(id, |w| w.set_text_style(font.as_str(), size, markup::from_scolor(color), align))
        });
        self.ui.on_field_commit_text(move |i, t| super::with_window(id, |w| w.field_commit_text(i, t.as_str())));
        self.ui.on_field_toggle(move |i| super::with_window(id, |w| w.field_toggle(i)));
        self.ui.on_field_choose(move |i, o| super::with_window(id, |w| w.field_choose(i, o)));
        self.ui.on_field_sign(move |i| super::with_window(id, |w| w.field_sign(i)));
        self.ui.on_signature_chosen(move |sid| super::with_window(id, |w| w.signature_chosen(sid as u64)));
        self.ui.on_signature_delete(move |sid| super::with_window(id, |w| w.signature_delete(sid as u64)));
        self.ui.on_signature_start_drawing(move || super::with_window(id, |w| w.signature_start_drawing()));
        self.ui.on_signature_import(move || super::with_window(id, |w| w.signature_import()));
        self.ui.on_pad_pointer(move |k, x, y| super::with_window(id, |w| w.pad_pointer(k, x, y)));
        self.ui.on_pad_clear(move || super::with_window(id, |w| w.pad_clear()));
        self.ui.on_pad_save(move || super::with_window(id, |w| w.pad_save()));
        self.ui.on_signatures_closed(move || super::with_window(id, |w| w.signatures_closed()));
        self.ui.on_sheet_viewport_changed(move || super::with_window(id, |w| w.update_sheet()));
        self.ui.on_sheet_item_clicked(move |i| super::with_window(id, |w| w.sheet_select(i as usize)));
        self.ui.on_sheet_item_opened(move |i| super::with_window(id, |w| w.sheet_open(i as usize)));
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
            // Unsaved changes are resolved first; the window closes only if allowed.
            let mut close = true;
            super::with_window(id, |w| close = w.confirm_close());
            if !close {
                return slint::CloseRequestResponse::KeepWindowShown;
            }
            // Defer removal: the window must not be dropped inside its own callback.
            slint::Timer::single_shot(Duration::ZERO, move || super::unregister_window(id));
            slint::CloseRequestResponse::HideWindow
        });
        if super::context().winit {
            use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
            self.ui.window().on_winit_window_event(move |_w, ev| {
                if let winit::event::WindowEvent::DroppedFile(path) = ev {
                    let path = path.clone();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        super::with_window(id, |w| w.open_here_or_new(path));
                    });
                    return EventResult::PreventDefault;
                }
                EventResult::Propagate
            });
        }
    }

    /// Dispatches a registered command. Returns false for unknown ids.
    pub fn command(&self, cmd: &str) -> bool {
        if self.is_model() && self.model_command(cmd) {
            return true;
        }
        match cmd {
            "open" => self.choose_and_open(),
            "clear-recent" => {
                super::update_settings(|s| s.recent.clear());
                super::refresh_window_menus();
            }
            "close" => {
                if self.confirm_close() {
                    self.close();
                }
            }
            "show-in-explorer" => {
                if let Some(p) = self.doc_path() {
                    crate::platform_windows::shell::reveal_in_explorer(&p);
                }
            }
            "print" => self.print(),
            "share" => self.share(),
            "register-file-types" => self.register_file_types(),
            "unregister-file-types" => self.unregister_file_types(),
            "save" => self.save(false),
            "save-as" => self.save(true),
            "export-pdf" => self.show_export(true),
            "export" => self.show_export(false),
            "new-from-clipboard" => self.new_from_clipboard(),
            "flip-horizontal" => self.flip(true),
            "flip-vertical" => self.flip(false),
            "adjust-size" => self.show_adjust_size(),
            "adjust-color" => self.show_adjust_color(),
            "instant-alpha" => {
                if self.is_image() {
                    self.set_tool(Tool::InstantAlpha);
                }
            }
            "show-location" => self.show_location(),
            "remove-location" => self.remove_location(),
            "flip-up-axis" => {}
            "duplicate" => self.duplicate(),
            "revert-saved" => self.revert_to_saved(),
            "revert-original" => self.revert_to_original(),
            "lock" => self.toggle_lock(),
            "rotate-left" => self.rotate_pages(-1),
            "rotate-right" => self.rotate_pages(1),
            "delete-pages" => self.delete_pages(),
            "insert-blank" => self.insert_blank_page(),
            "insert-file" => self.insert_from_file(None),
            "crop" => self.crop_to_selection(),
            "undo" => self.undo(),
            "redo" => self.redo(),
            "delete" => {
                self.delete_selected_annotation();
            }
            "copy" => self.copy(),
            "select-all" => self.select_all(),
            "find" => {
                self.ui.invoke_focus_search();
            }
            "find-next" => self.step_search(1),
            "find-prev" => self.step_search(-1),
            "content-only" => self.set_sidebar_visible(false),
            "toggle-sidebar" => self.set_sidebar_visible(!self.ui.get_sidebar_visible()),
            "sidebar-thumbnails" => self.set_sidebar_mode("thumbnails"),
            "sidebar-contents" => self.set_sidebar_mode("contents"),
            "sidebar-bookmarks" => self.set_sidebar_mode("bookmarks"),
            "contact-sheet" => self.toggle_contact_sheet(),
            "view-continuous" => self.set_display_mode(DisplayMode::Continuous),
            "view-single" => self.set_display_mode(DisplayMode::SinglePage),
            "view-two-pages" => self.set_display_mode(DisplayMode::TwoPages),
            "zoom-in" => self.zoom_step(true),
            "zoom-out" => self.zoom_step(false),
            "zoom-actual" => self.set_zoom_mode(ZoomMode::Custom(1.0), None),
            "zoom-fit-page" => self.set_zoom_mode(ZoomMode::FitPage, None),
            "zoom-fit-width" => self.set_zoom_mode(ZoomMode::FitWidth, None),
            "zoom-to-selection" => self.zoom_to_selection(),
            "slideshow" => self.start_slideshow(),
            "appearance-system" | "appearance-light" | "appearance-dark" => {
                let a = cmd.trim_start_matches("appearance-").to_string();
                super::update_settings(|s| s.appearance = a.clone());
                super::apply_appearance();
            }
            "page-prev" => self.step_page(-1),
            "page-next" => self.step_page(1),
            "page-first" => self.go_to_page(0),
            "page-last" => self.go_to_page(usize::MAX),
            "go-to-page" => self.show_prompt("go-to-page", "Go to Page", &self.page_range_text(), "", false, "Go"),
            "toggle-bookmark" => self.toggle_bookmark(),
            "tool-text" => self.set_tool(Tool::Text),
            "tool-rect" => self.set_tool(Tool::Rect),
            "tool-sketch" => self.set_tool(Tool::Sketch),
            "tool-draw" => self.set_tool(Tool::Draw),
            "tool-textbox" => self.set_tool(Tool::TextBox),
            "tool-note" => self.set_tool(Tool::Note),
            "tool-signature" => self.open_signatures(),
            "tool-redact" => {
                if !self.redact_selection() {
                    self.set_tool(Tool::Redact);
                }
            }
            "tool-highlight" | "highlight" | "underline" | "strikethrough" => {
                let kind = match cmd {
                    "underline" => MarkupKind::Underline,
                    "strikethrough" => MarkupKind::StrikeOut,
                    _ => MarkupKind::Highlight,
                };
                // With text selected, mark it up now; otherwise enter the mode.
                if !self.apply_markup(kind) {
                    self.set_tool(Tool::Highlight(kind));
                }
            }
            "shape-line" => self.select_shape_tool(ShapeKind::Line),
            "shape-arrow" => self.select_shape_tool(ShapeKind::Arrow),
            "shape-rect" => self.select_shape_tool(ShapeKind::Rect),
            "shape-rounded" => self.select_shape_tool(ShapeKind::Rounded),
            "shape-oval" => self.select_shape_tool(ShapeKind::Oval),
            "shape-bubble" => self.select_shape_tool(ShapeKind::Bubble),
            "shape-star" => self.select_shape_tool(ShapeKind::Star),
            "shape-lens" => self.select_shape_tool(ShapeKind::Lens),
            "toggle-markup" => self.ui.set_markup_visible(!self.ui.get_markup_visible()),
            "sidebar-notes" => self.set_sidebar_mode("notes"),
            "toggle-loupe" => self.toggle_loupe(),
            "inspector" => self.toggle_inspector(),
            "minimize" => self.ui.window().set_minimized(true),
            "maximize" => {
                let m = self.ui.window().is_maximized();
                self.ui.window().set_maximized(!m);
            }
            "shortcuts" => self.ui.set_shortcuts_visible(true),
            "about" => self.ui.invoke_show_about(),
            "show-notices" => crate::platform_windows::shell::show_notices(),
            _ => {
                debug_assert!(commands::find(cmd).is_none(), "registered command {cmd} has no handler");
                return false;
            }
        }
        true
    }

    /// Commands that act differently on a 3D model. Returns false to fall through.
    fn model_command(&self, cmd: &str) -> bool {
        match cmd {
            "zoom-in" => self.model_zoom(0.8),
            "zoom-out" => self.model_zoom(1.25),
            "zoom-actual" | "zoom-fit-page" | "zoom-fit-width" => self.model_reset_view(),
            "rotate-left" => self.model_edit(|m| m.rotate_quarter(-1)),
            "rotate-right" => self.model_edit(|m| m.rotate_quarter(1)),
            "flip-up-axis" => self.model_edit(|m| m.flip_up_axis()),
            "save" => self.save_model(false),
            "save-as" => self.save_model(true),
            "export" | "export-pdf" => self.show_export(false),
            "print" => self.error(
                "3D models can’t be printed directly.",
                "Choose File > Export… and export an image of the view, then print the image.",
            ),
            "share" => match (self.doc_path(), self.is_dirty()) {
                (Some(p), false) => self.share_now(&p),
                _ => self.error("Save the model first.", "Sharing sends the saved file."),
            },
            _ => return false,
        }
        true
    }

    pub fn close(&self) {
        let id = self.id;
        self.end_slideshow();
        self.release_document();
        let _ = self.ui.hide();
        slint::Timer::single_shot(Duration::ZERO, move || super::unregister_window(id));
    }

    /// Closes the engine-side document and drops every cached bitmap and per-document
    /// view state.
    pub fn release_document(&self) {
        let mut st = self.st.borrow_mut();
        if let Some(old) = st.doc.take() {
            self.engine.submit(Priority::Interactive, None, move |s| {
                s.docs.remove(&old.id);
            });
        }
        st.page_cache.as_mut().unwrap().clear();
        st.thumb_cache.as_mut().unwrap().clear();
        st.sheet_cache.as_mut().unwrap().clear();
        st.pending_pages.clear();
        st.pending_thumbs.clear();
        st.pending_sheet.clear();
        st.tile = None;
        st.pending_tile = None;
        st.outline.clear();
        st.outline_collapsed.clear();
        st.bookmarks.clear();
        st.search = SearchState::default();
        st.drag = None;
        st.rect_selection = None;
        st.links.clear();
        st.pending_links.clear();
        st.list_page = None;
        st.page_text.clear();
        st.pending_text.clear();
        st.text_sel = None;
        st.annots.clear();
        st.selected_annot = None;
        st.annot_drag = None;
        st.creating = None;
        st.editing = None;
        st.undo.clear();
        st.redo.clear();
        st.dirty = false;
        st.lens_cache.clear();
        st.original_saved = false;
        st.fields.clear();
        st.thumb_drag = None;
        st.model = None;
        st.model_path = None;
        st.last_scroll_y = None;
        st.image = None;
        st.alpha = None;
        st.size_model = None;
        st.pending_dpi_undo = None;
        drop(st);
        self.render_gen.bump();
        self.thumb_gen.bump();
        self.search_gen.bump();
        self.loupe_gen.bump();
        for m in [&self.outline_model, &self.bookmarks_model, &self.search_model] {
            m.set_vec(Vec::new());
        }
        self.pages_model.set_vec(Vec::new());
        self.thumbs_model.set_vec(Vec::new());
        self.overlays_model.set_vec(Vec::new());
        self.sheet_model.set_vec(Vec::new());
        self.info_model.set_vec(Vec::new());
        self.ui.set_has_search(false);
        self.ui.set_search_status(SharedString::new());
        self.ui.set_has_rect_selection(false);
        self.ui.set_has_text_selection(false);
        self.ui.set_editing(false);
        self.ui.set_annot_selected(false);
        self.ui.set_edited(false);
        *self.open_password.borrow_mut() = None;
        self.annot_model.set_vec(Vec::new());
        self.notes_model.set_vec(Vec::new());
        self.forms_model.set_vec(Vec::new());
        self.sync_undo_ui();
        self.ui.set_contact_sheet(false);
        self.ui.set_is_image(false);
        self.ui.set_is_model(false);
        self.ui.set_doc_subtitle(SharedString::new());
        self.ui.set_mask_visible(false);
        self.ui.set_adjust_visible(false);
        self.ui.set_size_visible(false);
        self.sync_location_ui();
    }

    pub fn doc_path(&self) -> Option<PathBuf> {
        let st = self.st.borrow();
        st.doc.as_ref().and_then(|d| d.path.clone()).or_else(|| st.model_path.clone())
    }

    pub fn title(&self) -> String {
        let st = self.st.borrow();
        st.doc
            .as_ref()
            .map(|d| d.title.clone())
            .or_else(|| st.model_path.as_deref().map(file_name))
            .unwrap_or_else(|| "Untitled".into())
    }

    pub fn has_document(&self) -> bool {
        let st = self.st.borrow();
        st.doc.is_some() || st.model.is_some()
    }

    pub fn current_page(&self) -> usize {
        self.st.borrow().current_page
    }

    fn set_sidebar_visible(&self, v: bool) {
        self.ui.set_sidebar_visible(v);
        super::update_settings(|s| s.sidebar_visible = v);
        self.update_thumbs();
    }

    fn set_display_mode(&self, mode: DisplayMode) {
        self.st.borrow_mut().mode = mode;
        self.ui.set_display_mode(display_mode_name(mode).into());
        super::update_settings(|s| s.display_mode = display_mode_name(mode).into());
        self.relayout(None);
        let page = self.st.borrow().current_page;
        self.go_to_page(page);
    }

    fn page_range_text(&self) -> String {
        let n = self.st.borrow().doc.as_ref().map(|d| d.sizes.len()).unwrap_or(0);
        format!("Enter a page number from 1 to {n}.")
    }

    pub(crate) fn show_prompt(
        &self,
        kind: &str,
        heading: &str,
        message: &str,
        error: &str,
        password: bool,
        accept: &str,
    ) {
        self.ui.set_prompt_heading(heading.into());
        self.ui.set_prompt_message(message.into());
        self.ui.set_prompt_error(error.into());
        self.ui.set_prompt_password(password);
        self.ui.set_prompt_accept_label(accept.into());
        self.ui.set_prompt_value(SharedString::new());
        self.ui.set_prompt_kind(kind.into());
    }

    fn hide_prompt(&self) {
        self.ui.set_prompt_kind(SharedString::new());
        self.ui.set_prompt_value(SharedString::new());
        self.ui.invoke_focus_canvas();
    }

    fn prompt_accepted(&self, kind: &str, value: &str) {
        match kind {
            "go-to-page" => {
                let n = self.st.borrow().doc.as_ref().map(|d| d.sizes.len()).unwrap_or(0);
                match value.trim().parse::<usize>() {
                    Ok(p) if p >= 1 && p <= n => {
                        self.hide_prompt();
                        self.go_to_page(p - 1);
                    }
                    _ => self.ui.set_prompt_error(format!("“{}” is not a page in this document.", value.trim()).into()),
                }
            }
            "password" => {
                let pending = self.st.borrow_mut().password_for.take();
                self.hide_prompt();
                if let Some((path, attempts)) = pending {
                    self.open_pdf(path, Some(value.to_string()), attempts);
                }
            }
            _ => self.hide_prompt(),
        }
    }

    fn prompt_cancelled(&self, kind: &str) {
        if kind == "password" {
            self.st.borrow_mut().password_for = None;
        }
        self.hide_prompt();
    }

    pub(crate) fn error(&self, title: &str, detail: &str) {
        *self.last_error.borrow_mut() = Some(format!("{title} {detail}"));
        if !super::context().winit {
            return;
        }
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Paper Mirror")
            .set_description(format!("{title}\n\n{detail}"))
            .set_parent(&self.ui.window().window_handle())
            .show();
    }

    /// Asks a yes/no question. Under the test backend the answer is always yes.
    pub(crate) fn confirm(&self, title: &str, detail: &str) -> bool {
        if !super::context().winit {
            return true;
        }
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title(title)
            .set_description(detail)
            .set_buttons(rfd::MessageButtons::YesNo)
            .set_parent(&self.ui.window().window_handle())
            .show()
            == rfd::MessageDialogResult::Yes
    }
}
