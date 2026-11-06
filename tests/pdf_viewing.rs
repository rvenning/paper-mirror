//! Backend tests for M2 viewing features: search, outline, links, document info.

use std::sync::Arc;

use paper_mirror::core::geometry::Point;
use paper_mirror::fixtures::{self, NEEDLE};
use paper_mirror::pdf::document::PdfDoc;
use paper_mirror::pdf::ffi::Source;
use paper_mirror::pdf::outline::{self, LinkTarget};
use paper_mirror::pdf::text::{self, SearchOptions};

fn open(bytes: Vec<u8>) -> PdfDoc {
    PdfDoc::open(Source::Memory(Arc::new(bytes)), None).unwrap()
}

#[test]
fn search_finds_needle_on_every_page_with_sane_rects() {
    let d = open(fixtures::text_document());
    for page in 0..3 {
        let hits = text::search_page(&d, page, NEEDLE, SearchOptions::default());
        assert_eq!(hits.len(), 1, "page {page}");
        let h = &hits[0];
        assert_eq!(h.len, NEEDLE.len());
        assert!(!h.rects.is_empty());
        let (pw, ph) = d.page_sizes()[page];
        for r in &h.rects {
            assert!(
                r.x >= 0.0 && r.y >= 0.0 && r.right() <= pw + 0.5 && r.bottom() <= ph + 0.5,
                "{r:?} outside page {page}"
            );
            assert!(r.w > 1.0 && r.h > 1.0);
        }
        assert!(h.context.contains(NEEDLE), "context: {}", h.context);
    }
    // Unrotated page 1: text is near the top (display y small), starting at x = 72.
    let r = text::search_page(&d, 0, NEEDLE, SearchOptions::default())[0].rects[0];
    assert!(r.y < 150.0 && r.x > 72.0, "{r:?}");
    // Page 2 is rotated 90 degrees clockwise: the text runs top-to-bottom near the right edge.
    let r = text::search_page(&d, 1, NEEDLE, SearchOptions::default())[0].rects[0];
    assert!(r.h > r.w, "rotated match should be tall: {r:?}");
    assert!(r.x > 792.0 / 2.0, "{r:?}");
}

#[test]
fn search_options() {
    let d = open(fixtures::text_document());
    let up = NEEDLE.to_uppercase();
    assert_eq!(text::search_page(&d, 0, &up, SearchOptions::default()).len(), 1, "case-insensitive by default");
    assert_eq!(text::search_page(&d, 0, &up, SearchOptions { match_case: true, whole_word: false }).len(), 0);
    assert_eq!(text::search_page(&d, 0, "light", SearchOptions { match_case: false, whole_word: true }).len(), 0);
    assert_eq!(text::search_page(&d, 0, "light", SearchOptions::default()).len(), 1);
    assert!(text::search_page(&d, 0, "", SearchOptions::default()).is_empty());
    assert!(text::search_page(&d, 0, "zzzz-not-there", SearchOptions::default()).is_empty());
    // "a" appears many times.
    assert!(text::search_page(&d, 0, "a", SearchOptions::default()).len() > 5);
}

#[test]
fn page_text_extraction() {
    let d = open(fixtures::text_document());
    let t = text::page_text(&d, 0);
    assert!(t.contains("Fixture page 1"), "{t}");
    assert!(t.contains("Quick brown foxes"));
}

#[test]
fn outline_tree_and_destinations() {
    let d = open(fixtures::outline_document());
    let o = outline::outline(&d);
    let titles: Vec<_> = o.iter().map(|i| (i.title.as_str(), i.depth, i.page)).collect();
    assert_eq!(
        titles,
        vec![
            ("Chapter 1", 0, Some(0)),
            ("Chapter 2", 0, Some(2)),
            ("Section 2.1", 1, Some(3)),
            ("Section 2.2", 1, Some(4)),
        ]
    );
    assert!(o[1].has_children && !o[0].has_children);
    assert_eq!(o[2].parent, Some(1));
    assert!(outline::outline(&open(fixtures::text_document())).is_empty());
}

#[test]
fn links_resolve_to_pages_and_uris() {
    let d = open(fixtures::links_document());
    // Link rects are PDF y 680..700 and 662..682 on a 792pt page: display y = 792 - pdf y.
    assert_eq!(outline::link_at(&d, 0, Point::new(100.0, 792.0 - 690.0)), Some(LinkTarget::Page(2)));
    assert_eq!(
        outline::link_at(&d, 0, Point::new(100.0, 792.0 - 670.0)),
        Some(LinkTarget::Uri("https://example.com/".into()))
    );
    assert_eq!(outline::link_at(&d, 0, Point::new(400.0, 400.0)), None);
    assert_eq!(outline::link_at(&d, 1, Point::new(100.0, 100.0)), None);
    let rects = outline::link_rects(&d, 0);
    assert_eq!(rects.len(), 2);
    assert!(rects.iter().any(|r| r.contains(Point::new(100.0, 792.0 - 690.0))));
    assert!(outline::link_rects(&d, 1).is_empty());
}

#[test]
fn document_info() {
    let d = open(fixtures::text_document());
    let i = outline::info(&d);
    assert_eq!(i.title, "Paper Mirror fixture");
    assert_eq!(i.author, "Paper Mirror tests");
    assert_eq!(i.creator, "paper-mirror fixtures.rs");
    assert_eq!(i.version, "1.7");
    assert_eq!(i.pages, 3);
    assert_eq!(i.encryption, "None");
    assert!(i.permissions.is_empty());
    let e = PdfDoc::open(Source::Memory(Arc::new(fixtures::encrypted_document("x"))), Some("x")).unwrap();
    let i = outline::info(&e);
    assert_eq!(i.encryption, "AES 256-bit");
    assert!(i.permissions.iter().all(|(_, allowed)| *allowed));
}
