//! Deterministic, self-created test documents. Used by the unit and integration tests,
//! the performance probes and `cargo run --example make_fixtures`, which writes them to
//! `tests/fixtures/`. Nothing here is user content, so the corpus is freely
//! distributable.

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};

/// Words placed on every text page so search and selection tests have known targets.
pub const NEEDLE: &str = "lighthouse";

pub struct Builder {
    pub doc: Document,
    pages_id: ObjectId,
    font_id: ObjectId,
    kids: Vec<Object>,
}

fn text_op(x: f32, y: f32, size: f32, text: &str) -> Vec<Operation> {
    vec![
        Operation::new("BT", vec![]),
        Operation::new("Tf", vec!["F1".into(), size.into()]),
        Operation::new("Td", vec![x.into(), y.into()]),
        Operation::new("Tj", vec![Object::String(text.as_bytes().to_vec(), StringFormat::Literal)]),
        Operation::new("ET", vec![]),
    ]
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Builder {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        Builder { doc, pages_id, font_id, kids: Vec::new() }
    }

    /// Adds a page of the given size with text lines; returns its object id.
    pub fn text_page(&mut self, w: f32, h: f32, rotate: i32, lines: &[&str]) -> ObjectId {
        let mut ops = Vec::new();
        // A light frame makes rotation and cropping visible in renders.
        ops.push(Operation::new("RG", vec![0.6.into(), 0.6.into(), 0.6.into()]));
        ops.push(Operation::new("re", vec![18.into(), 18.into(), (w - 36.0).into(), (h - 36.0).into()]));
        ops.push(Operation::new("S", vec![]));
        let mut y = h - 72.0;
        for (i, line) in lines.iter().enumerate() {
            let size = if i == 0 { 20.0 } else { 12.0 };
            ops.extend(text_op(72.0, y, size, line));
            y -= if i == 0 { 34.0 } else { 18.0 };
        }
        self.page_with_content(w, h, rotate, ops, None)
    }

    pub fn page_with_content(&mut self, w: f32, h: f32, rotate: i32, ops: Vec<Operation>, extra_resources: Option<Dictionary>) -> ObjectId {
        let content = Content { operations: ops };
        let content_id = self.doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let mut resources = dictionary! { "Font" => dictionary! { "F1" => self.font_id } };
        if let Some(extra) = extra_resources {
            for (k, v) in extra.into_iter() {
                resources.set(k, v);
            }
        }
        let mut page = dictionary! {
            "Type" => "Page",
            "Parent" => self.pages_id,
            "MediaBox" => vec![0.into(), 0.into(), w.into(), h.into()],
            "Contents" => content_id,
            "Resources" => resources,
        };
        if rotate != 0 {
            page.set("Rotate", rotate);
        }
        let id = self.doc.add_object(page);
        self.kids.push(id.into());
        id
    }

    /// Adds a page containing a generated RGB image (a gradient with a black square)
    /// drawn at `(x, y, iw, ih)` in points.
    pub fn image_page(&mut self, px_w: u32, px_h: u32, place: (f32, f32, f32, f32), caption: &str) -> ObjectId {
        let mut data = Vec::with_capacity((px_w * px_h * 3) as usize);
        for y in 0..px_h {
            for x in 0..px_w {
                let inside = x > px_w / 3 && x < 2 * px_w / 3 && y > px_h / 3 && y < 2 * px_h / 3;
                if inside {
                    data.extend_from_slice(&[0, 0, 0]);
                } else {
                    data.extend_from_slice(&[(x * 255 / px_w) as u8, (y * 255 / px_h) as u8, 180]);
                }
            }
        }
        let img = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => px_w as i64,
                "Height" => px_h as i64,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
            },
            data,
        );
        let img_id = self.doc.add_object(img);
        let (x, y, iw, ih) = place;
        let mut ops = vec![
            Operation::new("q", vec![]),
            Operation::new("cm", vec![iw.into(), 0.into(), 0.into(), ih.into(), x.into(), y.into()]),
            Operation::new("Do", vec!["Im1".into()]),
            Operation::new("Q", vec![]),
        ];
        ops.extend(text_op(72.0, 740.0, 14.0, caption));
        self.page_with_content(612.0, 792.0, 0, ops, Some(dictionary! { "XObject" => dictionary! { "Im1" => img_id } }))
    }

    /// Finishes the page tree and catalog. `catalog_extra` entries are merged in.
    pub fn finish(mut self, catalog_extra: Dictionary) -> Document {
        let count = self.kids.len() as i64;
        self.doc.objects.insert(
            self.pages_id,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => self.kids, "Count" => count }),
        );
        let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => self.pages_id };
        for (k, v) in catalog_extra.into_iter() {
            catalog.set(k, v);
        }
        let catalog_id = self.doc.add_object(catalog);
        self.doc.trailer.set("Root", catalog_id);
        let info = self.doc.add_object(dictionary! {
            "Title" => Object::string_literal("Paper Mirror fixture"),
            "Author" => Object::string_literal("Paper Mirror tests"),
            "Subject" => Object::string_literal("Self-created test document"),
            "Keywords" => Object::string_literal("fixture, test"),
            "Creator" => Object::string_literal("paper-mirror fixtures.rs"),
        });
        self.doc.trailer.set("Info", info);
        self.doc
    }

    pub fn pages_id(&self) -> ObjectId {
        self.pages_id
    }
}

