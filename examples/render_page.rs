//! Renders one page of a PDF to PNG: `cargo run --example render_page -- in.pdf page out.png [width]`.
use std::sync::Arc;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).expect("read pdf");
    let page: usize = a[1].parse().unwrap();
    let width: u32 = a.get(3).and_then(|w| w.parse().ok()).unwrap_or(900);
    let d = paper_mirror::pdf::document::PdfDoc::open(paper_mirror::pdf::ffi::Source::Memory(Arc::new(bytes)), None)
        .unwrap();
    let (w, h, px) = d.render_to_vec(page, width).unwrap();
    image::RgbaImage::from_raw(w, h, px).unwrap().save(&a[2]).unwrap();
}
