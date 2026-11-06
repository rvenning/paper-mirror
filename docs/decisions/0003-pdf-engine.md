# 0003 — PDFium distribution and binding

Date: 2026-11-06 · Status: accepted

## Decision
- PDFium build `chromium/8138` from bblanchon/pdfium-binaries (`pdfium-win-x64.tgz`,
  SHA-256 pinned in `scripts/fetch-pdfium.ps1`), shipped as `pdfium.dll` next to the
  executable. Loaded by absolute path only (no DLL search-order exposure).
- `pdfium-render` 0.9.5 is used **only** as the typed, dynamically loaded function table
  (`PdfiumLibraryBindings`). Its high-level API keeps handles crate-private, which blocks
  clip rendering, device↔page transforms, annotation appearance streams and page moves.
  Paper Mirror owns every handle through small RAII wrappers in `src/pdf/ffi.rs`.
- File-backed documents load lazily through `FPDF_LoadCustomDocument` (positional reads),
  so memory does not scale with file size. Limit: 4 GB per file (PDFium's `c_ulong`).

## Licensing
PDFium is BSD-3-Clause; the binary bundles FreeType, ICU, libjpeg-turbo, libpng,
OpenJPEG, LittleCMS, zlib, abseil, HarfBuzz, highway, simdutf, fast_float, dragonbox,
agg and llvm-libc, all permissive. Their licence texts ship in
`THIRD-PARTY-NOTICES.txt`.

## Capability matrix (verified as each milestone lands)
| Need | PDFium | Paper Mirror code |
|---|---|---|
| Render, clip tiles, rotation | yes | — |
| Text extraction, search, char boxes | yes (`FPDFText_*`) | selection model |
| Outline, links, metadata | yes | — |
| Annotations: read, create, remove | yes (`FPDFAnnot_*`) | appearance streams for shapes/text |
| Forms: read/set values | yes (form-fill env) | overlay widgets |
| Page delete/move/import/rotate/crop | yes | undo snapshots |
| Save | yes (`FPDF_SaveAsCopy`) | atomic replace |
| **Write encryption** | **no** | lopdf post-process (ADR 0006) |
| Redaction | object-level only | char/pixel-level removal |
| Printing | renders bitmaps | GDI print path |
