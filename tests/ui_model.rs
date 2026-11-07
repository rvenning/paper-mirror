//! Headless UI tests for M9: opening 3D models, orbit/zoom, orientation edits,
//! saving and exporting, and the explanation for unsupported formats.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use paper_mirror::app::window::DocWindow;
use paper_mirror::app::{self, AppContext};
use paper_mirror::core::geometry::Point;
use paper_mirror::images::encode::SaveFormat;
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

fn wait(w: &Rc<DocWindow>, what: &str, cond: impl Fn(&DocWindow) -> bool + 'static) {
    let w2 = w.clone();
    assert!(pump_until(Duration::from_secs(20), move || cond(&w2)), "timed out waiting for {what}");
}

const CUBE_OBJ: &str = "mtllib cube.mtl\nusemtl Red\n\
v 0 0 0\nv 2 0 0\nv 2 1 0\nv 0 1 0\nv 0 0 3\nv 2 0 3\nv 2 1 3\nv 0 1 3\n\
f 1 3 2\nf 1 4 3\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\nf 3 4 8\nf 3 8 7\nf 2 3 7\nf 2 7 6\nf 1 5 8\nf 1 8 4\n";

fn open(path: PathBuf) -> Rc<DocWindow> {
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(640.0, 480.0));
    w.open_path(path);
    w
}

fn has_image(w: &DocWindow) -> bool {
    w.ui.get_pages().iter().any(|p| p.has_image)
}

fn bounds(m: &paper_mirror::model3d::Mesh) -> ([f32; 3], [f32; 3]) {
    m.bounds()
}

#[test]
fn models_end_to_end() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = std::env::temp_dir().join(format!("pm-ui-model-{}", std::process::id()));
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
    let obj = dir.join("box.obj");
    std::fs::write(&obj, CUBE_OBJ).unwrap();
    std::fs::write(dir.join("cube.mtl"), "newmtl Red\nKd 0.9 0.1 0.1\n").unwrap();

    // --- Open and render ------------------------------------------------------------
    let w = open(obj.clone());
    wait(&w, "first render", has_image);
    assert!(w.is_model());
    assert!(w.ui.get_is_model());
    let mesh = w.model_mesh().unwrap();
    assert_eq!(mesh.triangles.len(), 12);
    assert_eq!(mesh.colors.as_ref().unwrap()[0], [230, 26, 26], "the MTL colour is used");
    assert!(w.command("inspector"));
    wait(&w, "inspector", |w| w.ui.get_info_rows().iter().any(|r| r.key == "Triangles"));
    let size = w.ui.get_info_rows().iter().find(|r| r.key == "Size").unwrap().value;
    assert_eq!(size.as_str(), "2 × 1 × 3 (model units)");

    // --- Orbit with a drag, zoom, reset ---------------------------------------------
    let before = w.model_camera().unwrap();
    w.pointer(0, Point::new(300.0, 200.0), false, false, 0);
    w.pointer(1, Point::new(360.0, 230.0), false, false, 0);
    w.pointer(2, Point::new(360.0, 230.0), false, false, 0);
    let after = w.model_camera().unwrap();
    assert!((after.yaw - before.yaw).abs() > 10.0 && (after.pitch - before.pitch).abs() > 5.0);
    // Shift+drag pans: the target moves, the angles stay.
    w.pointer(0, Point::new(300.0, 200.0), true, false, 0);
    w.pointer(1, Point::new(340.0, 200.0), true, false, 0);
    w.pointer(2, Point::new(340.0, 200.0), true, false, 0);
    let panned = w.model_camera().unwrap();
    assert_ne!(panned.target, after.target);
    assert_eq!(panned.yaw, after.yaw);
    assert!(w.command("zoom-in"));
    assert!(w.model_camera().unwrap().distance < panned.distance);
    assert!(w.command("zoom-actual"));
    assert_eq!(w.model_camera().unwrap().yaw, before.yaw);
    assert!(!w.is_dirty(), "viewing doesn't change the file");

    // --- Orientation edits change the mesh; Save writes them ---------------------------
    assert!(w.command("flip-up-axis"));
    assert!(w.is_dirty());
    let (lo, hi) = bounds(&w.model_mesh().unwrap());
    assert_eq!(((hi[1] - lo[1]) as i32, (hi[2] - lo[2]) as i32), (3, 1), "Z-up became Y-up");
    let stl = dir.join("box.stl");
    w.save_model_to(stl.clone(), false);
    let s2 = stl.clone();
    wait(&w, "save", move |w| w.last_saved.borrow().as_deref() == Some(&s2) && !w.is_dirty());
    let saved = paper_mirror::model3d::load(&stl, paper_mirror::core::filetype::ModelFormat::Stl).unwrap();
    assert_eq!(saved.triangles.len(), 12);
    let (lo, hi) = saved.bounds();
    assert!(((hi[1] - lo[1]) - 3.0).abs() < 1e-5);
    assert_eq!(w.doc_path().as_deref(), Some(stl.as_path()));

    // --- Export a GLB copy and an image of the view -------------------------------------
    let glb = dir.join("box-copy.glb");
    *w.last_saved.borrow_mut() = None;
    w.save_model_to(glb.clone(), true);
    let g2 = glb.clone();
    wait(&w, "glb", move |w| w.last_saved.borrow().as_deref() == Some(&g2));
    assert_eq!(w.doc_path().as_deref(), Some(stl.as_path()), "export keeps the current file");
    let png = dir.join("view.png");
    w.export_model_image(png.clone(), SaveFormat::Png, 90);
    let p2 = png.clone();
    wait(&w, "png", move |w| w.last_saved.borrow().as_deref() == Some(&p2));
    let img = paper_mirror::images::decode::decode(
        &std::fs::read(&png).unwrap(),
        paper_mirror::core::filetype::ImageFormat::Png,
    )
    .unwrap();
    let f = &img.frames[0];
    let view_w = w.ui.get_view_width();
    assert!((f.width as f32 - view_w * 2.0).abs() <= 2.0, "rendered at twice the view size: {} vs {view_w}", f.width);
    let centre = f.pixel(f.width / 2, f.height / 2);
    let corner = f.pixel(2, 2);
    assert!(centre[0] > centre[2] + 40, "the red box is in the middle: {centre:?}");
    assert!(corner[0] > 220 && corner[2] > 220, "background in the corner: {corner:?}");

    // --- The GLB opens in another window ---------------------------------------------
    let g = open(glb);
    wait(&g, "glb render", has_image);
    assert_eq!(g.model_mesh().unwrap().triangles.len(), 12);
    assert!(g.model_mesh().unwrap().colors.is_some());

    // --- Unsupported formats explain themselves ------------------------------------------
    let usdz = dir.join("scene.usdz");
    std::fs::write(&usdz, b"PK\x03\x04 not really a usdz").unwrap();
    let u = DocWindow::open_new(None).unwrap();
    u.open_path(usdz);
    assert!(!u.has_document());
    let err = u.last_error.borrow().clone().unwrap();
    assert!(err.contains("OpenUSD"), "{err}");
    drop(engine);
}
