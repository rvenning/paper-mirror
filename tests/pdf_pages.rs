//! M6 backend tests: page organisation, effects and file-size reduction.

use std::sync::Arc;

use paper_mirror::core::geometry::{Rect, Size};
use paper_mirror::core::pagemap::PageOp;
use paper_mirror::fixtures::{self, Builder};
use paper_mirror::pdf::annot_io::{self, SaveOptions};
use paper_mirror::pdf::document::PdfDoc;
use paper_mirror::pdf::effects::Effect;
use paper_mirror::pdf::ffi::Source;
use paper_mirror::pdf::{pageops, text};

fn open(bytes: Vec<u8>) -> PdfDoc {
    PdfDoc::open(Source::Memory(Arc::new(bytes)), None).unwrap()
}

/// Which "Fixture page N" title each page carries (reading order varies with rotation).
fn first_lines(d: &PdfDoc) -> Vec<String> {
    (0..d.page_count())
        .map(|p| {
            let t = text::page_text(d, p);
            t.lines().map(str::trim).find(|l| l.starts_with("Fixture page")).unwrap_or("").to_string()
        })
        .collect()
}

fn reopen(d: &PdfDoc) -> PdfDoc {
    open(d.doc.save_to_vec(false).unwrap())
}

#[test]
fn rotate_delete_move() {
    let mut d = open(fixtures::many_pages(5));
    pageops::apply(&mut d, &PageOp::Rotate { pages: vec![0, 2], quarter_turns: 1 }, None).unwrap();
    assert_eq!(d.page_sizes()[0], (792.0, 612.0));
    assert_eq!(d.page_sizes()[1], (612.0, 792.0));
    pageops::apply(&mut d, &PageOp::Rotate { pages: vec![0], quarter_turns: -1 }, None).unwrap();
    assert_eq!(d.page_sizes()[0], (612.0, 792.0));

    pageops::apply(&mut d, &PageOp::Delete { pages: vec![1, 3] }, None).unwrap();
    let d = reopen(&d);
    assert_eq!(first_lines(&d), vec!["Fixture page 1", "Fixture page 3", "Fixture page 5"]);
    let mut d = d;
    // Move the last page to the front.
    pageops::apply(&mut d, &PageOp::Move { pages: vec![2], dest: 0 }, None).unwrap();
    let d = reopen(&d);
    assert_eq!(first_lines(&d), vec!["Fixture page 5", "Fixture page 1", "Fixture page 3"]);
    // The remaining rotated page (originally 3) kept its rotation through the edits.
    assert_eq!(d.page_sizes()[2], (792.0, 612.0));
    let mut d = d;
    assert!(
        pageops::apply(&mut d, &PageOp::Delete { pages: vec![0, 1, 2] }, None).is_err(),
        "cannot delete every page"
    );
}

#[test]
fn insert_blank_and_combine() {
    let mut d = open(fixtures::many_pages(2));
    pageops::apply(&mut d, &PageOp::Insert { at: 1, count: 1, blank: Some(Size::new(500.0, 400.0)) }, None).unwrap();
    assert_eq!(d.page_count(), 3);
    assert_eq!(d.page_sizes()[1], (500.0, 400.0));
    let other = open(fixtures::outline_document());
    pageops::apply(&mut d, &PageOp::Insert { at: 3, count: other.page_count(), blank: None }, Some(&other)).unwrap();
    let d = reopen(&d);
    assert_eq!(d.page_count(), 8);
    assert_eq!(first_lines(&d)[3], "Fixture page 1");
    assert!(text::page_text(&d, 7).contains("Fixture page 5"));
}

#[test]
fn crop_sets_the_visible_area() {
    let mut d = open(fixtures::text_document());
    pageops::apply(&mut d, &PageOp::Crop { page: 0, rect: Rect::new(50.0, 40.0, 300.0, 200.0) }, None).unwrap();
    let (w, h) = d.page_sizes()[0];
    assert!((w - 300.0).abs() < 0.5 && (h - 200.0).abs() < 0.5, "{w}x{h}");
    // The title, which was at display (72, ~72), is now near the crop's top-left.
    let hit = text::search_page(&d, 0, "Fixture", Default::default());
    let r = hit[0].rects[0];
    assert!(r.x < 40.0 && r.y < 50.0, "{r:?}");
    let saved = reopen(&d);
    assert!((saved.page_sizes()[0].0 - 300.0).abs() < 0.5);
}

#[test]
fn effects_change_colours_but_keep_text() {
    let d = open(fixtures::image_document());
    let bytes = annot_io::save(&d, &[], &SaveOptions { effect: Some(Effect::GrayTone), ..Default::default() }).unwrap();
    let g = open(bytes);
    assert!(text::page_text(&g, 0).contains(fixtures::NEEDLE), "text stays text");
    let (w, h, px) = g.render_to_vec(0, 612).unwrap();
    let coloured = px.as_chunks::<4>().0.iter().filter(|p| (p[0] as i32 - p[2] as i32).abs() > 12).count();
    assert!(coloured < (w * h) as usize / 1000, "gray tone leaves no colour: {coloured}");
    // The open document itself is untouched.
    let (_, _, orig) = d.render_to_vec(0, 612).unwrap();
    assert!(orig.as_chunks::<4>().0.iter().any(|p| (p[0] as i32 - p[2] as i32).abs() > 40));
}

#[test]
fn reduce_file_size_downsamples_large_images() {
    let mut b = Builder::new();
    // 1500x1000 pixels shown at 300x200 points: 360 ppi.
    b.image_page(1500, 1000, (156.0, 300.0, 300.0, 200.0), "Big image");
    let big = fixtures::to_bytes(b.finish(lopdf::dictionary! {}));
    let d = open(big.clone());
    let small = annot_io::save(&d, &[], &SaveOptions { reduce_size: true, ..Default::default() }).unwrap();
    assert!(small.len() * 4 < big.len(), "{} -> {}", big.len(), small.len());
    let s = open(small);
    assert!(text::page_text(&s, 0).contains("Big image"));
    // The picture still renders where it was: the black centre square is dark.
    let (w, _h, px) = s.render_to_vec(0, 612).unwrap();
    let i = ((392 * w + 306) * 4) as usize;
    assert!(px[i] < 60, "centre of the image is still black: {:?}", &px[i..i + 3]);
}
