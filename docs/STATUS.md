# Build status (2026-11-07)

## Done and verified (automated tests, clippy clean)
- Prototype + M1 shell: menus/shortcut registry with sync lint, settings, recents, password prompt, view modes, multi-window.
- M2 viewing: search, outline, bookmarks, contact sheet, inspector, slideshow, rect selection, zoom to selection, loupe, links.
- M3 text selection and copy (text and region image), EditBridge for focused fields.
- M4 annotations: all Preview markup shapes, editable after save (/PaperMirror JSON), appearance streams, flatten export, true redaction, undo/redo, originals backup.
- M5 forms (PDFium form-fill engine) and DPAPI-protected signatures.
- M6 page organisation: rotate, delete, drag-reorder, insert blank, insert/combine from file, crop, multi-select, undo via document snapshots with annotations/bookmarks remapped; Export as PDF with effects, Reduce File Size, password and flattening; Duplicate; Revert To Last Saved/Original; Lock.
- M7 images (ADR 0007): PNG, JPEG, GIF (all frames), BMP, ICO, WebP, OpenEXR via `image`; TIFF (all pages), HEIC/HEIF/AVIF via WIC; JPEG 2000 via PDFium. Images are PDF pages at 0.75 pt/px (pixel-exact round trip, tested), so viewing, markup, rotate, crop, reorder and undo are shared. Save/Save As/Export to PNG, JPEG, HEIC, TIFF, GIF, WebP, BMP, OpenEXR, ICO and PDF; PDF pages export as images at a chosen resolution. Markup baked on save; EXIF kept (orientation reset), location hidden until Show Location, Remove Location Info; ICC profiles converted for display and written back. Flip, Adjust Size (units, resolution, resample), Adjust Color (levels, exposure, contrast, saturation, temperature, tint, sepia, sharpness, Auto Levels) with live preview, Instant Alpha, New from Clipboard, insert images as PDF pages. A 24 MP JPEG opens in ~220 ms (release), peak working set 192 MB.
- M8 Windows integration (ADR 0008): Print (PrintDlgEx, PDFium to the printer DC, fitted and auto-rotated, markup included; verified via Microsoft Print to PDF), Share sheet (unsaved changes shared as a copy), opt-in per-user file types + Default apps, per-user installer (0.5 MB stub, deflate payload with SHA-256, Start menu, Apps & features, update in place, self-removing uninstaller), portable zip, generated THIRD-PARTY-NOTICES.txt (239 linked crates + PDFium), icon/version/manifest resources, scripts/package.ps1 (setup 14.4 MB, zip 13.9 MB).

## Remaining
- M9 3D, M10 performance report, final README and known-limitations review, final release build.

## Build
`. scripts/env.sh` (Git Bash) then `cargo test` / `cargo build --release`. Toolchain: Rust 1.99.1 gnullvm + llvm-mingw in `D:\rust`; PDFium via `scripts/fetch-pdfium.ps1`.
