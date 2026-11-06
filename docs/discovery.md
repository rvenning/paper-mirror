# Discovery report

Date: 2026-11-06. Sources read in full from the Paper Mirror project home in Notion and
its five linked documents: Product specification — Apple Preview parity, Visual and
interaction specification, Technical architecture and risk register, Testing,
performance and release plan, and Claude Code project handoff and implementation plan.
No page was inaccessible.

## Requirements summary

**Product.** A lightweight Windows 11 desktop viewer/editor that reproduces the everyday
workflows and information architecture of macOS Preview (reference: Preview User Guide
for macOS Tahoe 26) using original code and assets.

- **PDF:** open (dialog, file association, drag-and-drop), thumbnails, contact sheet,
  outline, continuous / single / two-page views, page navigation, zoom (in, out, actual,
  fit page, fit width, zoom to selection), loupe, search with match navigation, text
  selection and copy, document info, bookmarks, slideshow, correct rendering at high
  zoom.
- **Markup:** highlight / underline / strikethrough, sketch, draw, shapes (line, arrow,
  rectangle, oval, speech bubble), text boxes, notes, signatures, style controls,
  move/resize/delete, editable after save, flattened export, true redaction, image
  region selection, zoom-lens annotation. Not a general text editor for existing PDF
  content.
- **Forms and signatures:** detect and fill fields, save, keyboard workflow; create
  signatures with mouse/touch/image, store them securely on the device, place and resize.
- **Page organisation:** insert from another PDF, combine, reorder by drag, delete,
  rotate, crop, non-adjacent multi-select, undo or original preservation.
- **Effects, security, files:** PDF effects (Quartz-filter equivalents), save/export,
  revert, duplicate, lock, password protection/encryption, password prompt, reduce file
  size, share, print, clear distinction between reversible and permanent changes.
- **Images:** HEIC, JPEG, JPEG 2000, OpenEXR, PNG, TIFF, animated GIF frames; metadata
  incl. location (with consent), colour profiles, rotate, crop, resize, annotate,
  background removal, convert/export preserving transparency/profiles/metadata, print,
  share, import from a device.
- **3D:** USD/USDZ/USDA/USDC, PLY, STL, Alembic (Ogawa), glTF/GLB, OBJ, MaterialX,
  OpenVDB — view, limited editing, export (formats, image, movie, PDF). Explicitly a
  separately validated, later workstream.
- **Shell:** Preview-like window, contextual toolbar, sidebar, markup toolbar, File / Edit
  / View / Tools / Window / Help menus, context menus, dialogs, light/dark, resizable
  panels, documented shortcuts, mouse/keyboard/touch/high-DPI, accessibility, multiple
  windows, file associations, recent documents, drag-and-drop.
- **Non-functional:** fast cold start, low idle memory, lazy rendering, bounded caches,
  responsive UI, no telemetry/network/background services/auto-start, never silently
  overwrite originals, graceful handling of bad files, local-first privacy, protected
  signature storage, no document content in logs.

**Engineering rules.** Rust + Slint + PDFium; UI never calls PDFium directly; expensive
work off the UI thread; module boundaries (app, ui, core, pdf, images, platform_windows,
storage); every dependency justified; no speculative features; parity claimed only with
a passing test or documented manual verification; ADRs in `docs/decisions/`; feature
checklist with status; `docs/known-limitations.md`.

## Conflicts and ambiguities in the documentation

