//! M8: printing through GDI, verified by printing to "Microsoft Print to PDF" with an
//! output file and reading the result back. Skips when that printer is missing.

use std::sync::Arc;

use paper_mirror::fixtures;
use paper_mirror::pdf::document::{PdfDoc, RenderTarget};
use paper_mirror::pdf::ffi::Source;
use paper_mirror::pdf::print::{self, PrintSpec};

#[test]
fn prints_selected_pages_to_a_pdf_printer() {
    let Some(dc) = print::printer_dc("Microsoft Print to PDF") else {
        eprintln!("Microsoft Print to PDF is not installed; skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("pm-print-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("printed.pdf");
    let _ = std::fs::remove_file(&out);
    let doc = PdfDoc::open(Source::Memory(Arc::new(fixtures::text_document())), None).unwrap();
    assert!(doc.page_count() >= 3);
    let spec = PrintSpec {
        pages: vec![0, 2],
        title: "Paper Mirror print test".into(),
        output: Some(out.clone()),
        enlarge: false,
        auto_rotate: true,
    };
    let printed = print::print(&doc, dc, &spec);
    print::delete_dc(dc);
    assert_eq!(printed, Ok(2));
    // The driver finishes writing the file shortly after EndDoc.
    let start = std::time::Instant::now();
    while !(out.exists() && std::fs::metadata(&out).is_ok_and(|m| m.len() > 0)) {
        assert!(start.elapsed().as_secs() < 30, "no output file");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let result = PdfDoc::open(Source::File(out.clone()), None).unwrap();
    assert_eq!(result.page_count(), 2);
    // The printed page is not blank: some dark pixels where the text was.
    let (w, h) = (300u32, 400u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let scale = w as f32 / result.page_sizes()[0].0;
    assert!(result.render(0, scale, 0.0, 0.0, RenderTarget { pixels: &mut px, width: w, height: h }, true));
    let dark = px.as_chunks::<4>().0.iter().filter(|p| p[0] < 128 && p[1] < 128 && p[2] < 128).count();
    assert!(dark > 100, "printed page has content ({dark} dark pixels)");
}
