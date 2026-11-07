# 0007 — Images: codecs, document model, editing and saving

Date: 2026-11-07 · Status: accepted

## Context
Preview opens images in the same window as PDFs, with the same thumbnails, zoom,
markup, rotate, crop, Inspector and export. It also has image-only tools: Adjust Size,
Adjust Color, Instant Alpha, flip, and conversion between formats. The product spec
lists HEIC, JPEG, JPEG 2000, OpenEXR, PNG, TIFF and animated GIF frames, location
metadata "with consent", and colour profiles. Images are untrusted input.

## Decision
1. **One document model.** An opened image becomes an in-memory PDF built with PDFium:
   one page per frame, holding one image object, at **0.75 pt per pixel**, so 100% zoom
   shows one image pixel per 96-DPI screen pixel. Viewing, thumbnails, the contact sheet,
   markup, rotation, cropping, page reordering, undo snapshots and Export as PDF all
   reuse the PDF code paths. Crops on images snap to whole pixels.
2. **Saving back to pixels.** On save, annotations are flattened into a copy of the
   document. Each page is then rendered at exactly 4/3 px per point (one pixel per
   image pixel) onto a transparent bitmap. Tests check that an image without markup
   comes back unchanged (pixel for pixel), alpha included.
3. **Decoders** in order of preference:
   - `image` 0.25.10 (pure Rust, memory-safe) for PNG, JPEG, GIF (all frames, with
     delays), BMP, ICO, WebP and OpenEXR. Decoding has memory limits (1 GiB allocation,
     40,000 px per side, 250 MP per frame).
   - Windows Imaging Component (system codecs, through a small hand-written COM binding)
     for TIFF (all pages), HEIC/HEIF and AVIF (when the HEIF/AV1 extensions from
     Microsoft are installed), and as a fallback. Explorer uses the same codecs for
     thumbnails.
   - PDFium's own JPEG 2000 decoder (OpenJPEG), by wrapping the codestream in a
     one-image PDF. This avoids a new dependency.
4. **Encoders.** PNG (via `png` 0.18.1, which writes iCCP, eXIf and pHYs), JPEG, GIF
   (animated, frame delays kept), BMP, ICO and lossless WebP come from `image`. TIFF
   (all pages, with resolution) comes from `tiff` 0.11.3, OpenEXR from `image`
   (half-float, linearised from sRGB), and HEIC from WIC when the HEIF encoder is
   installed. JPEG 2000 has no local encoder, so it is read-only and Save asks for
   another format.
5. **Colour.** Pixels are stored in the image's own colour space, so saving keeps them
   untouched and re-attaches the original ICC profile. The **display** goes through
   `moxcms` 0.8.1 (pure Rust; already a dependency of `image`), which converts embedded
   profiles to sRGB when pages are rendered for the screen.
6. **Metadata.** EXIF is parsed with `kamadak-exif` 0.6.2 (pure Rust, BSD-2-Clause) and
   shown in the Inspector. Orientation is applied on open and written back as 1. GPS
   coordinates stay hidden until the user chooses Show Location (that is the consent),
   and nothing is ever looked up on a map, since the app makes no network requests.
   Tools > Remove Location Information strips GPS on the next save. EXIF and ICC are
   kept when the format can hold them.
7. **Edits.** Flip, Adjust Size, Adjust Color and Instant Alpha act on pixels. The
   engine renders the page at 1:1, the edit runs in Rust, and the page's image is
   replaced. Each edit takes a PDF snapshot for undo. Adjust Color previews live as a
   filter on rendered bitmaps and is applied to the pixels when the panel closes.
8. **Depth.** Editing works in 8-bit RGBA. Saving a 16-bit or floating-point image
   stores 8-bit data; this is listed in known limitations.

## Dependencies
- `image` features gif, bmp, ico, webp, exr, tiff (MIT OR Apache-2.0; pulls `gif`,
  `image-webp`, `exr`, `tiff`, `zune-inflate`, `half`: all MIT/Apache/Zlib, pure Rust).
- `png` 0.18.1, `tiff` 0.11.3 as direct dependencies (versions `image` already uses).
- `moxcms` 0.8.1 (BSD-3-Clause OR Apache-2.0).
- `kamadak-exif` 0.6.2 (BSD-2-Clause, `mutate_once` only).
- `windows-sys` feature `Win32_System_Com` for `CoCreateInstance`.

## Consequences
Images and PDFs share one UI, so every viewing and markup feature works on both. Frame
count is bounded by memory, since each frame is a page. Animated GIFs show their frames
as pages, as Preview does, and are not played. Machine-learning subject extraction
("Remove Background") is not available. Instant Alpha's colour-region removal is the
local alternative.