pub fn to_bytes(mut doc: Document) -> Vec<u8> {
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("serialise fixture");
    out
}

fn body_lines(page: usize) -> Vec<String> {
    vec![
        format!("Fixture page {}", page + 1),
        format!("The {NEEDLE} keeper counts ships on page {}.", page + 1),
        "Quick brown foxes jump over lazy dogs.".to_string(),
        "Searchable text: alpha beta gamma delta.".to_string(),
    ]
}

/// Three pages: Letter portrait, Letter rotated 90 degrees, A4 landscape.
pub fn text_document() -> Vec<u8> {
    let mut b = Builder::new();
    let sizes = [(612.0, 792.0, 0), (612.0, 792.0, 90), (842.0, 595.0, 0)];
    for (i, (w, h, r)) in sizes.iter().enumerate() {
        let lines = body_lines(i);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        b.text_page(*w, *h, *r, &refs);
    }
    to_bytes(b.finish(dictionary! {}))
}

/// `n` Letter pages of text.
pub fn many_pages(n: usize) -> Vec<u8> {
    let mut b = Builder::new();
    for i in 0..n {
        let lines = body_lines(i);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        b.text_page(612.0, 792.0, 0, &refs);
    }
    to_bytes(b.finish(dictionary! {}))
}

/// Five pages with a two-level outline (table of contents).
pub fn outline_document() -> Vec<u8> {
    let mut b = Builder::new();
    let mut pages = Vec::new();
    for i in 0..5 {
        let lines = body_lines(i);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        pages.push(b.text_page(612.0, 792.0, 0, &refs));
    }
    let outlines_id = b.doc.new_object_id();
    let ch1 = b.doc.new_object_id();
    let ch2 = b.doc.new_object_id();
    let sec21 = b.doc.new_object_id();
    let sec22 = b.doc.new_object_id();
    let dest = |p: ObjectId| -> Object { vec![p.into(), "Fit".into()].into() };
    b.doc.objects.insert(
        ch1,
        dictionary! { "Title" => Object::string_literal("Chapter 1"), "Parent" => outlines_id, "Next" => ch2, "Dest" => dest(pages[0]) }.into(),
    );
    b.doc.objects.insert(
        ch2,
        dictionary! {
            "Title" => Object::string_literal("Chapter 2"), "Parent" => outlines_id, "Prev" => ch1,
            "First" => sec21, "Last" => sec22, "Count" => 2, "Dest" => dest(pages[2])
        }
        .into(),
    );
    b.doc.objects.insert(
        sec21,
        dictionary! { "Title" => Object::string_literal("Section 2.1"), "Parent" => ch2, "Next" => sec22, "Dest" => dest(pages[3]) }.into(),
    );
    b.doc.objects.insert(
        sec22,
        dictionary! { "Title" => Object::string_literal("Section 2.2"), "Parent" => ch2, "Prev" => sec21, "Dest" => dest(pages[4]) }.into(),
    );
    b.doc.objects.insert(
        outlines_id,
        dictionary! { "Type" => "Outlines", "First" => ch1, "Last" => ch2, "Count" => 4 }.into(),
    );
    to_bytes(b.finish(dictionary! { "Outlines" => outlines_id, "PageMode" => "UseOutlines" }))
}

