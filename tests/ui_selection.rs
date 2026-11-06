//! Headless UI tests for M3: text selection and copy.

use std::cell::RefCell;
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

#[test]
fn text_selection_and_copy() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = std::env::temp_dir().join(format!("pm-ui-sel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pdf = dir.join("text.pdf");
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
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(1000.0, 800.0));
    w.open_path(pdf);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.has_document()));
    w.command("zoom-actual");
    settle();

    // Page 1 at 100 %: the second line ("The lighthouse keeper …") has its baseline
    // at PDF y = 792 - 72 - 34 = 686, so display y ≈ 106 points from the top.
    let page = w.ui.get_pages().iter().find(|p| p.index == 0).unwrap();
    let k = page.width / 612.0;
    let at = |x: f32, y: f32| Point::new(page.x + x * k, page.y + y * k);

    // Hovering over text shows the I-beam once geometry is loaded.
    let w2 = w.clone();
    let over = at(100.0, 103.0);
    assert!(pump_until(Duration::from_secs(5), move || {
        w2.pointer(1, over, false, false, 0);
        w2.ui.get_canvas_cursor() == "text"
    }));

    // Drag across the whole second line.
    w.pointer(0, at(70.0, 103.0), false, false, 0);
    w.pointer(1, at(200.0, 103.0), false, false, 0);
    w.pointer(1, at(400.0, 103.0), false, false, 0);
    w.pointer(2, at(400.0, 103.0), false, false, 0);
    assert!(w.text_selection().is_some());
    assert!(w.ui.get_has_text_selection());
    assert!(w.ui.get_overlays().iter().any(|o| o.kind == 2), "selection is highlighted");
    assert!(w.command("copy"));
    let copied = w.last_copied.borrow().clone().unwrap();
    assert!(copied.starts_with("The lighthouse keeper"), "{copied:?}");
    assert!(!copied.contains("Fixture"), "only the dragged line: {copied:?}");

    // A click clears the selection; a double-click selects a word.
    w.pointer(0, at(300.0, 400.0), false, false, 0);
    w.pointer(2, at(300.0, 400.0), false, false, 0);
    assert!(w.text_selection().is_none());
    w.pointer(4, at(110.0, 103.0), false, false, 0);
    assert!(w.command("copy"));
    assert_eq!(w.last_copied.borrow().as_deref(), Some(NEEDLE));
    assert!(w.escape(), "Esc clears the text selection");
    assert!(w.text_selection().is_none());

    // Select All copies every page (gathered on the engine thread).
    *w.last_copied.borrow_mut() = None;
    assert!(w.command("select-all"));
    assert!(w.command("copy"));
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.last_copied.borrow().is_some()));
    let all = w.last_copied.borrow().clone().unwrap();
    for p in 1..=3 {
        assert!(all.contains(&format!("Fixture page {p}")), "{all}");
    }
    w.escape();

    // Copying a rectangular selection copies an image of that region.
    *w.last_copied.borrow_mut() = None;
    w.set_rect_selection(0, Rect::new(72.0, 60.0, 144.0, 72.0));
    assert!(w.command("copy"));
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.last_copied.borrow().is_some()));
    assert_eq!(w.last_copied.borrow().as_deref(), Some("[image 384x192]"));

    // With a text field focused, Copy goes to the field, not the document.
    *w.last_copied.borrow_mut() = None;
    let bridge = w.ui.global::<paper_mirror::ui::EditBridge>();
    bridge.set_field_focused(true);
    let serial = bridge.get_serial();
    assert!(w.command("copy"));
    assert_eq!(bridge.get_serial(), serial + 1);
    assert_eq!(bridge.get_op(), "copy");
    assert!(w.last_copied.borrow().is_none());
    bridge.set_field_focused(false);
}
