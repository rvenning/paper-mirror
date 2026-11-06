# Build status (2026-11-06)

## Done and verified (automated tests, clippy clean)
- Prototype + M1 shell: menus/shortcut registry with sync lint, settings, recents, password prompt, view modes, multi-window.
- M2 viewing: search, outline, bookmarks, contact sheet, inspector, slideshow, rect selection, zoom to selection, loupe, links.
- M3 text selection and copy (text and region image), EditBridge for focused fields.
- M4 annotations: all Preview markup shapes, editable after save (/PaperMirror JSON), appearance streams, flatten export, true redaction, undo/redo, originals backup.
- M5 forms (PDFium form-fill engine) and DPAPI-protected signatures.
- M6 page organisation: rotate, delete, drag-reorder, insert blank, insert/combine from file, crop, multi-select, undo via document snapshots with annotations/bookmarks remapped; Export as PDF with effects, Reduce File Size, password and flattening; Duplicate; Revert To Last Saved/Original; Lock.

## Remaining
- M7 images (decode/edit/convert/metadata/colour/GIF frames/instant alpha), M8 print/share/packaging/installer, M9 3D, M10 perf report, README, known-limitations.md, THIRD-PARTY-NOTICES.txt.

## Build
`. scripts/env.sh` (Git Bash) then `cargo test` / `cargo build --release`. Toolchain: Rust 1.99.1 gnullvm + llvm-mingw in `D:\rust`; PDFium via `scripts/fetch-pdfium.ps1`.
