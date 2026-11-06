//! Headless UI tests for M4 markup: creating, editing, styling, undo, saving and
//! redaction through the real window controller.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use paper_mirror::app::window::{AfterSave, DocWindow};
use paper_mirror::app::{self, AppContext};
use paper_mirror::core::annot::{Color, Shape};
use paper_mirror::core::geometry::Point;
use paper_mirror::fixtures::{self, NEEDLE};
use paper_mirror::pdf::annot_io::SaveOptions;
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

fn open(path: std::path::PathBuf) -> Rc<DocWindow> {
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(1000.0, 900.0));
    w.open_path(path);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.has_document()));
    w.command("zoom-actual");
    settle();
    w
}

fn drag(w: &DocWindow, from: Point, to: Point) {
    w.pointer(0, from, false, false, 0);
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        w.pointer(1, Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t), false, false, 0);
    }
    w.pointer(2, to, false, false, 0);
}

#[test]
fn markup_end_to_end() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = std::env::temp_dir().join(format!("pm-ui-markup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: this test binary has a single test; set before any data-dir use.
    unsafe { std::env::set_var("PAPER_MIRROR_DATA_DIR", dir.join("data")) };
    let pdf = dir.join("doc.pdf");
    std::fs::write(&pdf, fixtures::text_document()).unwrap();
    let engine = Engine::start();
    app::install_context(AppContext {
        engine: engine.engine.clone(),
        options: Default::default(),
        started: Instant::now(),
        settings: Rc::new(RefCell::new(Settings::default())),
        settings_path: dir.join("settings.json"),
        winit: false,
    });
    let w = open(pdf.clone());
    let page = w.ui.get_pages().iter().find(|p| p.index == 0).unwrap();
    let k = page.width / 612.0;
    let at = |x: f32, y: f32| Point::new(page.x + x * k, page.y + y * k);

    // --- Rectangle: create, move, resize, restyle, delete, undo, redo -----------------
    assert!(w.command("shape-rect"));
    assert!(w.ui.get_markup_visible(), "creation tools open the markup toolbar");
    drag(&w, at(100.0, 300.0), at(250.0, 400.0));
    let a = w.annotations();
    assert_eq!(a.len(), 1);
    assert!(matches!(a[0].shape, Shape::Rectangle));
    assert!((a[0].bounds.w - 150.0).abs() < 1.0 && (a[0].bounds.h - 100.0).abs() < 1.0, "{:?}", a[0].bounds);
    assert!(w.ui.get_annot_selected() && w.ui.get_annot_resizable());
    assert!(w.ui.get_annot_prims().row_count() >= 1);
    assert!(w.is_dirty() && w.ui.get_edited());
    // Move it by dragging its interior.
    assert!(w.command("tool-text"));
    drag(&w, at(150.0, 350.0), at(180.0, 370.0));
    let b = w.annotations()[0].bounds;
    assert!((b.x - 130.0).abs() < 1.0 && (b.y - 320.0).abs() < 1.0, "moved: {b:?}");
    // Resize from the bottom-right handle.
    drag(&w, at(b.right(), b.bottom()), at(b.right() + 50.0, b.bottom() + 20.0));
    let c = w.annotations()[0].bounds;
    assert!((c.w - 200.0).abs() < 1.0 && (c.h - 120.0).abs() < 1.0, "resized: {c:?}");
    w.set_stroke_color(Color::rgb(0, 128, 0));
    assert_eq!(w.annotations()[0].style.stroke, Color::rgb(0, 128, 0));
    assert!(w.delete_selected_annotation());
    assert!(w.annotations().is_empty());
    w.undo();
    assert_eq!(w.annotations().len(), 1);
    assert_eq!(w.annotations()[0].style.stroke, Color::rgb(0, 128, 0));
    w.redo();
    assert!(w.annotations().is_empty());
    w.undo();

    // --- Draw and Sketch ---------------------------------------------------------------
    assert!(w.command("tool-draw"));
    drag(&w, at(300.0, 450.0), at(400.0, 480.0));
    assert!(matches!(w.annotations().last().unwrap().shape, Shape::Ink { .. }));
    assert!(w.command("tool-sketch"));
    let (cx, cy) = (450.0, 600.0);
    w.pointer(0, at(cx + 50.0, cy), false, false, 0);
    for i in 1..=40 {
        let ang = i as f32 / 40.0 * std::f32::consts::TAU;
        w.pointer(1, at(cx + 50.0 * ang.cos(), cy + 35.0 * ang.sin()), false, false, 0);
    }
    w.pointer(2, at(cx + 50.0, cy), false, false, 0);
    assert!(matches!(w.annotations().last().unwrap().shape, Shape::Oval), "sketched circle becomes an oval");

    // --- Text box and note ---------------------------------------------------------------
    assert!(w.command("tool-textbox"));
    w.pointer(0, at(80.0, 650.0), false, false, 0);
    w.pointer(2, at(80.0, 650.0), false, false, 0);
    assert!(w.ui.get_editing());
    w.finish_edit("Reviewed — looks good");
    assert!(!w.ui.get_editing());
    assert_eq!(w.annotations().last().unwrap().shape.text(), Some("Reviewed — looks good"));
    assert!(w.command("tool-note"));
    w.pointer(0, at(560.0, 60.0), false, false, 0);
    w.pointer(2, at(560.0, 60.0), false, false, 0);
    assert!(w.ui.get_editing() && w.ui.get_edit_note());
    w.finish_edit("Check this figure");
    assert!(matches!(&w.annotations().last().unwrap().shape, Shape::Note { text } if text == "Check this figure"));

    // --- Highlight a word from a text selection --------------------------------------------
    assert!(w.command("tool-text"));
    w.escape();
    w.pointer(4, at(110.0, 103.0), false, false, 0);
    assert!(w.text_selection().is_some(), "word selected");
    assert!(w.command("highlight"));
    let hl =
        w.annotations().into_iter().find(|a| matches!(a.shape, Shape::Highlight { .. })).expect("highlight created");
    assert_eq!(hl.page, 0);
    assert!(w.command("sidebar-notes"));
    assert!(w.ui.get_note_rows().iter().any(|r| r.title.contains(NEEDLE)), "notes list shows the highlighted text");
    let count = w.annotations().len();

    // --- Save as a new file and reopen ----------------------------------------------------
    let out = dir.join("annotated.pdf");
    w.save_to(out.clone(), SaveOptions::default(), AfterSave::Nothing);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.last_saved.borrow().is_some()));
    assert!(!w.is_dirty(), "saving clears the edited state");
    assert_eq!(w.doc_path().unwrap(), out, "Save As adopts the new file");
    let r = open(out.clone());
    assert_eq!(r.annotations().len(), count, "annotations reopen as editable objects");
    r.close();

    // --- Redaction applied on save --------------------------------------------------------
    w.pointer(4, at(110.0, 103.0), false, false, 0);
    assert!(w.command("tool-redact"), "with text selected, Redact marks the selection");
    assert!(w.has_redactions());
    let red = dir.join("redacted.pdf");
    *w.last_saved.borrow_mut() = None;
    w.save_to(red.clone(), SaveOptions::default(), AfterSave::Nothing);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.last_saved.borrow().is_some()));
    assert!(!w.has_redactions(), "applied redactions leave the model");
    let bytes = std::fs::read(&red).unwrap();
    let d = paper_mirror::pdf::document::PdfDoc::open(
        paper_mirror::pdf::ffi::Source::Memory(std::sync::Arc::new(bytes)),
        None,
    )
    .unwrap();
    let t = paper_mirror::pdf::text::page_text(&d, 0);
    assert!(!t.contains(NEEDLE) && t.contains("keeper"), "{t}");

    // --- In-place save keeps an original copy ---------------------------------------------
    assert!(w.command("shape-oval"));
    drag(&w, at(100.0, 200.0), at(160.0, 240.0));
    *w.last_saved.borrow_mut() = None;
    w.save_to(red.clone(), SaveOptions::default(), AfterSave::Nothing);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.last_saved.borrow().is_some()));
    let orig = paper_mirror::storage::originals::original_for(&red).expect("original preserved before overwrite");
    assert_ne!(std::fs::read(orig).unwrap(), std::fs::read(&red).unwrap());

    // --- Unsaved changes keep the window open (no prompt under the test backend) -------------
    assert!(w.command("shape-oval"));
    drag(&w, at(300.0, 200.0), at(360.0, 240.0));
    assert!(w.is_dirty());
    assert!(w.command("close"));
    assert!(w.has_document(), "a dirty window is not closed without an answer");
}
