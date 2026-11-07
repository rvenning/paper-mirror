//! Headless UI tests for M7: opening images, markup baked on save, metadata and
//! location, flip, Adjust Size, Adjust Color, Instant Alpha, export and conversion.

use std::cell::RefCell;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use paper_mirror::app::window::{AfterSave, DocWindow};
use paper_mirror::app::{self, AppContext};
use paper_mirror::core::annot::{Annotation, Color, Shape, Style};
use paper_mirror::core::filetype::ImageFormat;
use paper_mirror::core::geometry::{Point, Rect};
use paper_mirror::fixtures;
use paper_mirror::images::encode::{self, EncodeOptions, ImageTarget, SaveFormat};
use paper_mirror::images::{Frame, PT_PER_PX, decode, metadata};
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
    assert!(pump_until(Duration::from_secs(15), move || cond(&w2)), "timed out waiting for {what}");
}

/// Saves a copy as `name` and decodes it.
fn export(w: &Rc<DocWindow>, dir: &Path, name: &str, target: ImageTarget) -> paper_mirror::images::Decoded {
    let path = dir.join(name);
    *w.last_saved.borrow_mut() = None;
    w.export_image_to(path.clone(), target);
    let p2 = path.clone();
    wait(w, name, move |w| w.last_saved.borrow().as_deref() == Some(&p2));
    let bytes = std::fs::read(&path).unwrap();
    let fmt = match paper_mirror::core::filetype::detect(&bytes, "") {
        paper_mirror::core::filetype::FileKind::Image(f) => f,
        k => panic!("{name} is {k:?}"),
    };
    decode::decode(&bytes, fmt).unwrap()
}

/// PNG with the image's own metadata, as File > Export builds it.
fn png_target(w: &DocWindow) -> ImageTarget {
    w.image_target(SaveFormat::Png, 90).unwrap()
}

/// A 200 × 100 image: left half red, right half white, a blue square at 150..170.
fn sample() -> Frame {
    let mut f = Frame::new(200, 100, vec![0; 200 * 100 * 4]);
    for y in 0..100 {
        for x in 0..200 {
            let p = if (150..170).contains(&x) && (40..60).contains(&y) {
                [0, 0, 255, 255]
            } else if x < 100 {
                [220, 30, 30, 255]
            } else {
                [255, 255, 255, 255]
            };
            let i = (y * 200 + x) * 4;
            f.rgba[i..i + 4].copy_from_slice(&p);
        }
    }
    f
}

fn exif_with_location() -> Vec<u8> {
    use exif::{Field, In, Rational, Tag, Value};
    let r = |n: u32| Rational { num: n, denom: 1 };
    let fields = [
        Field { tag: Tag::Make, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"Paper Cam".to_vec()]) },
        Field { tag: Tag::Orientation, ifd_num: In::PRIMARY, value: Value::Short(vec![1]) },
        Field { tag: Tag::GPSLatitudeRef, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"N".to_vec()]) },
        Field { tag: Tag::GPSLatitude, ifd_num: In::PRIMARY, value: Value::Rational(vec![r(51), r(30), r(0)]) },
        Field { tag: Tag::GPSLongitudeRef, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"W".to_vec()]) },
        Field { tag: Tag::GPSLongitude, ifd_num: In::PRIMARY, value: Value::Rational(vec![r(0), r(7), r(30)]) },
    ];
    let mut w = exif::experimental::Writer::new();
    for f in &fields {
        w.push_field(f);
    }
    let mut out = Cursor::new(Vec::new());
    w.write(&mut out, true).unwrap();
    out.into_inner()
}

fn info_value(w: &DocWindow, key: &str) -> Option<String> {
    w.ui.get_info_rows().iter().find(|r| r.key == key).map(|r| r.value.to_string())
}

fn open(path: PathBuf) -> Rc<DocWindow> {
    let w = DocWindow::open_new(None).unwrap();
    w.ui.window().set_size(slint::LogicalSize::new(1000.0, 800.0));
    w.open_path(path);
    wait(&w, "document", |w| w.has_document());
    w
}

