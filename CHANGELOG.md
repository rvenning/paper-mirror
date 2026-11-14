# Changelog

All notable changes to Paper Mirror are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-14

The first public release.

### PDFs
- Fast, virtualised viewing in continuous, single-page and two-page layouts. Pages
  stay sharp at any zoom through high-resolution detail tiles.
- Thumbnails, table of contents, bookmarks, search with match navigation, contact
  sheet, slideshow, magnifier, and links (safe schemes only).
- Text selection and copy, and copy a region as an image.
- Markup:
  - highlight, underline and strikethrough;
  - sketch with shape recognition, freehand drawing, and shapes;
  - text boxes, notes, signatures and loupe annotations.
- Annotations are saved as standard PDF annotations, editable after saving.
- True redaction that removes the text, images and graphics underneath.
- Form filling, and signatures stored encrypted for your Windows account.
- Page organisation: rotate, reorder, delete, insert, combine and crop.
- Export as PDF with effects, size reduction, password protection and flattening.
- Duplicate, Revert To, and Lock.

### Images
- PNG, JPEG, GIF (every frame), TIFF (every page), BMP, ICO, WebP, OpenEXR, JPEG 2000,
  and HEIC/HEIF/AVIF (through Windows' codec extensions).
- Pixel-exact viewing and saving. Markup is merged into the pixels on save.
- Rotate, flip, crop, Adjust Size, Adjust Color, and Instant Alpha.
- EXIF, with location shown only on request and removable.
- Colour profiles used for display and kept on save.
- Conversion between formats, and New from Clipboard.

### 3D models
- STL, OBJ, PLY and glTF/GLB, drawn on the CPU.
- Orbit, pan and zoom; orientation edits.
- Export to the other formats or to an image.

### Windows
- Print through the Windows Print dialog, including Microsoft Print to PDF.
- Share through the Windows Share sheet.
- Opt-in file-type registration.
- Per-user installer (no administrator rights) or portable zip.
- Touch: tap, one-finger scrolling, pinch to zoom, and drawing with a finger in markup
  tools. A pen selects and draws like a mouse.
- Screen readers: every control is named, tool states are announced, and page text is
  readable.
- Light and dark appearance. The title bar, menu bar and toolbar form one surface, and
  the Windows accent colour and Windows 11 typeface are used.
- Closing with unsaved changes asks Save / Don't Save / Cancel.
- No network access, telemetry, background services or auto-start.

### Packaging
- MIT licence.
- Generated third-party notices.
- Optional Authenticode signing of every program and DLL.
- SHA-256 checksums for every download.

[Unreleased]: https://github.com/rvenning/paper-mirror/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/rvenning/paper-mirror/releases/tag/v0.1.0
