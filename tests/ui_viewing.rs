//! Headless UI tests for M2 viewing features, driven through the real window
//! controller and engine thread.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use paper_mirror::app::{self, AppContext, window::DocWindow};
use paper_mirror::core::geometry::{Point, Rect};
use paper_mirror::fixtures::{self, NEEDLE};
use paper_mirror::pdf::engine::Engine;
use paper_mirror::storage::settings::Settings;
use slint::{ComponentHandle, Model};

fn pump_until(timeout: Duration, cond: impl Fn() -> bool + 'static) -> bool {
    let start = Instant::now();
    let ok = Rc::new(RefCell::new(false));
    let ok2 = ok.clone();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(5), move || {
        if cond() {
            *ok2.borrow_mut() = true;
            let _ = slint::quit_event_loop();
        } else if start.elapsed() > timeout {
            let _ = slint::quit_event_loop();
        }
    });
    slint::run_event_loop_until_quit().unwrap();

    *ok.borrow()
}

fn settle() {
    pump_until(Duration::from_millis(250), || false);
}

fn open(path: PathBuf) -> Rc<DocWindow> {
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(1100.0, 800.0));
    w.open_path(path);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.has_document()), "document did not open");
    settle();
    w
}

#[test]
fn viewing_features() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = std::env::temp_dir().join(format!("pm-ui-view-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let many = dir.join("many.pdf");
    std::fs::write(&many, fixtures::many_pages(40)).unwrap();
    let outline_pdf = dir.join("outline.pdf");
    std::fs::write(&outline_pdf, fixtures::outline_document()).unwrap();
    let links_pdf = dir.join("links.pdf");
    std::fs::write(&links_pdf, fixtures::links_document()).unwrap();
    let settings_path = dir.join("settings.json");
    let engine = Engine::start();
    app::install_context(AppContext {
        engine: engine.engine.clone(),
        options: Default::default(),
        started: Instant::now(),
        settings: Rc::new(RefCell::new(Settings::default())),
        settings_path: settings_path.clone(),
        winit: false,
    });

    // --- Search across 40 pages ------------------------------------------------
    let w = open(many.clone());
    w.start_search(NEEDLE);
    assert_eq!(w.ui.get_sidebar_mode(), "search");
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(20), move || w2.search_finished()), "search did not finish");
    assert_eq!(w.search_hits(), 40);
    assert_eq!(w.ui.get_search_status(), "40 matches");
    assert_eq!(w.ui.get_search_rows().row_count(), 40);
    assert_eq!(w.current_hit(), Some(0), "first match is selected");
    settle();
    let overlays: Vec<_> = w.ui.get_overlays().iter().collect();
    assert!(overlays.iter().any(|o| o.kind == 1), "current match is highlighted");
    assert!(w.command("find-next"));
    assert_eq!(w.current_hit(), Some(1));
    settle();
    assert_eq!(w.ui.get_doc_subtitle(), "Page 2 of 40");
    assert!(w.command("find-prev"));
    assert!(w.command("find-prev"), "wraps around");
    assert_eq!(w.current_hit(), Some(39));
    settle();
    assert_eq!(w.ui.get_doc_subtitle(), "Page 40 of 40");
    w.ui.invoke_list_row_clicked("search".into(), 5);
    settle();
    assert_eq!(w.current_hit(), Some(5));
    assert_eq!(w.ui.get_doc_subtitle(), "Page 6 of 40");
    // A query with no matches.
    w.start_search("zebra crossing");
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(20), move || w2.search_finished()));
    assert_eq!(w.ui.get_search_status(), "No matches");
    // Clearing restores the thumbnails.
    w.start_search("");
    assert!(!w.ui.get_has_search());
    assert_eq!(w.ui.get_sidebar_mode(), "thumbnails");
    assert!(w.ui.get_overlays().iter().all(|o| o.kind > 1));

    // --- Bookmarks persist per file --------------------------------------------
    w.go_to_page(3);
    settle();
    assert!(w.command("toggle-bookmark"));
    assert!(w.ui.get_page_bookmarked());
    w.go_to_page(10);
    settle();
    assert!(w.command("toggle-bookmark"));
    assert_eq!(w.bookmarks(), vec![3, 10]);
    assert_eq!(Settings::load_from(&settings_path).bookmarks_for(&many), vec![3, 10]);
    assert!(w.command("sidebar-bookmarks"));
    assert_eq!(w.ui.get_bookmark_rows().row_count(), 2);
    w.ui.invoke_list_row_clicked("bookmarks".into(), 3);
    settle();
    assert_eq!(w.ui.get_doc_subtitle(), "Page 4 of 40");
    assert!(w.command("toggle-bookmark"), "removes the bookmark on the current page");
    assert_eq!(w.bookmarks(), vec![10]);
    assert!(w.command("sidebar-thumbnails"));

    // --- Contact sheet ---------------------------------------------------------
    assert!(w.command("contact-sheet"));
    assert!(w.ui.get_contact_sheet());
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.ui.get_sheet_items().iter().any(|i| i.has_image)));
    assert!(w.ui.get_sheet_items().row_count() >= 8, "a grid of pages is shown");
    w.ui.invoke_sheet_item_opened(7);
    settle();
    assert!(!w.ui.get_contact_sheet());
    assert_eq!(w.ui.get_doc_subtitle(), "Page 8 of 40");

    // --- Slideshow -------------------------------------------------------------
    assert!(w.command("slideshow"));
    assert_eq!(w.slideshow_page(), Some(7));
    w.slideshow_step(1);
    w.slideshow_step(1);
    assert_eq!(w.slideshow_page(), Some(9));
    w.slideshow_step(-1);
    assert_eq!(w.slideshow_page(), Some(8));
    w.slideshow_step(100);
    assert_eq!(w.slideshow_page(), None, "stepping past the end leaves the slideshow");
    settle();
    assert_eq!(w.ui.get_doc_subtitle(), "Page 9 of 40");

    // --- Rectangular selection and Zoom to Selection ---------------------------
    assert!(w.command("tool-rect"));
    assert_eq!(w.ui.get_tool(), "tool-rect");
    // Drag across the top-left quarter of the current page.
    let (start, end) = {
        let page_rect = w.ui.get_pages().iter().find(|p| p.index == 8).unwrap();
        (
            Point::new(page_rect.x + 40.0, page_rect.y + 40.0),
            Point::new(page_rect.x + page_rect.width / 3.0, page_rect.y + page_rect.height / 4.0),
        )
    };
    w.pointer(0, start, false, false, 0);
    w.pointer(1, Point::new(start.x + 20.0, start.y + 20.0), false, false, 0);
    w.pointer(1, end, false, false, 0);
    w.pointer(2, end, false, false, 0);
    let (page, sel) = w.rect_selection().expect("a rectangle is selected");
    assert_eq!(page, 8);
    assert!(sel.w > 50.0 && sel.h > 50.0, "{sel:?}");
    assert!(w.ui.get_has_rect_selection());
    assert!(w.ui.get_overlays().iter().any(|o| o.kind == 3));
    let before = w.ui.get_zoom_label();
    assert!(w.command("zoom-to-selection"));
    settle();
    assert_ne!(w.ui.get_zoom_label(), before);
    // The selection's centre is near the viewport centre.
    let ov = w.ui.get_overlays().iter().find(|o| o.kind == 3).unwrap();
    let cx = ov.x + ov.width / 2.0 - w.ui.get_scroll_x();
    let cy = ov.y + ov.height / 2.0 - w.ui.get_scroll_y();
    assert!((cx - w.ui.get_view_width() / 2.0).abs() < 30.0, "cx={cx}");
    assert!((cy - w.ui.get_view_height() / 2.0).abs() < 30.0, "cy={cy}");
    // A click without dragging clears it; Esc also works.
    w.set_rect_selection(8, Rect::new(10.0, 10.0, 100.0, 100.0));
    assert!(w.command("tool-text"));
    assert!(w.escape());
    assert!(w.rect_selection().is_none());

    // --- Inspector -------------------------------------------------------------
    assert!(w.command("inspector"));
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.ui.get_info_rows().row_count() > 5));
    let rows: Vec<(String, String)> =
        w.ui.get_info_rows().iter().map(|r| (r.key.to_string(), r.value.to_string())).collect();
    assert!(rows.contains(&("Pages".into(), "40".into())), "{rows:?}");
    assert!(rows.contains(&("Title".into(), "Paper Mirror fixture".into())));
    assert!(rows.contains(&("Encryption".into(), "None".into())));
    assert!(rows.iter().any(|(k, v)| k == "Page size" && v.starts_with("8.50 × 11.00 in")));
    assert!(w.command("inspector"));
    assert!(!w.ui.get_inspector_visible());

    // --- Table of contents -----------------------------------------------------
    let o = open(outline_pdf);
    let o2 = o.clone();
    assert!(pump_until(Duration::from_secs(5), move || o2.outline_len() == 4));
    assert!(o.command("sidebar-contents"));
    // Chapter 2 starts expanded (depth 0), so all four rows are visible.
    assert_eq!(o.ui.get_outline_rows().row_count(), 4);
    o.ui.invoke_list_row_clicked("contents".into(), 3);
    settle();
    assert_eq!(o.ui.get_doc_subtitle(), "Page 5 of 5");
    assert!(o.ui.get_outline_rows().iter().any(|r| r.selected && r.title == "Section 2.2"));
    o.ui.invoke_list_row_toggled("contents".into(), 1);
    assert_eq!(o.ui.get_outline_rows().row_count(), 2, "collapsing Chapter 2 hides its sections");

    // --- Links -----------------------------------------------------------------
    let l = open(links_pdf);
    l.command("zoom-actual");
    settle();
    let page0 = l.ui.get_pages().iter().find(|p| p.index == 0).unwrap();
    let scale = page0.width / 612.0;
    let to_view = |x: f32, pdf_y: f32| Point::new(page0.x + x * scale, page0.y + (792.0 - pdf_y) * scale);
    // Hovering over the internal link shows a pointing hand once link rects load.
    let l2 = l.clone();
    let p = to_view(100.0, 690.0);
    assert!(pump_until(Duration::from_secs(5), move || {
        l2.pointer(1, p, false, false, 0);
        l2.ui.get_canvas_cursor() == "pointer"
    }));
    l.pointer(0, p, false, false, 0);
    l.pointer(2, p, false, false, 0);
    let l2 = l.clone();
    assert!(pump_until(Duration::from_secs(5), move || l2.current_page() == 2), "internal link goes to page 3");
    l.go_to_page(0);
    settle();
    let page0 = l.ui.get_pages().iter().find(|p| p.index == 0).unwrap();
    let p = Point::new(page0.x + 100.0 * scale, page0.y + (792.0 - 670.0) * scale);
    l.pointer(0, p, false, false, 0);
    l.pointer(2, p, false, false, 0);
    let l2 = l.clone();
    assert!(
        pump_until(Duration::from_secs(5), move || l2.last_opened_link.borrow().is_some()),
        "external link is offered"
    );
    assert_eq!(l.last_opened_link.borrow().as_deref(), Some("https://example.com/"));
}
