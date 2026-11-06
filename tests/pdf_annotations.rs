//! M4 backend tests: annotation round trip, appearance, flattening, encryption and
//! permanent redaction.

use std::sync::Arc;

use paper_mirror::core::annot::{Annotation, Color, Shape, Style};
use paper_mirror::core::geometry::{Point, Rect};
use paper_mirror::fixtures::{self, NEEDLE};
use paper_mirror::pdf::annot_io::{self, SaveOptions};
use paper_mirror::pdf::document::PdfDoc;
use paper_mirror::pdf::ffi::{OpenError, Source};
use paper_mirror::pdf::{redact, security::Protection, text};

fn open(bytes: Vec<u8>) -> PdfDoc {
    PdfDoc::open(Source::Memory(Arc::new(bytes)), None).unwrap()
}

fn style(c: Color) -> Style {
    Style { stroke: c, width: 3.0, ..Style::default() }
}

fn sample_annotations() -> Vec<Annotation> {
    let red = Color::RED;
    let mut v = vec![
        Annotation::new(
            1,
            0,
            Rect::default(),
            Shape::Highlight { rects: vec![Rect::new(100.0, 90.0, 120.0, 16.0)] },
            style(Color::HIGHLIGHT_YELLOW),
        ),
        Annotation::new(
            2,
            0,
            Rect::default(),
            Shape::Underline { rects: vec![Rect::new(100.0, 120.0, 120.0, 16.0)] },
            style(red),
        ),
        Annotation::new(
            3,
            0,
            Rect::default(),
            Shape::StrikeOut { rects: vec![Rect::new(100.0, 140.0, 120.0, 16.0)] },
            style(red),
        ),
        Annotation::new(
            4,
            0,
            Rect::default(),
            Shape::Ink {
                strokes: vec![vec![Point::new(300.0, 300.0), Point::new(350.0, 330.0), Point::new(400.0, 300.0)]],
            },
            style(red),
        ),
        Annotation::new(
            5,
            0,
            Rect::default(),
            Shape::Line { from: Point::new(100.0, 400.0), to: Point::new(250.0, 450.0), arrow: true },
            style(red),
        ),
        Annotation::new(
            6,
            0,
            Rect::new(100.0, 500.0, 120.0, 80.0),
            Shape::Rectangle,
            Style { fill: Some(Color::rgb(0, 128, 255)), ..style(red) },
        ),
        Annotation::new(7, 0, Rect::new(260.0, 500.0, 120.0, 80.0), Shape::Oval, style(red)),
        Annotation::new(8, 0, Rect::new(420.0, 500.0, 100.0, 100.0), Shape::Star, style(red)),
        Annotation::new(
            9,
            0,
            Rect::new(100.0, 620.0, 160.0, 60.0),
            Shape::SpeechBubble { tail: Point::new(90.0, 720.0) },
            style(red),
        ),
        Annotation::new(
            10,
            0,
            Rect::new(300.0, 620.0, 200.0, 60.0),
            Shape::TextBox { text: "Hello Grüße 你好".into() },
            Style { font_size: 16.0, ..style(red) },
        ),
        Annotation::new(
            11,
            0,
            Rect::new(540.0, 40.0, 20.0, 20.0),
            Shape::Note { text: "A sticky note".into() },
            style(red),
        ),
        Annotation::new(
            12,
            0,
            Rect::new(420.0, 380.0, 100.0, 100.0),
            Shape::ZoomLens { source: Rect::new(72.0, 60.0, 40.0, 40.0) },
            style(Color::BLACK),
        ),
        Annotation::new(13, 0, Rect::new(300.0, 200.0, 40.0, 40.0), Shape::RoundedRectangle, style(red)),
    ];
    // Page 2 of the fixture is rotated 90 degrees.
    v.push(Annotation::new(14, 1, Rect::new(50.0, 50.0, 200.0, 100.0), Shape::Rectangle, style(red)));
    v.push(Annotation::new(
        15,
        1,
        Rect::new(300.0, 300.0, 180.0, 40.0),
        Shape::TextBox { text: "Rotated page".into() },
        style(red),
    ));
    v
}

