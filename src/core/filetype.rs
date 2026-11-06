//! File-type detection from content, with the extension as a fallback.

use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Pdf,
    Image(ImageFormat),
    Model3d(ModelFormat),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Tiff,
    Bmp,
    WebP,
    Ico,
    Heic,
    Jpeg2000,
    OpenExr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    Stl,
    Obj,
    Ply,
    Gltf,
    Glb,
    Usd,
    Usdz,
    Abc,
    Mtlx,
    Vdb,
}

impl ImageFormat {
    pub fn name(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Gif => "GIF",
            ImageFormat::Tiff => "TIFF",
            ImageFormat::Bmp => "BMP",
            ImageFormat::WebP => "WebP",
            ImageFormat::Ico => "ICO",
            ImageFormat::Heic => "HEIC",
            ImageFormat::Jpeg2000 => "JPEG 2000",
            ImageFormat::OpenExr => "OpenEXR",
        }
    }
}

/// Identifies a file from its first bytes. `ext` (lower case, no dot) breaks ties for
/// formats without a reliable signature (text-based 3D formats).
pub fn detect(head: &[u8], ext: &str) -> FileKind {
    let starts = |sig: &[u8]| head.starts_with(sig);
    // PDF headers may be preceded by up to 1 KB of junk; PDFium accepts that too.
    if head.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return FileKind::Pdf;
    }
    if starts(b"\x89PNG\r\n\x1a\n") {
        return FileKind::Image(ImageFormat::Png);
    }
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return FileKind::Image(ImageFormat::Jpeg);
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return FileKind::Image(ImageFormat::Gif);
    }
    if starts(b"II*\0") || starts(b"MM\0*") || starts(b"II+\0") || starts(b"MM\0+") {
        return FileKind::Image(ImageFormat::Tiff);
    }
    if starts(b"BM") && head.len() > 14 {
        return FileKind::Image(ImageFormat::Bmp);
    }
    if starts(b"RIFF") && head.len() >= 12 && &head[8..12] == b"WEBP" {
        return FileKind::Image(ImageFormat::WebP);
    }
    if starts(&[0, 0, 1, 0]) && ext == "ico" {
        return FileKind::Image(ImageFormat::Ico);
    }
    if starts(&[0x76, 0x2F, 0x31, 0x01]) {
        return FileKind::Image(ImageFormat::OpenExr);
    }
    if starts(&[0, 0, 0, 0x0C, b'j', b'P', b' ', b' ']) || starts(&[0xFF, 0x4F, 0xFF, 0x51]) {
        return FileKind::Image(ImageFormat::Jpeg2000);
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        if [b"heic", b"heix", b"hevc", b"heim", b"heis", b"mif1", b"msf1", b"avif"].iter().any(|b| brand == *b) {
            return FileKind::Image(ImageFormat::Heic);
        }
    }
    if starts(b"glTF") {
        return FileKind::Model3d(ModelFormat::Glb);
    }
    if starts(b"ply\n") || starts(b"ply\r\n") {
        return FileKind::Model3d(ModelFormat::Ply);
    }
    if starts(b"PXR-USDC") || starts(b"#usda") {
        return FileKind::Model3d(ModelFormat::Usd);
    }
    if starts(b"PK\x03\x04") && ext == "usdz" {
        return FileKind::Model3d(ModelFormat::Usdz);
    }
    if starts(b"Ogawa") || (starts(&[0x89, b'H', b'D', b'F']) && ext == "abc") {
        return FileKind::Model3d(ModelFormat::Abc);
    }
    match ext {
        "stl" => FileKind::Model3d(ModelFormat::Stl),
        "obj" => FileKind::Model3d(ModelFormat::Obj),
        "gltf" => FileKind::Model3d(ModelFormat::Gltf),
        "mtlx" => FileKind::Model3d(ModelFormat::Mtlx),
        "vdb" => FileKind::Model3d(ModelFormat::Vdb),
        "usd" | "usda" | "usdc" => FileKind::Model3d(ModelFormat::Usd),
        _ => FileKind::Unknown,
    }
}

/// Reads the first 4 KB of a file and detects its kind.
pub fn detect_path(path: &Path) -> std::io::Result<FileKind> {
    let mut f = std::fs::File::open(path)?;
    let mut head = vec![0u8; 4096];
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..])? {
            0 => break,
            k => n += k,
        }
    }
    head.truncate(n);
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    Ok(detect(&head, &ext))
}

/// Extensions offered by the Open dialog, grouped as (label, extensions).
pub const OPEN_FILTERS: &[(&str, &[&str])] = &[
    (
        "All supported documents",
        &[
            "pdf", "png", "jpg", "jpeg", "jpe", "gif", "tif", "tiff", "bmp", "webp", "ico", "heic", "heif", "jp2",
            "j2k", "jpf", "jpx", "exr", "stl", "obj", "ply", "gltf", "glb",
        ],
    ),
    ("PDF documents", &["pdf"]),
    (
        "Images",
        &[
            "png", "jpg", "jpeg", "jpe", "gif", "tif", "tiff", "bmp", "webp", "ico", "heic", "heif", "jp2", "j2k",
            "jpf", "jpx", "exr",
        ],
    ),
    ("3D models", &["stl", "obj", "ply", "gltf", "glb", "usd", "usda", "usdc", "usdz", "abc", "mtlx", "vdb"]),
    ("All files", &["*"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_signature_not_extension() {
        assert_eq!(detect(b"%PDF-1.7\n", "png"), FileKind::Pdf);
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n....", "pdf"), FileKind::Image(ImageFormat::Png));
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0], ""), FileKind::Image(ImageFormat::Jpeg));
        assert_eq!(detect(b"GIF89a....", ""), FileKind::Image(ImageFormat::Gif));
        assert_eq!(detect(b"II*\0....", ""), FileKind::Image(ImageFormat::Tiff));
        assert_eq!(detect(b"RIFF\0\0\0\0WEBPVP8 ", ""), FileKind::Image(ImageFormat::WebP));
        assert_eq!(detect(b"\0\0\0\x18ftypheic\0\0\0\0", ""), FileKind::Image(ImageFormat::Heic));
        assert_eq!(
            detect(&[0, 0, 0, 0x0C, b'j', b'P', b' ', b' ', 13, 10, 0x87, 10], ""),
            FileKind::Image(ImageFormat::Jpeg2000)
        );
        assert_eq!(detect(&[0x76, 0x2F, 0x31, 0x01, 2, 0, 0, 0], ""), FileKind::Image(ImageFormat::OpenExr));
        assert_eq!(detect(b"glTF\x02\0\0\0", ""), FileKind::Model3d(ModelFormat::Glb));
    }

    #[test]
    fn pdf_header_after_junk_and_unknowns() {
        let mut v = vec![b'x'; 300];
        v.extend_from_slice(b"%PDF-1.4");
        assert_eq!(detect(&v, ""), FileKind::Pdf);
        assert_eq!(detect(b"hello world", "txt"), FileKind::Unknown);
        assert_eq!(detect(b"", ""), FileKind::Unknown);
        assert_eq!(detect(b"solid cube", "stl"), FileKind::Model3d(ModelFormat::Stl));
    }
}
