//! Headless UI integration tests: the real `MainWindow` and `DocWindow` controller
//! driven through Slint's testing backend, with the real PDF engine thread.
//!
//! Slint UI must stay on one thread, so everything runs inside a single test.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use paper_mirror::app::{self, AppContext, window::DocWindow};
use paper_mirror::fixtures;
use paper_mirror::pdf::engine::Engine;
use paper_mirror::storage::settings::Settings;
use slint::{ComponentHandle, Model};

/// Runs the event loop until `cond` holds or `timeout` passes.
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

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pm-ui-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn shell_end_to_end() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = temp_dir("shell");
    let text_pdf = dir.join("text.pdf");
    std::fs::write(&text_pdf, fixtures::text_document()).unwrap();
    let locked_pdf = dir.join("locked.pdf");
    std::fs::write(&locked_pdf, fixtures::encrypted_document("open sesame")).unwrap();
    let bogus = dir.join("notes.pdf");
    std::fs::write(&bogus, fixtures::not_a_pdf()).unwrap();
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

    // --- Open a PDF and wait for the first page bitmap -------------------------
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(1000.0, 800.0));
    assert!(!w.ui.get_has_document());
    w.open_path(text_pdf.clone());
    let w2 = w.clone();
    assert!(
        pump_until(Duration::from_secs(10), move || {
            let p = w2.ui.get_pages();
            p.row_count() > 0 && p.row_data(0).is_some_and(|i| i.has_image)
        }),
        "first page never rendered"
    );
    assert!(w.ui.get_has_document());
    assert_eq!(w.ui.get_doc_title(), "text.pdf");
    assert_eq!(w.ui.get_doc_subtitle(), "Page 1 of 3");
    let w2 = w.clone();
    assert!(
        pump_until(Duration::from_secs(10), move || w2.ui.get_thumbs().iter().any(|t| t.has_image)),
        "no thumbnails"
    );
    // The file was added to Open Recent and persisted.
    assert_eq!(w.ui.get_recent_files().row_count(), 1);
    assert_eq!(Settings::load_from(&settings_path).recent.len(), 1);

    // --- Navigation ------------------------------------------------------------
    assert!(w.command("page-last"));
    settle();
    assert_eq!(w.ui.get_doc_subtitle(), "Page 3 of 3");
    assert!(w.command("page-first"));
    settle();
    assert_eq!(w.ui.get_doc_subtitle(), "Page 1 of 3");

    // Go to Page: invalid input keeps the prompt open with an error.
    assert!(w.command("go-to-page"));
    assert_eq!(w.ui.get_prompt_kind(), "go-to-page");
    w.ui.invoke_prompt_accepted("go-to-page".into(), "99".into());
    assert!(!w.ui.get_prompt_error().is_empty());
    w.ui.invoke_prompt_accepted("go-to-page".into(), "2".into());
    assert_eq!(w.ui.get_prompt_kind(), "");
    settle();
    assert_eq!(w.ui.get_doc_subtitle(), "Page 2 of 3");

    // --- View modes and zoom ---------------------------------------------------
    assert!(w.command("view-two-pages"));
    assert_eq!(w.ui.get_display_mode(), "two-pages");
    assert!(w.command("view-single"));
    settle();
    assert_eq!(w.ui.get_pages().row_count(), 1, "single page mode shows one page");
    assert!(w.command("view-continuous"));
    assert_eq!(Settings::load_from(&settings_path).display_mode, "continuous");
    let before = w.ui.get_content_height();
    assert!(w.command("zoom-in"));
    assert!(w.ui.get_content_height() > before);
    assert!(w.command("zoom-actual"));
    assert_eq!(w.ui.get_zoom_label(), "100%");

    // --- Sidebar and appearance persist ----------------------------------------
    assert!(w.command("toggle-sidebar"));
    assert!(!w.ui.get_sidebar_visible());
    assert!(!Settings::load_from(&settings_path).sidebar_visible);
    assert!(w.command("toggle-sidebar"));
    assert!(w.command("appearance-dark"));
    assert_eq!(w.ui.global::<paper_mirror::ui::Theme>().get_appearance(), "dark");
    assert!(w.ui.global::<paper_mirror::ui::Theme>().get_dark());
    assert!(w.command("appearance-system"));

    // Unknown commands are rejected.
    assert!(!w.command("no-such-command"));

    // --- Opening the same file again focuses the existing window ----------------
    let empty = DocWindow::open_new(None).unwrap();
    empty.open_here_or_new(text_pdf.clone());
    assert!(!empty.has_document(), "the open copy is focused instead of duplicated");
    assert_eq!(app::windows().iter().filter(|x| x.has_document()).count(), 1);

    // --- Password-protected PDF ------------------------------------------------
    empty.open_path(locked_pdf.clone());
    let pw = empty.clone();
    assert!(pump_until(Duration::from_secs(10), move || pw.ui.get_prompt_kind() == "password"), "no password prompt");
    assert!(empty.ui.get_prompt_password(), "prompt must mask input");
    empty.ui.invoke_prompt_accepted("password".into(), "wrong".into());
    let pw = empty.clone();
    assert!(
        pump_until(Duration::from_secs(10), move || !pw.ui.get_prompt_error().is_empty()),
        "wrong password not reported"
    );
    empty.ui.invoke_prompt_accepted("password".into(), "open sesame".into());
    let pw = empty.clone();
    assert!(pump_until(Duration::from_secs(10), move || pw.has_document()), "correct password did not open");
    assert_eq!(empty.ui.get_prompt_kind(), "");

    // --- A non-PDF file is refused with an actionable message --------------------
    let e = DocWindow::open_new(None).unwrap();
    e.open_path(bogus);
    assert!(!e.has_document());
    let msg = e.last_error.borrow().clone().unwrap();
    assert!(msg.contains("not in a supported format"), "{msg}");

    // --- Window menu lists every window; closing releases the document ----------
    assert_eq!(w.ui.get_window_titles().row_count(), app::windows().len());
    empty.close();
    settle();
    assert!(!empty.has_document());
    assert_eq!(w.ui.get_window_titles().row_count(), app::windows().len());
    assert!(app::windows().iter().all(|x| x.id != empty.id));

    // Clear recent.
    assert!(w.command("clear-recent"));
    assert_eq!(w.ui.get_recent_files().row_count(), 0);
}
