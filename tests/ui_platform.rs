//! Headless UI tests for M8: printing a marked-up document (to Microsoft Print to
//! PDF, when installed) and sharing a document with unsaved changes as a copy.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use paper_mirror::app::window::DocWindow;
use paper_mirror::app::{self, AppContext};
use paper_mirror::core::annot::{Annotation, Shape, Style};
use paper_mirror::core::geometry::Rect;
use paper_mirror::fixtures;
use paper_mirror::pdf::document::PdfDoc;
use paper_mirror::pdf::engine::Engine;
use paper_mirror::pdf::ffi::Source;
use paper_mirror::pdf::print;
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

#[test]
fn print_and_share() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = std::env::temp_dir().join(format!("pm-ui-platform-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: single test in this binary; set before any data-dir use.
    unsafe { std::env::set_var("PAPER_MIRROR_DATA_DIR", dir.join("data")) };
    let engine = Engine::start();
    app::install_context(AppContext {
        engine: engine.engine.clone(),
        options: Default::default(),
        started: Instant::now(),
        settings: Rc::new(RefCell::new(Settings::default())),
        settings_path: dir.join("settings.json"),
        winit: false,
    });
    let pdf = dir.join("doc.pdf");
    std::fs::write(&pdf, fixtures::text_document()).unwrap();
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(900.0, 800.0));
    w.open_path(pdf.clone());
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.has_document()));
    // A filled black rectangle in an empty corner of page 1, so it shows when printed.
    let style = Style { fill: Some(paper_mirror::core::annot::Color::BLACK), ..Default::default() };
    w.add_annotation(Annotation::new(0, 0, Rect::new(400.0, 650.0, 150.0, 100.0), Shape::Rectangle, style));

    // --- Print pages 1 and 3 with the markup ------------------------------------------
    if let Some(dc) = print::printer_dc("Microsoft Print to PDF") {
        let out = dir.join("printed.pdf");
        w.print_to(dc, vec![0, 2], Some(out.clone()));
        let w2 = w.clone();
        assert!(pump_until(Duration::from_secs(60), move || w2.last_printed.borrow().is_some()));
        assert_eq!(w.last_printed.borrow().clone(), Some(Ok(2)));
        let start = Instant::now();
        while std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) == 0 {
            assert!(start.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(500));
        let printed = PdfDoc::open(Source::File(out), None).unwrap();
        assert_eq!(printed.page_count(), 2);
        // The rectangle is printed: the lower-right area of the first printed page is dark.
        let (pw, ph) = printed.page_sizes()[0];
        let (w_px, h_px) = (200u32, (200.0 * ph / pw) as u32);
        let mut px = vec![0u8; (w_px * h_px * 4) as usize];
        let target = paper_mirror::pdf::document::RenderTarget { pixels: &mut px, width: w_px, height: h_px };
        assert!(printed.render(0, w_px as f32 / pw, 0.0, 0.0, target, true));
        let dark = px.as_chunks::<4>().0.iter().filter(|p| p[0] < 60 && p[1] < 60 && p[2] < 60).count();
        assert!(dark > 500, "the markup is printed ({dark} dark pixels)");
    } else {
        eprintln!("Microsoft Print to PDF is not installed; skipping the print check");
    }

    // --- Share: unsaved changes are shared as a copy --------------------------------
    assert!(w.is_dirty());
    assert!(w.command("share"));
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(10), move || w2.last_shared.borrow().is_some()));
    let shared = w.last_shared.borrow().clone().unwrap();
    assert_ne!(shared, pdf, "the edited state is shared, not the old file");
    assert_eq!(shared.file_name().unwrap(), "doc.pdf");
    let copy = PdfDoc::open(Source::File(shared.clone()), None).unwrap();
    let annots = paper_mirror::pdf::annot_io::extract(&copy, &mut 0);
    assert_eq!(annots.len(), 1, "the copy carries the markup");
    assert!(w.is_dirty(), "sharing doesn't save the document");
    drop(copy);
    let _ = std::fs::remove_file(&shared);
    drop(engine);
}
