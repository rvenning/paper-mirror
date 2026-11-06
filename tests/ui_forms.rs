//! Headless UI tests for M5: form filling, undo, signatures and saving.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use paper_mirror::app::window::{AfterSave, DocWindow};
use paper_mirror::app::{self, AppContext};
use paper_mirror::core::annot::Shape;
use paper_mirror::core::geometry::Point;
use paper_mirror::fixtures;
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

fn open(path: std::path::PathBuf) -> Rc<DocWindow> {
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(1000.0, 900.0));
    w.open_path(path);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.has_document() && w2.fields().len() == 3));
    w
}

#[test]
fn forms_and_signatures() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = std::env::temp_dir().join(format!("pm-ui-forms-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: single test in this binary; set before any data-dir use.
    unsafe { std::env::set_var("PAPER_MIRROR_DATA_DIR", dir.join("data")) };
    let pdf = dir.join("form.pdf");
    std::fs::write(&pdf, fixtures::form_document()).unwrap();
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

    // Overlays exist for every field, in reading order, with the right kinds.
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.ui.get_form_fields().row_count() == 3));
    let kinds: Vec<String> = w.ui.get_form_fields().iter().map(|f| f.kind.to_string()).collect();
    assert_eq!(kinds, vec!["text", "check", "choice"]);

    // Fill each field.
    w.field_commit_text(0, "Grace Hopper");
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.fields()[0].value == "Grace Hopper"));
    w.field_toggle(1);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.fields()[1].checked));
    w.field_choose(2, 0);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.fields()[2].value == "Red"));
    assert!(w.is_dirty() && w.ui.get_can_undo());

    // Undo restores the previous document state (the choice goes back to Green).
    w.undo();
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.fields().get(2).is_some_and(|f| f.value == "Green")));
    assert_eq!(w.fields()[0].value, "Grace Hopper", "earlier edits survive the undo");
    w.redo();
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(5), move || w2.fields().get(2).is_some_and(|f| f.value == "Red")));

    // Create a signature (stored encrypted) and place it on the page.
    let strokes =
        vec![vec![Point::new(0.0, 30.0), Point::new(40.0, 0.0), Point::new(80.0, 30.0), Point::new(120.0, 5.0)]];
    assert!(w.add_signature_for_test(&strokes, "G. Hopper").is_some());
    assert!(dir.join("data").join("signatures.dat").exists());
    assert!(w.command("tool-signature"));
    assert!(w.ui.get_signatures_visible(), "the Signatures panel opens");
    assert_eq!(w.ui.get_signatures().row_count(), 1);
    let sig_id = w.ui.get_signatures().row_data(0).unwrap().id;
    w.signature_chosen(sig_id as u64);
    assert!(!w.ui.get_signatures_visible());
    w.command("zoom-actual");
    pump_until(Duration::from_millis(200), || false);
    let page = w.ui.get_pages().iter().find(|p| p.index == 0).unwrap();
    let k = page.width / 612.0;
    let at = Point::new(page.x + 300.0 * k, page.y + 500.0 * k);
    w.pointer(0, at, false, false, 0);
    w.pointer(2, at, false, false, 0);
    let sig =
        w.annotations().into_iter().find(|a| matches!(a.shape, Shape::Signature { .. })).expect("signature placed");
    assert!((sig.bounds.h - 40.0).abs() < 1.0, "{:?}", sig.bounds);

    // Save, reopen: field values and the signature persist.
    let out = dir.join("filled.pdf");
    w.save_to(out.clone(), SaveOptions::default(), AfterSave::Nothing);
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.last_saved.borrow().is_some()));
    let r = open(out);
    let f = r.fields();
    assert_eq!(f[0].value, "Grace Hopper");
    assert!(f[1].checked);
    assert_eq!(f[2].value, "Red");
    assert!(r.annotations().iter().any(|a| matches!(a.shape, Shape::Signature { .. })));
}

#[test]
fn signature_images_are_traced() {
    let dir = std::env::temp_dir().join(format!("pm-sig-img-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut img = image::GrayImage::from_pixel(400, 120, image::Luma([245]));
    for x in 40..360 {
        let y = 60 + ((x as f32 / 30.0).sin() * 25.0) as u32;
        for t in 0..4 {
            img.put_pixel(x, y + t, image::Luma([20]));
        }
    }
    let p = dir.join("sig.png");
    img.save(&p).unwrap();
    let strokes = paper_mirror::app::window::trace_signature_image(&p).unwrap();
    assert!(strokes.len() > 40, "ink rows become strokes: {}", strokes.len());
    let blank = dir.join("blank.png");
    image::GrayImage::from_pixel(100, 50, image::Luma([255])).save(&blank).unwrap();
    assert!(paper_mirror::app::window::trace_signature_image(&blank).is_err());
}