#[test]
fn annotations_round_trip_through_save_and_reopen() {
    let d = open(fixtures::text_document());
    let annots = sample_annotations();
    let bytes = annot_io::save(&d, &annots, &SaveOptions::default()).unwrap();

    // Standard subtypes and appearance streams are present for other viewers.
    let lo = lopdf::Document::load_mem(&bytes).unwrap();
    let mut subtypes = Vec::new();
    for obj in lo.objects.values() {
        if let Ok(dict) = obj.as_dict()
            && dict.get(b"Type").and_then(|t| t.as_name()).ok() == Some(b"Annot".as_slice())
        {
            subtypes.push(String::from_utf8_lossy(dict.get(b"Subtype").unwrap().as_name().unwrap()).to_string());
            assert!(dict.has(b"AP"), "every annotation has an appearance");
            assert!(dict.has(annot_io::PM_KEY.as_bytes()));
        }
    }
    subtypes.sort();
    for s in [
        "Highlight",
        "Underline",
        "StrikeOut",
        "Ink",
        "Line",
        "Square",
        "Circle",
        "Polygon",
        "FreeText",
        "Text",
        "Stamp",
    ] {
        assert!(subtypes.iter().any(|x| x == s), "missing /{s} in {subtypes:?}");
    }

    // Reopen: annotations come back editable, in display space, and are removed from
    // PDFium's page so they are not drawn twice.
    let d2 = open(bytes);
    let mut next = 100;
    let back = annot_io::extract(&d2, &mut next);
    assert_eq!(back.len(), annots.len());
    for (orig, got) in annots.iter().zip(back.iter()) {
        assert_eq!(orig.page, got.page);
        assert_eq!(orig.shape.name(), got.shape.name());
        let (a, b) = (orig.bounds, got.bounds);
        assert!(
            (a.x - b.x).abs() < 0.05
                && (a.y - b.y).abs() < 0.05
                && (a.w - b.w).abs() < 0.05
                && (a.h - b.h).abs() < 0.05,
            "{}: {a:?} vs {b:?}",
            orig.shape.name()
        );
        assert_eq!(orig.shape.text(), got.shape.text());
    }
    // A second save after extraction does not duplicate anything.
    let again = annot_io::save(&d2, &back, &SaveOptions::default()).unwrap();
    let d3 = open(again);
    assert_eq!(annot_io::extract(&d3, &mut next).len(), annots.len());
}

