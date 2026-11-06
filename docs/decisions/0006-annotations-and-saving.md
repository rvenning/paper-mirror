# 0006 — Annotation model, appearance streams, saving and encryption

Date: 2026-11-06 · Status: accepted

## Context
Preview-style markup must stay editable after save, look right in other PDF viewers,
support any script in text boxes, and be drawn live while editing. PDFium can create
annotations but only appends page objects to Ink/Stamp appearances, cannot attach
resources to `FPDFAnnot_SetAP` streams, and cannot write encryption.

## Decision
1. **Model.** Paper Mirror's annotations live in a Rust model (`core::annot`) in page
   display space while editing, and are drawn by Slint as vector overlays. They are
   persisted in PDF user space so later page rotation or cropping cannot misplace them.
2. **Round trip.** Each saved annotation is a standard PDF annotation (`/Highlight`,
   `/Underline`, `/StrikeOut`, `/Ink`, `/Line`, `/Square`, `/Circle`, `/Polygon`,
   `/FreeText`, `/Text`, `/Stamp`) with an appearance stream Paper Mirror generates,
   plus a private `/PaperMirror` JSON string. On open, annotations carrying that key are
   rebuilt into the model and removed from PDFium's in-memory page so they are not
   drawn twice. Annotations from other software stay with PDFium (rendered, deletable).
3. **Writing.** PDFium serialises the document; lopdf then appends the annotation
   objects and, when requested, AES-256 encryption (R6). Output is written to a sibling
   temporary file and atomically renamed over the target.
4. **Text.** Text-box and note appearances draw glyph outlines (via `ttf-parser` from
   the Windows system fonts, with CJK/symbol fallbacks) instead of embedding fonts.
   Any script the system fonts cover renders identically in other viewers, files stay
   small, and the text itself is kept in `/Contents` for accessibility and search.
   Limitation: no complex-script shaping (Arabic joining, Indic reordering).
5. **Flattening** writes the annotations and then calls PDFium's `FPDFPage_Flatten`.
6. **Redaction** edits PDFium page objects: characters inside the area are removed (the
   rest of each text object is rebuilt), image pixels inside are blacked out, vector
   paths fully inside are removed, and an opaque black rectangle is drawn. Redaction is
   applied on save only after explicit confirmation, and defaults to Save As.
7. **Originals.** Before the first in-place save of a file in a session, the original
   is copied to `<data dir>\Originals` (bounded), enabling File > Revert To > Original.

## Dependencies
`ttf-parser` 0.25.1 (MIT OR Apache-2.0, no dependencies) for glyph outlines and metrics.

## Consequences
Annotation appearance is deterministic and unit-testable (content streams are text).
Re-saving an encrypted PDF re-encrypts it with AES-256 using the open password as both
user and owner password and the original permission flags.
