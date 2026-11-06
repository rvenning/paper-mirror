//! M5 backend tests: filling AcroForm fields through PDFium and saving them.

use std::sync::Arc;

use paper_mirror::fixtures;
use paper_mirror::pdf::document::PdfDoc;
use paper_mirror::pdf::ffi::Source;
use paper_mirror::pdf::forms::{self, FieldKind};

fn open(bytes: Vec<u8>) -> PdfDoc {
    PdfDoc::open(Source::Memory(Arc::new(bytes)), None).unwrap()
}

#[test]
fn detects_fields() {
    let d = open(fixtures::form_document());
    let f = forms::fields(&d);
    let kinds: Vec<_> = f.iter().map(|x| (x.name.as_str(), x.kind)).collect();
    assert_eq!(
        kinds,
        vec![("name", FieldKind::Text), ("subscribe", FieldKind::CheckBox), ("colour", FieldKind::ComboBox)]
    );
    let colour = &f[2];
    assert_eq!(colour.options, vec!["Red", "Green", "Blue"]);
    assert_eq!(colour.value, "Green");
    assert!(!f[1].checked);
    // Field rectangles are in display space (y down): "name" sits above "subscribe".
    assert!(f[0].rect.y < f[1].rect.y);
    assert!(forms::fields(&open(fixtures::text_document())).is_empty());
}

#[test]
fn fills_and_saves_every_field_type() {
    let d = open(fixtures::form_document());
    let f = forms::fields(&d);
    assert!(forms::set_text(&d, f[0].page, f[0].annot_index, "Ada Lovelace \u{2014} 1815"));
    assert!(forms::click(&d, &f[1]));
    assert!(forms::select_option(&d, f[2].page, f[2].annot_index, 2));
    let now = forms::fields(&d);
    assert_eq!(now[0].value, "Ada Lovelace \u{2014} 1815");
    assert!(now[1].checked);
    assert_eq!(now[2].value, "Blue");

    let saved = open(d.doc.save_to_vec(false).unwrap());
    let back = forms::fields(&saved);
    assert_eq!(back[0].value, "Ada Lovelace \u{2014} 1815");
    assert!(back[1].checked);
    assert_eq!(back[2].value, "Blue");
    // The regenerated appearance shows the text: the field area now has dark pixels.
    let (w, _h, px) = saved.render_to_vec(0, 612).unwrap();
    let r = back[0].rect;
    let dark = (r.y as u32..r.bottom() as u32)
        .flat_map(|y| (r.x as u32..r.right() as u32).map(move |x| (x, y)))
        .filter(|(x, y)| px[((y * w + x) * 4) as usize] < 100)
        .count();
    assert!(dark > 40, "field text rendered: {dark}");
    // Clicking again unchecks.
    assert!(forms::click(&saved, &back[1]));
    assert!(!forms::fields(&saved)[1].checked);
}