/// One page with an AcroForm: a text field, a checkbox and a combo box.
pub fn form_document() -> Vec<u8> {
    let mut b = Builder::new();
    let page = b.text_page(612.0, 792.0, 0, &["Form fixture", "Name:", "", "Subscribe:", "", "Colour:"]);
    let helv = b.doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let zadb = b.doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "ZapfDingbats" });
    let on_ap = b.doc.add_object(Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 14.into(), 14.into()],
                      "Resources" => dictionary! { "Font" => dictionary! { "ZaDb" => zadb } } },
        b"q 0 0 0 rg BT /ZaDb 12 Tf 1.5 2.5 Td (4) Tj ET Q".to_vec(),
    ));
    let off_ap = b.doc.add_object(Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 14.into(), 14.into()] },
        b"".to_vec(),
    ));
    let name = b.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("name"),
        "Rect" => vec![150.into(), 680.into(), 400.into(), 700.into()], "P" => page,
        "DA" => Object::string_literal("/Helv 12 Tf 0 g"), "F" => 4,
        "MK" => dictionary! { "BC" => vec![0.into()] },
    });
    let subscribe = b.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "T" => Object::string_literal("subscribe"),
        "Rect" => vec![150.into(), 644.into(), 164.into(), 658.into()], "P" => page, "V" => "Off", "AS" => "Off", "F" => 4,
        "AP" => dictionary! { "N" => dictionary! { "Yes" => on_ap, "Off" => off_ap } },
        "MK" => dictionary! { "BC" => vec![0.into()] },
    });
    let colour = b.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "Ff" => 131072, "T" => Object::string_literal("colour"),
        "Rect" => vec![150.into(), 608.into(), 300.into(), 626.into()], "P" => page, "F" => 4,
        "Opt" => vec![Object::string_literal("Red"), Object::string_literal("Green"), Object::string_literal("Blue")],
        "V" => Object::string_literal("Green"), "DA" => Object::string_literal("/Helv 12 Tf 0 g"),
    });
    if let Ok(Object::Dictionary(p)) = b.doc.get_object_mut(page) {
        p.set("Annots", vec![name.into(), subscribe.into(), colour.into()]);
    }
    let acro = dictionary! {
        "Fields" => vec![name.into(), subscribe.into(), colour.into()],
        "DR" => dictionary! { "Font" => dictionary! { "Helv" => helv, "ZaDb" => zadb } },
        "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        "NeedAppearances" => true,
    };
    to_bytes(b.finish(dictionary! { "AcroForm" => acro }))
}

/// One page with an image in the middle and text above it.
pub fn image_document() -> Vec<u8> {
    let mut b = Builder::new();
    b.image_page(300, 200, (156.0, 300.0, 300.0, 200.0), &format!("Image fixture with the word {NEEDLE}"));
    to_bytes(b.finish(dictionary! {}))
}

/// A PDF with a broken body and xref.
pub fn malformed_document() -> Vec<u8> {
    let mut v = text_document();
    v.truncate(v.len() / 3);
    v.extend_from_slice(b"\n%%garbage\x00\xff\xfe");
    v
}

/// Bytes that are not a PDF at all.
pub fn not_a_pdf() -> Vec<u8> {
    b"This is plain text, not a PDF.\n".to_vec()
}

/// Writes every committed fixture into `dir`.
pub fn write_all(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("text-3p.pdf"), text_document())?;
    std::fs::write(dir.join("outline-5p.pdf"), outline_document())?;
    std::fs::write(dir.join("form.pdf"), form_document())?;
    std::fs::write(dir.join("image.pdf"), image_document())?;
    std::fs::write(dir.join("malformed.pdf"), malformed_document())?;
    std::fs::write(dir.join("not-a-pdf.pdf"), not_a_pdf())?;
    std::fs::write(dir.join("pages-120.pdf"), many_pages(120))?;
    Ok(())
}