#[test]
fn images_end_to_end() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let dir = std::env::temp_dir().join(format!("pm-ui-images-{}", std::process::id()));
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

    let png = dir.join("photo.png");
    let opts = EncodeOptions { exif: Some(exif_with_location()), dpi: (144.0, 144.0), ..Default::default() };
    std::fs::write(&png, encode::encode(&[sample()], SaveFormat::Png, &opts).unwrap()).unwrap();

    // --- Open: one page at 0.75 pt per pixel ----------------------------------------
    let w = open(png.clone());
    assert!(w.is_image());
    assert!(w.ui.get_is_image());
    let s = w.page_sizes()[0];
    assert!((s.w - 200.0 * PT_PER_PX).abs() < 0.01 && (s.h - 100.0 * PT_PER_PX).abs() < 0.01);

    // --- Inspector: image facts; location hidden until asked -----------------------
    assert!(w.command("inspector"));
    wait(&w, "inspector rows", |w| info_value(w, "Dimensions").is_some());
    assert_eq!(info_value(&w, "Dimensions").as_deref(), Some("200 × 100 pixels"));
    assert_eq!(info_value(&w, "Resolution").as_deref(), Some("144 pixels/inch"));
    assert_eq!(info_value(&w, "Camera make").as_deref(), Some("Paper Cam"));
    assert!(w.ui.get_has_location());
    assert!(info_value(&w, "GPS").unwrap().starts_with("Hidden"));
    assert!(w.command("show-location"));
    wait(&w, "location shown", |w| info_value(w, "GPS").is_some_and(|v| v.contains("51.50000° N")));
    assert!(info_value(&w, "GPS").unwrap().contains("0.12500° W"));
    assert!(w.command("remove-location"));
    assert!(w.is_dirty());
    assert!(w.ui.get_location_removed());

    // --- Markup is baked into the saved pixels; the original is kept ---------------
    let style =
        Style { stroke: Color::rgb(0, 160, 0), fill: Some(Color::rgb(0, 160, 0)), width: 1.0, ..Default::default() };
    let r = Rect::new(20.0 * PT_PER_PX, 70.0 * PT_PER_PX, 40.0 * PT_PER_PX, 20.0 * PT_PER_PX);
    w.add_annotation(Annotation::new(0, 0, r, Shape::Rectangle, style));
    *w.last_saved.borrow_mut() = None;
    assert!(w.command("save"));
    wait(&w, "save", |w| w.last_saved.borrow().is_some() && !w.is_dirty());
    let saved = decode::decode(&std::fs::read(&png).unwrap(), ImageFormat::Png).unwrap();
    let f = &saved.frames[0];
    assert_eq!((f.width, f.height), (200, 100));
    assert_eq!(f.pixel(40, 80)[..3], [0, 160, 0], "the rectangle is in the pixels");
    assert_eq!(f.pixel(5, 5), [220, 30, 30, 255], "elsewhere unchanged");
    assert_eq!(f.pixel(160, 50), [0, 0, 255, 255]);
    let exif = saved.info.exif.as_deref().expect("EXIF kept");
    assert!(!metadata::has_location(exif), "location removed on save");
    assert_eq!(metadata::orientation(exif), Some(1));
    assert_eq!(saved.info.dpi, (144.0, 144.0));
    assert!(!w.ui.get_has_location(), "location is gone from the open image too");
    assert!(paper_mirror::storage::originals::original_for(&png).is_some(), "original preserved");
    // The markup stays editable after saving.
    assert_eq!(w.annotations().len(), 1);

    // --- Flip, then undo ----------------------------------------------------------
    assert!(w.command("flip-horizontal"));
    wait(&w, "flip", |w| w.ui.get_busy_text().is_empty() && w.ui.get_can_undo() && w.is_dirty());
    let d = export(&w, &dir, "flipped.png", png_target(&w));
    assert_eq!(d.frames[0].pixel(195, 5)[..3], [220, 30, 30], "red is now on the right");
    assert_eq!(d.frames[0].pixel(39, 50)[..3], [0, 0, 255], "blue square mirrored to 30..50");
    w.undo();
    wait(&w, "undo flip", |w| w.ui.get_busy_text().is_empty() && w.ui.get_can_redo());
    let d = export(&w, &dir, "unflipped.png", png_target(&w));
    assert_eq!(d.frames[0].pixel(5, 5)[..3], [220, 30, 30]);

    // --- Adjust Size: half the pixels; markup scales with the image -----------------
    assert!(w.command("adjust-size"));
    assert!(w.ui.get_size_visible());
    assert_eq!(w.ui.get_size_width().as_str(), "200");
    w.ui.set_size_width("100".into());
    w.size_edited("width");
    assert_eq!(w.ui.get_size_height().as_str(), "50", "proportional");
    w.size_confirm();
    wait(&w, "resize", |w| (w.page_sizes()[0].w - 100.0 * PT_PER_PX).abs() < 0.01);
    let a = &w.annotations()[0];
    assert!((a.bounds.w - 20.0 * PT_PER_PX).abs() < 0.01, "annotation scaled: {:?}", a.bounds);
    let d = export(&w, &dir, "half.png", png_target(&w));
    assert_eq!((d.frames[0].width, d.frames[0].height), (100, 50));
    assert_eq!(d.frames[0].pixel(2, 2)[..3], [220, 30, 30]);

    // Resolution only (no resampling): pixels stay, dpi changes, undo restores it.
    assert!(w.command("adjust-size"));
    w.ui.set_size_resample(false);
    w.size_edited("resample");
    w.ui.set_size_resolution("300".into());
    w.size_edited("resolution");
    w.size_confirm();
    let d = export(&w, &dir, "dpi.png", png_target(&w));
    assert_eq!((d.frames[0].width, d.info.dpi.0), (100, 300.0));
    w.undo();
    let d = export(&w, &dir, "dpi-undone.png", png_target(&w));
    assert_eq!(d.info.dpi.0, 144.0);

    // --- Adjust Color: previewed live, applied on Done ---------------------------------
    assert!(w.command("adjust-color"));
    assert!(w.ui.get_adjust_visible());
    w.ui.set_adjust_saturation(0.0);
    w.adjust_changed();
    w.adjust_done();
    assert!(!w.ui.get_adjust_visible());
    wait(&w, "colour", |w| w.ui.get_busy_text().is_empty());
    let d = export(&w, &dir, "gray.png", png_target(&w));
    let p = d.frames[0].pixel(2, 2);
    assert!(p[0].abs_diff(p[1]) <= 2 && p[1].abs_diff(p[2]) <= 2, "saturation 0 makes red gray: {p:?}");

    // --- Instant Alpha: drag on the white half, release to clear it ----------------
    assert!(w.command("instant-alpha"));
    assert_eq!(w.ui.get_tool().as_str(), "tool-alpha");
    let page = w.ui.get_pages().iter().find(|p| p.index == 0).unwrap();
    let k = page.width / (100.0 * PT_PER_PX);
    let at = |x: f32, y: f32| Point::new(page.x + x * PT_PER_PX * k, page.y + y * PT_PER_PX * k);
    w.pointer(0, at(90.0, 5.0), false, false, 0);
    let frac = w.alpha_selection_fraction().unwrap();
    assert!(frac > 0.3 && frac < 0.5, "white half minus the blue square: {frac}");
    assert!(w.ui.get_mask_visible());
    w.pointer(1, at(92.0, 6.0), false, false, 0);
    w.pointer(2, at(92.0, 6.0), false, false, 0);
    assert!(!w.ui.get_mask_visible());
    wait(&w, "alpha", |w| w.ui.get_busy_text().is_empty());
    let d = export(&w, &dir, "alpha.png", png_target(&w));
    assert_eq!(d.frames[0].pixel(90, 5)[3], 0, "white became transparent");
    assert_eq!(d.frames[0].pixel(5, 5)[3], 255, "the other half stays");
    assert_eq!(d.frames[0].pixel(80, 25)[3], 255, "the blue square stays");

    // --- Conversion: Save As JPEG makes it a JPEG document --------------------------
    let jpg = dir.join("photo.jpg");
    *w.last_saved.borrow_mut() = None;
    w.save_image_to(jpg.clone(), 92, AfterSave::Nothing);
    let j2 = jpg.clone();
    wait(&w, "jpeg", move |w| w.last_saved.borrow().as_deref() == Some(&j2));
    assert_eq!(w.doc_path().as_deref(), Some(jpg.as_path()));
    let d = decode::decode(&std::fs::read(&jpg).unwrap(), ImageFormat::Jpeg).unwrap();
    assert_eq!((d.frames[0].width, d.frames[0].height), (100, 50));
    // Transparency is flattened onto white.
    let p = d.frames[0].pixel(90, 5);
    assert!(p.iter().take(3).all(|c| *c > 240), "{p:?}");

    // --- Animated GIF: frames are pages; deleting one keeps the rest ----------------
    let mut frames = Vec::new();
    for i in 0..3u8 {
        let mut f = Frame::new(8, 8, vec![i * 80; 8 * 8 * 4]);
        f.rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
        f.delay_ms = 100 * (i as u32 + 1);
        frames.push(f);
    }
    let gif = dir.join("anim.gif");
    std::fs::write(&gif, encode::encode(&frames, SaveFormat::Gif, &Default::default()).unwrap()).unwrap();
    let g = open(gif.clone());
    assert_eq!(g.page_count(), 3);
    g.thumb_clicked(1, false, false);
    assert!(g.command("delete-pages"));
    wait(&g, "frame deleted", |w| w.page_count() == 2);
    *g.last_saved.borrow_mut() = None;
    assert!(g.command("save"));
    wait(&g, "gif saved", |w| w.last_saved.borrow().is_some() && !w.is_dirty());
    let d = decode::decode(&std::fs::read(&gif).unwrap(), ImageFormat::Gif).unwrap();
    assert_eq!(d.frames.len(), 2);
    assert_eq!((d.frames[0].delay_ms, d.frames[1].delay_ms), (100, 300), "delays follow their frames");

    // --- JPEG 2000 opens; it is read-only, so Save needs another format ------------
    let jp2 = dir.join("image.jp2");
    std::fs::copy("tests/fixtures/image.jp2", &jp2).unwrap();
    let j = open(jp2);
    assert!(j.is_image());
    assert_eq!(j.page_count(), 1);
    let d = export(&j, &dir, "from-jp2.png", png_target(&j));
    assert_eq!(d.frames[0].pixel(7, 3)[..3], [42, 24, 30]);

    // --- PDF page exported as an image, and an image inserted into a PDF ------------
    let pdf = dir.join("doc.pdf");
    std::fs::write(&pdf, fixtures::text_document()).unwrap();
    let p = open(pdf);
    assert!(!p.is_image());
    let t = ImageTarget {
        format: SaveFormat::Png,
        page: Some(0),
        options: EncodeOptions { dpi: (36.0, 36.0), ..Default::default() },
        delays: Vec::new(),
    };
    let d = export(&p, &dir, "page.png", t);
    assert_eq!((d.frames[0].width, d.frames[0].height), (306, 396), "US Letter at 36 dpi");
    let n = p.page_count();
    p.insert_from_file(Some(png.clone()));
    wait(&p, "image inserted", move |w| w.page_count() == n + 1);
    let inserted = p.page_sizes()[1];
    assert!((inserted.w - 150.0).abs() < 0.01, "an image page: {inserted:?}");

    // --- New from Clipboard opens the clipboard image (restoring the clipboard) ------
    let previous = paper_mirror::platform_windows::clipboard::get_text();
    let rgba = [10u8, 20, 30, 255].repeat(12);
    if paper_mirror::platform_windows::clipboard::set_image(4, 3, &rgba).is_ok() {
        let c = DocWindow::open_new(None).unwrap();
        assert!(c.command("new-from-clipboard"));
        wait(&c, "clipboard image", |w| w.has_document());
        assert!(c.is_image() && c.is_dirty() && c.doc_path().is_none());
        assert_eq!(c.page_sizes()[0].w, 4.0 * PT_PER_PX);
    }
    if let Some(t) = previous {
        let _ = paper_mirror::platform_windows::clipboard::set_text(&t);
    }
    drop(engine);
}
