# Build status (2026-11-07)

All milestones M0–M10 are built and verified: 168 automated tests (119 unit tests,
plus 49 backend, integration and headless UI tests, many of them end-to-end), with
clippy and rustfmt clean. Each requirement's status and evidence is in
`docs/feature-checklist.md`; gaps are in `docs/known-limitations.md`.

## Milestones
- **M0–M1 shell:** menus and a shortcut registry with a sync lint, settings, recents,
  password prompt, view modes, multiple windows.
- **M2 viewing:** search, outline, bookmarks, contact sheet, Inspector, slideshow,
  rectangular selection, zoom to selection, loupe, links.
- **M3** text selection and copy (text and region image).
- **M4 annotations:** every Preview markup shape, editable after save, appearance
  streams, flattened export, true redaction, undo/redo, original kept (ADR 0006).
- **M5** forms (PDFium form-fill) and DPAPI-protected signatures.
- **M6 pages:** organisation with undo, Export as PDF (effects, size reduction,
  password, flattening), Duplicate, Revert To, Lock.
- **M7 images (ADR 0007):** the formats in the spec plus BMP, ICO and WebP; images held
  as pixel-exact PDF pages; markup baked into the pixels on save; EXIF and location
  with consent; colour profiles; flip, resize, Adjust Color, Instant Alpha;
  conversion; New from Clipboard.
- **M8 Windows (ADR 0008):** Print, Share, opt-in file types, per-user installer and
  portable zip, generated third-party notices, icon, version information and manifest.
- **M9 3D (ADR 0009):** STL, OBJ, PLY and glTF/GLB viewer with a CPU rasteriser,
  orientation edits and export; unsupported formats explained.
- **M10:** performance report and guardrails (`docs/performance.md`,
  `scripts/measure.ps1`), feature checklist, README, known limitations, release
  package (`dist/`: setup 14.6 MB, portable zip 14.0 MB, SHA256SUMS).

## Build
`. scripts/env.sh` (Git Bash) or `. .\scripts\env.ps1` (PowerShell), then
`cargo test` or `.\scripts\package.ps1`. Toolchain: Rust 1.99.1 gnullvm and llvm-mingw
in `D:\rust`; PDFium via `scripts/fetch-pdfium.ps1`.

## After M10 (2026-11-07)
- **Licence and signing (ADR 0010):** MIT; `scripts/package.ps1` signs every program
  and DLL plus the setup when given a certificate, and the setup handles its signed
  layout.
- **Touch, pen and accessibility (ADR 0011):** touch pans, pinches and draws; a pen
  acts as a mouse; the first click after a focus change is no longer lost; Narrator
  can read page text, button states and every control name.
- **Look (ADR 0012):** title bar, menu bar and toolbar form one surface in the theme's
  colours, in light and dark; the Windows accent colour and Windows 11's typeface are
  used; hover and selection fade. Closing with unsaved changes uses a task dialog with
  Save / Don't Save / Cancel buttons.

## Open items for the owner
- Buy or set up a code-signing certificate (an OV certificate on a token, or Azure
  Trusted Signing) and package with it. Releases are unsigned until then.
- When hardware is at hand, try a real touch screen and pen, and listen to Narrator.
  Both were checked through Windows' own input injection and the UI Automation tree,
  not by hand.