| # | Conflict / ambiguity | Resolution adopted |
|---|---|---|
| 1 | The product spec puts 3D in the parity target; the handoff calls it "later, separately validated"; the project-home roadmap omits it. | Treat 3D as the last implementation milestone after a feasibility ADR; implement only formats with a verifiable local decoder and fixture; document the rest as limitations. |
| 2 | Visual spec requires reference captures on a Mac. No Mac is available in this environment. | Use Apple's published Preview User Guide and documented menu/shortcut structure as the reference; record that screenshots were not captured (known limitation). |
| 3 | Menus: spec lists File, Edit, View, Tools, Window, Help; Preview also has a Go menu. | Include Go (page navigation) because it is part of Preview's information architecture; it adds no new scope. |
| 4 | "Lock a PDF" is ambiguous: Preview's Lock is a macOS file lock, distinct from password protection. | Lock = Windows read-only attribute, with editing disabled while locked; password protection is separate. |
| 5 | Image format list is Apple's *conversion* list, not an exhaustive open list. | Open every listed format plus BMP/GIF/ICO/WebP that the chosen pure-Rust codecs support; export only where an encoder exists (see ADR on codecs). |
| 6 | Auto-update is an open decision, but network activity and background services are prohibited. | No auto-update. Releases are replaced manually (ADR). |
| 7 | "Share" requires an OS share mechanism, yet no network calls are allowed. | Use the Windows Share sheet, which is user-initiated and hands off to the OS; the app itself makes no network calls. |
| 8 | Numeric performance targets must not be set before a baseline exists, but the handoff wants measured performance. | Measure the prototype first (see `docs/performance.md`), then set guardrails from that baseline. |
| 9 | Architecture doc proposes `app-shell/`, `ui/` folders; the handoff proposes `src/app/`, `src/ui/`, … | Follow the handoff's `src/` layout (one crate, modules per boundary); Slint markup lives in `ui/`. |
| 10 | Background removal "subject to a feasible local implementation". | Implement Preview's Instant Alpha style colour-region removal locally; ML subject extraction is a documented limitation. |

## Environment report

| Item | Finding |
|---|---|
| OS | Windows 11 Home 10.0.26100 |
| Disk | C: 3.9 GB free of 223 GB (cannot host Visual Studio Build Tools); D: 924 GB free. User instruction: all build tools on D:. |
| Rust | Not installed initially. Installed rustup to `D:\rust` (`CARGO_HOME`, `RUSTUP_HOME`), PATH not modified. Toolchain 1.99.1 `x86_64-pc-windows-gnullvm`. |
| C/C++ toolchain | No MSVC Build Tools or Windows SDK. Installed llvm-mingw 20261104 (UCRT, x86-64) to `D:\rust\llvm-mingw`; supplies clang, lld and llvm-dlltool. |
| Slint | 1.18.1 (crates.io, 2026-09-21). Licence: GPL-3.0 OR Royalty-free 2.0 OR Software 3.0. Windows via winit backend; native menu bar via muda; AccessKit for UI Automation. |
| PDFium | bblanchon/pdfium-binaries `chromium/8138` (2026-11-04), `pdfium-win-x64.tgz`, SHA-256 `8d48973e…a8a5`. PDFium BSD-3-Clause plus bundled third-party licences (FreeType, ICU, libjpeg-turbo, libpng, OpenJPEG, lcms, zlib, …), all permissive. |
| PDFium binding | pdfium-render 0.9.5 (MIT OR Apache-2.0), used as a dynamically loaded function table only. |
| Other crates | lopdf 0.47.0 (MIT; AES-256 encryption writer), image 0.25.10 (MIT/Apache), rfd 0.17.2 (MIT), windows-sys 0.61 (MIT/Apache), serde/serde_json (MIT/Apache), hayro-jpeg2000 0.5.0 (MIT/Apache; candidate), kamadak-exif 0.6.2 (BSD-2; candidate). |
| 3D (survey only) | No maintained pure-Rust USD (USDC), Alembic, MaterialX or OpenVDB readers; glTF (`gltf`), OBJ, STL and PLY are straightforward. See ADR 0010. |

## Toolchain problems found and solved

1. The default `x86_64-pc-windows-gnu` toolchain's bundled `dlltool` needs an external
   assembler it does not ship, so any crate using `raw-dylib` (all of `windows-sys`
   0.61) fails to build.
2. Pointing `-C dlltool` at llvm-dlltool while linking with GNU `ld` produced binaries
   that crashed with a DEP violation (mixed import-library formats).
3. Fix: use the `x86_64-pc-windows-gnullvm` host with llvm-mingw end to end, and link
   LLVM libunwind statically (`+crt-static`) so the executable only imports DLLs that
   ship with Windows.