fn pixel(d: &PdfDoc, page: usize, x: f32, y: f32) -> [u8; 4] {
    let (w, h, px) = d.render_to_vec(page, d.page_sizes()[page].0 as u32).unwrap();
    let (xi, yi) = (x.round() as u32, y.round() as u32);
    assert!(xi < w && yi < h);
    let i = ((yi * w + xi) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

#[test]
fn saved_appearances_render_in_place() {
    let d = open(fixtures::text_document());
    let annots = sample_annotations();
    let bytes = annot_io::save(&d, &annots, &SaveOptions::default()).unwrap();
    let saved = open(bytes);
    // Filled blue rectangle (page 1, display space at 72 dpi = 1 px per point).
    let p = pixel(&saved, 0, 160.0, 540.0);
    assert!(p[2] > 200 && p[0] < 60, "rectangle fill: {p:?}");
    // Highlight is yellow-ish over white.
    let p = pixel(&saved, 0, 110.0, 98.0);
    assert!(p[0] > 200 && p[1] > 150 && p[2] < 60, "highlight: {p:?}");
    // The rotated page's rectangle stroke appears where it was drawn in display space.
    let p = pixel(&saved, 1, 50.0, 100.0);
    assert!(p[0] > 180 && p[1] < 120, "rotated page stroke: {p:?}");
    // Text box glyphs render (some dark-red pixels inside the box).
    let (w, _h, px) = saved.render_to_vec(0, 612).unwrap();
    let red_in_box = (620..680)
        .flat_map(|y| (300..500).map(move |x| (x, y)))
        .filter(|(x, y)| {
            let i = ((*y as u32 * w + *x as u32) * 4) as usize;
            px[i] > 150 && px[i + 1] < 120 && px[i + 2] < 120
        })
        .count();
    assert!(red_in_box > 30, "text glyphs drawn: {red_in_box}");
}

#[test]
fn flattening_merges_annotations_into_content() {
    let d = open(fixtures::text_document());
    let annots = sample_annotations();
    let bytes = annot_io::save(&d, &annots, &SaveOptions { flatten: true, ..Default::default() }).unwrap();
    let f = open(bytes);
    let mut next = 0;
    assert!(annot_io::extract(&f, &mut next).is_empty(), "no editable annotations remain");
    let p = pixel(&f, 0, 160.0, 540.0);
    assert!(p[2] > 200 && p[0] < 60, "flattened rectangle still drawn: {p:?}");
}

#[test]
fn saving_with_protection_and_preserving_encryption() {
    let d = open(fixtures::text_document());
    let opts = SaveOptions {
        protection: Some(Protection {
            open_password: "pw1".into(),
            allow_printing: true,
            allow_copying: true,
            allow_editing: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let bytes = annot_io::save(&d, &sample_annotations(), &opts).unwrap();
    let arc = Arc::new(bytes);
    assert!(matches!(PdfDoc::open(Source::Memory(arc.clone()), None), Err(OpenError::NeedsPassword)));
    let e = PdfDoc::open(Source::Memory(arc), Some("pw1")).unwrap();
    let mut next = 0;
    assert_eq!(annot_io::extract(&e, &mut next).len(), sample_annotations().len());
    // Re-saving an encrypted document keeps it encrypted with the same password.
    let resaved =
        annot_io::save(&e, &[], &SaveOptions { reopen_password: Some("pw1".into()), ..Default::default() }).unwrap();
    let arc = Arc::new(resaved);
    assert!(matches!(PdfDoc::open(Source::Memory(arc.clone()), None), Err(OpenError::NeedsPassword)));
    assert!(PdfDoc::open(Source::Memory(arc), Some("pw1")).is_ok());
}

#[test]
fn redaction_removes_text_but_keeps_the_rest_of_the_line() {
    let d = open(fixtures::text_document());
    let hit = text::search_page(&d, 0, NEEDLE, Default::default()).remove(0);
    let area = hit.rects[0];
    let rep = redact::redact_page(&d, 0, &[area]).unwrap();
    assert_eq!(rep.chars_removed, NEEDLE.len());
    assert_eq!(rep.text_objects_rebuilt, 1);
    let saved = open(d.doc.save_to_vec(false).unwrap());
    let t = text::page_text(&saved, 0);
    assert!(!t.contains(NEEDLE), "redacted word is gone: {t}");
    assert!(t.contains("keeper counts ships"), "the rest of the line survives: {t}");
    assert!(t.contains("Quick brown foxes"));
    // The area is black.
    let c = area.center();
    let p = pixel(&saved, 0, c.x, c.y);
    assert!(p[0] < 30 && p[1] < 30 && p[2] < 30, "{p:?}");
    // Other pages are untouched.
    assert!(text::page_text(&saved, 1).contains(NEEDLE));
}

#[test]
fn redaction_blacks_out_image_pixels_in_the_data() {
    let d = open(fixtures::image_document());
    // The image occupies PDF x 156..456, y 300..500 on a 792pt page: display y 292..492.
    // Redact a band across its top-left gradient area.
    let area = Rect::new(170.0, 300.0, 60.0, 40.0);
    let rep = redact::redact_page(&d, 0, &[area]).unwrap();
    assert_eq!(rep.images_modified, 1);
    let saved_bytes = d.doc.save_to_vec(false).unwrap();
    // Check the image XObject's own pixels (not just the rendering): extract it with
    // lopdf and inspect the decoded samples.
    let lo = lopdf::Document::load_mem(&saved_bytes).unwrap();
    let (img_w, img_h, data) = lo
        .objects
        .values()
        .find_map(|o| {
            let s = o.as_stream().ok()?;
            (s.dict.get(b"Subtype").ok()?.as_name().ok()? == b"Image").then(|| {
                let w = s.dict.get(b"Width").unwrap().as_i64().unwrap() as usize;
                let h = s.dict.get(b"Height").unwrap().as_i64().unwrap() as usize;
                (w, h, s.decompressed_content().unwrap_or_else(|_| s.content.clone()))
            })
        })
        .expect("image survives");
    let bpp = data.len() / (img_w * img_h);
    let at = |x: usize, y: usize| &data[(y * img_w + x) * bpp..(y * img_w + x) * bpp + 3];
    // Image pixel (20, 10) lies under the redaction; (250, 150) does not.
    assert_eq!(at(20, 10), &[0, 0, 0], "redacted pixels are black in the image data");
    assert_ne!(at(250, 150), &[0, 0, 0]);
    // Text above the image is unaffected.
    assert!(text::page_text(&open(saved_bytes), 0).contains(NEEDLE));
}

/// Writes the annotated sample to PAPER_MIRROR_SAMPLE_OUT for visual inspection.
#[test]
fn write_visual_sample_when_requested() {
    if let Some(out) = std::env::var_os("PAPER_MIRROR_SAMPLE_OUT") {
        let d = open(fixtures::text_document());
        std::fs::write(out, annot_io::save(&d, &sample_annotations(), &SaveOptions::default()).unwrap()).unwrap();
    }
}
