//! Headless UI tests for M6: page organisation, export, duplicate and lock.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use paper_mirror::app::window::{AfterSave, DocWindow};
use paper_mirror::app::{self, AppContext};
use paper_mirror::core::annot::{Annotation, Shape, Style};
use paper_mirror::core::geometry::Rect;
use paper_mirror::fixtures;
use paper_mirror::pdf::annot_io::SaveOptions;
use paper_mirror::pdf::document::PdfDoc;
use paper_mirror::pdf::effects::Effect;
use paper_mirror::pdf::engine::Engine;
use paper_mirror::pdf::ffi::{OpenError, Source};
use paper_mirror::pdf::security::Protection;
use paper_mirror::storage::settings::Settings;
use slint::ComponentHandle;

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

fn wait_pages(w: &Rc<DocWindow>, n: usize) {
    let w2 = w.clone();
    assert!(
        pump_until(Duration::from_secs(10), move || w2.page_count() == n),
        "expected {n} pages, have {}",
        w.page_count()
    );
}

fn wait_saved(w: &Rc<DocWindow>) {
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.last_saved.borrow().is_some()));
}

fn rect_on(w: &DocWindow) -> Annotation {
    w.annotations().into_iter().find(|a| matches!(a.shape, Shape::Rectangle)).unwrap()
}

#[test]
fn page_organisation_end_to_end() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = std::env::temp_dir().join(format!("pm-ui-pages-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: single test in this binary; set before any data-dir use.
    unsafe { std::env::set_var("PAPER_MIRROR_DATA_DIR", dir.join("data")) };
    let pdf = dir.join("six.pdf");
    std::fs::write(&pdf, fixtures::many_pages(6)).unwrap();
    let extra = dir.join("extra.pdf");
    std::fs::write(&extra, fixtures::text_document()).unwrap();
    let engine = Engine::start();
    app::install_context(AppContext {
        engine: engine.engine.clone(),
        options: Default::default(),
        started: Instant::now(),
        settings: Rc::new(RefCell::new(Settings::default())),
        settings_path: dir.join("settings.json"),
        winit: false,
    });
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(1000.0, 900.0));
    w.open_path(pdf.clone());
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.has_document()));

    // An annotation on page 4 (index 3) and a bookmark on page 6 (index 5).
    w.add_annotation(Annotation::new(0, 3, Rect::new(100.0, 100.0, 200.0, 50.0), Shape::Rectangle, Style::default()));
    w.go_to_page(5);
    assert!(w.command("toggle-bookmark"));

    // --- Multi-select and rotate --------------------------------------------------
    w.thumb_clicked(1, false, false);
    w.thumb_clicked(2, false, true);
    assert_eq!(w.selected_pages(), vec![1, 2]);
    w.thumb_clicked(4, true, false);
    assert_eq!(w.selected_pages(), vec![1, 2, 4]);
    w.thumb_clicked(4, true, false);
    assert!(w.command("rotate-right"));
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.ui.get_can_undo() && w2.annotations().len() == 1 && {
        let s = w2.page_sizes();
        s.len() == 6 && s[1].w > s[1].h
    }));
    // Rotating the annotation's page turns the annotation with it.
    w.thumb_clicked(3, false, false);
    assert!(w.command("rotate-right"));
    let w2 = w.clone();
    assert!(
        pump_until(Duration::from_secs(10), move || {
            let r = rect_on(&w2);
            (r.bounds.w - 50.0).abs() < 0.5 && (r.bounds.h - 200.0).abs() < 0.5
        }),
        "annotation rotated with its page: {:?}",
        rect_on(&w).bounds
    );

    // --- Delete the first page: annotation and bookmark follow --------------------
    w.thumb_clicked(0, false, false);
    assert!(w.command("delete-pages"));
    wait_pages(&w, 5);
    assert_eq!(rect_on(&w).page, 2);
    assert_eq!(w.bookmarks(), vec![4]);

    // --- Drag the annotated page to the end ---------------------------------------
    w.thumb_clicked(2, false, false);
    pump_until(Duration::from_millis(200), || false);
    let (start, end) = w.thumb_drag_points(2);
    w.thumb_drag(2, start);
    w.thumb_drag(2, end);
    assert!(w.ui.get_thumb_drop_visible(), "an insertion marker appears");
    w.thumb_drop();
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || rect_on(&w2).page == 4), "page moved to the end");
    assert!(!w.ui.get_thumb_drop_visible());

    // --- Undo the move, then the delete -------------------------------------------
    w.undo();
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || rect_on(&w2).page == 2));
    w.undo();
    wait_pages(&w, 6);
    assert_eq!(rect_on(&w).page, 3);

    // --- Insert blank page and pages from another file ----------------------------
    w.go_to_page(0);
    assert!(w.command("insert-blank"));
    wait_pages(&w, 7);
    w.insert_from_file(Some(extra.clone()));
    wait_pages(&w, 10);
    assert_eq!(rect_on(&w).page, 7, "annotation shifted by the four inserted pages");

    // --- Crop -------------------------------------------------------------------
    let page = rect_on(&w).page;
    w.set_rect_selection(page, Rect::new(0.0, 0.0, 300.0, 300.0));
    assert!(w.command("crop"));
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || (w2.page_sizes()[page].w - 300.0).abs() < 0.5));

    // --- Save; the saved file has the edits ----------------------------------------
    let out = dir.join("organised.pdf");
    w.save_to(out.clone(), SaveOptions::default(), AfterSave::Nothing);
    wait_saved(&w);
    let saved = PdfDoc::open(Source::File(out.clone()), None).unwrap();
    assert_eq!(saved.page_count(), 10);

    // --- Export with an effect and a password ----------------------------------------
    *w.last_saved.borrow_mut() = None;
    let exported = dir.join("exported.pdf");
    w.export_to(
        exported.clone(),
        SaveOptions {
            effect: Some(Effect::SepiaTone),
            protection: Some(Protection {
                open_password: "pw".into(),
                allow_printing: true,
                allow_copying: true,
                allow_editing: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    wait_saved(&w);
    assert!(matches!(PdfDoc::open(Source::File(exported.clone()), None), Err(OpenError::NeedsPassword)));
    assert_eq!(PdfDoc::open(Source::File(exported), Some("pw")).unwrap().page_count(), 10);
    assert_eq!(w.doc_path().unwrap(), out, "export keeps editing the current file");
    // Export never overwrites the open document.
    w.export_to(out.clone(), SaveOptions::default());
    assert!(w.last_error.borrow().as_deref().unwrap_or("").contains("different name"));

    // --- Duplicate --------------------------------------------------------------------
    w.duplicate();
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || app::windows()
        .iter()
        .any(|x| x.id != w2.id && x.has_document())));
    let dup = app::windows().into_iter().find(|x| x.id != w.id && x.has_document()).unwrap();
    assert!(dup.doc_path().is_none() && dup.is_dirty());
    assert!(dup.title().ends_with("copy"));
    assert_eq!(dup.page_count(), 10);

    // --- Lock -------------------------------------------------------------------------
    assert!(w.command("lock"));
    assert!(w.is_locked() && w.ui.get_locked());
    *w.last_error.borrow_mut() = None;
    assert!(w.command("save"));
    assert!(w.last_error.borrow().as_deref().unwrap_or("").contains("locked"));
    assert!(w.command("lock"));
    assert!(!w.is_locked());
}
