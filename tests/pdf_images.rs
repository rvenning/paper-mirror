//! M7 backend tests: images as PDF pages, exact pixel round trips, page operations
//! on images, markup baked into pixels, and JPEG 2000 through PDFium.

use paper_mirror::core::annot::{Annotation, Color, Shape, Style};
use paper_mirror::core::filetype::ImageFormat;
use paper_mirror::core::geometry::Rect;
use paper_mirror::core::pagemap::PageOp;
use paper_mirror::images::{Frame, PT_PER_PX};
use paper_mirror::pdf::{imagedoc, pageops};

/// Deterministic noise with a transparent band, so any resampling shows up.
fn noise(w: u32, h: u32, alpha: bool) -> Frame {
    let mut s = 0x2545_F491u32;
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let a = if !alpha {
                255
            } else if x < 3 {
                0
            } else if y < 2 {
                128
            } else {
                255
            };
            v.extend_from_slice(&[s as u8, (s >> 8) as u8, (s >> 16) as u8, a]);
        }
    }
    Frame::new(w, h, v)
}

/// Equal where visible; fully transparent pixels only need to stay transparent, and
/// half-transparent ones may differ by rounding.
fn assert_same(a: &Frame, b: &Frame) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    for (i, (p, q)) in a.rgba.as_chunks::<4>().0.iter().zip(b.rgba.as_chunks::<4>().0.iter()).enumerate() {
        match q[3] {
            0 => assert_eq!(p[3], 0, "pixel {i}"),
            255 => assert_eq!(p, q, "pixel {i}"),
            _ => assert!(p.iter().zip(q).all(|(x, y)| x.abs_diff(*y) <= 2), "pixel {i}: {p:?} vs {q:?}"),
        }
    }
}

#[test]
fn image_pages_round_trip_pixel_for_pixel() {
    for alpha in [false, true] {
        let f = noise(37, 23, alpha);
        let doc = imagedoc::build(vec![f.clone()]).unwrap();
        assert_eq!(doc.page_count(), 1);
        let (w, h) = doc.page_sizes()[0];
        assert!((w - 37.0 * PT_PER_PX).abs() < 1e-3 && (h - 23.0 * PT_PER_PX).abs() < 1e-3);
        assert_eq!(imagedoc::pixel_size(&doc, 0), Some((37, 23)));
        let back = imagedoc::rasterise(&doc, 0, false).unwrap();
        assert_same(&back, &f);
        // Saved and reopened as a PDF, the page still holds the same pixels.
        let bytes = doc.doc.save_to_vec(false).unwrap();
        let re = paper_mirror::pdf::document::PdfDoc::open(
            paper_mirror::pdf::ffi::Source::Memory(std::sync::Arc::new(bytes)),
            None,
        )
        .unwrap();
        assert_same(&imagedoc::rasterise(&re, 0, false).unwrap(), &f);
    }
}

#[test]
fn rotate_and_pixel_aligned_crop_act_on_pixels() {
    let f = noise(20, 10, false);
    let mut doc = imagedoc::build(vec![f.clone()]).unwrap();
    pageops::apply(&mut doc, &PageOp::Rotate { pages: vec![0], quarter_turns: 1 }, None).unwrap();
    let r = imagedoc::rasterise(&doc, 0, false).unwrap();
    assert_eq!((r.width, r.height), (10, 20));
    // Clockwise: the top-left source pixel ends up top-right.
    assert_eq!(r.pixel(9, 0), f.pixel(0, 0));
    assert_eq!(r.pixel(0, 19), f.pixel(19, 9));

    let mut doc = imagedoc::build(vec![f.clone()]).unwrap();
    // Keep pixels x 4..12, y 2..7 (display points are pixels × 0.75).
    let rect = Rect::new(4.0 * PT_PER_PX, 2.0 * PT_PER_PX, 8.0 * PT_PER_PX, 5.0 * PT_PER_PX);
    pageops::apply(&mut doc, &PageOp::Crop { page: 0, rect }, None).unwrap();
    let c = imagedoc::rasterise(&doc, 0, false).unwrap();
    assert_eq!((c.width, c.height), (8, 5));
    for y in 0..5 {
        for x in 0..8 {
            assert_eq!(c.pixel(x, y), f.pixel(x + 4, y + 2));
        }
    }
}

#[test]
fn markup_is_baked_only_where_drawn() {
    let f = noise(60, 40, false);
    let doc = imagedoc::build(vec![f.clone()]).unwrap();
    let style =
        Style { stroke: Color::rgb(255, 0, 0), fill: Some(Color::rgb(0, 0, 255)), width: 1.0, ..Default::default() };
    // A filled rectangle over pixels 10..30 × 10..20.
    let rect = Rect::new(10.0 * PT_PER_PX, 10.0 * PT_PER_PX, 20.0 * PT_PER_PX, 10.0 * PT_PER_PX);
    let a = Annotation::new(1, 0, rect, Shape::Rectangle, style);
    let frames = imagedoc::rasterise_with_annotations(&doc, &[a]).unwrap();
    assert_eq!(frames.len(), 1);
    let out = &frames[0];
    assert_eq!((out.width, out.height), (60, 40));
    // Inside the fill: blue.
    let p = out.pixel(20, 15);
    assert!(p[2] > 200 && p[0] < 60, "{p:?}");
    // Far from the shape: untouched.
    for (x, y) in [(0, 0), (59, 39), (45, 5), (5, 35)] {
        assert_eq!(out.pixel(x, y), f.pixel(x, y), "({x}, {y})");
    }
}

#[test]
fn replace_page_and_frames() {
    let a = noise(10, 10, false);
    let b = noise(16, 8, true);
    let mut doc = imagedoc::build(vec![a.clone(), b.clone()]).unwrap();
    assert_eq!(doc.page_count(), 2);
    assert_same(&imagedoc::rasterise(&doc, 1, false).unwrap(), &b);
    let c = noise(5, 7, false);
    imagedoc::replace_page(&mut doc, 0, c.clone()).unwrap();
    assert_eq!(doc.page_count(), 2);
    assert_same(&imagedoc::rasterise(&doc, 0, false).unwrap(), &c);
    assert_same(&imagedoc::rasterise(&doc, 1, false).unwrap(), &b);
}

fn expected_jp2(x: u32, y: u32) -> [u8; 3] {
    [((x * 6) % 256) as u8, ((y * 8) % 256) as u8, ((x + y) * 3 % 256) as u8]
}

#[test]
fn jpeg2000_decodes_through_pdfium() {
    for name in ["image.jp2", "image.j2k", "image-alpha.jp2"] {
        let bytes = std::fs::read(format!("tests/fixtures/{name}")).unwrap();
        let kind = paper_mirror::core::filetype::detect(&bytes, "");
        assert_eq!(kind, paper_mirror::core::filetype::FileKind::Image(ImageFormat::Jpeg2000), "{name}");
        let d = imagedoc::decode_jpx(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let f = &d.frames[0];
        assert_eq!((f.width, f.height), (40, 30), "{name}");
        for (x, y) in [(0, 0), (7, 3), (39, 29), (20, 15)] {
            let p = f.pixel(x, y);
            if name.contains("alpha") && x < 5 {
                assert_eq!(p[3], 0, "{name} ({x}, {y})");
            } else {
                assert_eq!([p[0], p[1], p[2]], expected_jp2(x, y), "{name} ({x}, {y})");
                assert_eq!(p[3], 255, "{name} ({x}, {y})");
            }
        }
    }
    assert!(imagedoc::decode_jpx(b"\xFF\x4F\xFF\x51garbage").is_err());
}
