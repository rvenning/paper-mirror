//! Integration tests for the PDFium backend against the self-created corpus.

use paper_mirror::fixtures;
use paper_mirror::pdf::document::PdfDoc;
use paper_mirror::pdf::ffi::{OpenError, Source};
use std::sync::Arc;

fn open(bytes: Vec<u8>) -> PdfDoc {
    PdfDoc::open(Source::Memory(Arc::new(bytes)), None).expect("open fixture")
}

#[test]
fn opens_and_reports_rotated_sizes() {
    let d = open(fixtures::text_document());
    assert_eq!(d.page_count(), 3);
    assert_eq!(d.page_sizes()[0], (612.0, 792.0));
    // /Rotate 90 swaps the displayed width and height.
    assert_eq!(d.page_sizes()[1], (792.0, 612.0));
    assert_eq!(d.page_sizes()[2], (842.0, 595.0));
}

#[test]
fn renders_ink_onto_white() {
    let d = open(fixtures::text_document());
    let (w, h, px) = d.render_to_vec(0, 200).unwrap();
    assert_eq!(w, 200);
    assert_eq!(h, (792.0f32 * 200.0 / 612.0).round() as u32);
    let dark = px.chunks_exact(4).filter(|p| p[0] < 128 && p[1] < 128 && p[2] < 128).count();
    let white = px.chunks_exact(4).filter(|p| p[0] == 255 && p[1] == 255 && p[2] == 255).count();
    assert!(dark > 50, "expected some text pixels, got {dark}");
    assert!(white > px.len() / 4 / 2, "page background should be white");
}

#[test]
fn clipped_tile_matches_full_render() {
    let d = open(fixtures::text_document());
    let (w, _h, full) = d.render_to_vec(0, 612).unwrap();
    let scale = 1.0;
    let (tw, th) = (100u32, 60u32);
    let (ox, oy) = (50u32, 60u32);
    let mut tile = vec![0u8; (tw * th * 4) as usize];
    let target = paper_mirror::pdf::document::RenderTarget { pixels: &mut tile, width: tw, height: th };
    assert!(d.render(0, scale, ox as f32, oy as f32, target, true));
    let mut diff = 0u64;
    for y in 0..th {
        for x in 0..tw {
            let a = ((y * tw + x) * 4) as usize;
            let b = (((y + oy) * w + (x + ox)) * 4) as usize;
            diff += (tile[a] as i32 - full[b] as i32).unsigned_abs() as u64;
        }
    }
    let mean = diff as f64 / (tw * th) as f64;
    assert!(mean < 2.0, "tile differs from full render by {mean}");
}

#[test]
fn rejects_malformed_and_non_pdf() {
    for bytes in [fixtures::malformed_document(), fixtures::not_a_pdf(), Vec::new()] {
        match PdfDoc::open(Source::Memory(Arc::new(bytes)), None) {
            Err(OpenError::Malformed) => {}
            Err(e) => panic!("unexpected error {e:?}"),
            Ok(_) => panic!("garbage opened as a PDF"),
        }
    }
}

#[test]
fn missing_file_is_an_io_error() {
    let r = PdfDoc::open(Source::File("Z:/definitely/missing/file.pdf".into()), None);
    assert!(matches!(r, Err(OpenError::Io(_))));
}

#[test]
fn file_source_with_unicode_path() {
    let dir = std::env::temp_dir().join("paper-mirror-tests").join("ünïcödé — 文書");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("päge test.pdf");
    std::fs::write(&p, fixtures::text_document()).unwrap();
    let d = PdfDoc::open(Source::File(p.clone()), None).unwrap();
    assert_eq!(d.page_count(), 3);
    drop(d);
    std::fs::remove_file(p).unwrap();
}

#[test]
fn save_round_trip_preserves_pages() {
    let d = open(fixtures::text_document());
    let bytes = d.doc.save_to_vec(false).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    let d2 = open(bytes);
    assert_eq!(d2.page_count(), 3);
    assert_eq!(d2.page_sizes(), d.page_sizes());
}

#[test]
fn display_pdf_coordinates_round_trip_with_rotation() {
    let d = open(fixtures::text_document());
    for page in 0..3 {
        d.with_page(page, |p| {
            let (px, py) = p.display_to_pdf(100.0, 50.0);
            let (dx, dy) = p.pdf_to_display(px, py);
            assert!((dx - 100.0).abs() < 0.01 && (dy - 50.0).abs() < 0.01, "page {page}: {dx},{dy}");
        })
        .unwrap();
    }
    // Unrotated page: display y grows downwards, PDF y upwards.
    d.with_page(0, |p| {
        let (px, py) = p.display_to_pdf(10.0, 20.0);
        assert!((px - 10.0).abs() < 0.01 && (py - 772.0).abs() < 0.01, "{px},{py}");
    });
}

#[test]
fn metadata_is_readable() {
    let d = open(fixtures::text_document());
    assert_eq!(d.doc.meta("Title"), "Paper Mirror fixture");
    assert_eq!(d.doc.meta("Author"), "Paper Mirror tests");
    assert_eq!(d.doc.file_version(), Some(17));
}
