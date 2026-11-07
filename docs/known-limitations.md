# Known limitations

Reviewed against the release build on 2026-11-07. See also `docs/feature-checklist.md`.

## General
- Touch and pen arrive as pointer input (drag, draw, sign) but haven't been checked on
  touch hardware. Pinch-to-zoom isn't supported; use Ctrl+wheel or the zoom commands.
- Screen-reader support (AccessKit names and roles on controls) hasn't been checked
  with Narrator.
- Paper Mirror's own licence has not been chosen yet; see the final report.

## PDF markup and saving
- Text boxes and notes draw glyph outlines from the Windows fonts with no complex-script
  shaping, so Arabic joining and Indic reordering are not applied (ADR 0006).
- Documents larger than 64 MB keep no undo history for page and form-field edits, to
  limit memory. Annotation edits are always undoable.
- Pages can't be dragged between two open windows. Use Edit > Insert > Pages from File
  to combine documents.

## Images (ADR 0007)
- Editing works in 8-bit RGBA. Saving a 16-bit PNG/TIFF or a floating-point OpenEXR
  image writes 8-bit data (OpenEXR is written as half-float from those 8-bit values).
- JPEG 2000 opens through PDFium's decoder but can't be written: Save asks for another
  format.
- HEIC/HEIF and AVIF need Microsoft's HEIF Image Extensions plus the HEVC or AV1 Video
  Extensions. HEIC can be written only when the HEVC encoder is present. Only the
  primary image of a HEIF file is opened.
- Animated GIF frames are shown as pages and are not played. Saving keeps every frame
  and its delay; frames of different sizes are scaled to the first frame's size.
- WebP is written losslessly only, and animated WebP and APNG open at their first frame.
- Only TIFF and GIF hold several images. Saving a multi-page image as another format
  asks first, then keeps the first page.
- Embedded colour profiles are used for display (converted to sRGB) and written back
  on save. Edits work on the stored values, not on colour-managed ones. Images
  exported as PDF are tagged DeviceRGB.
- EXIF is kept on save, except the embedded thumbnail and the maker note, whose
  internal offsets can't be relocated safely. Orientation is applied on open and saved
  as 1. TIFF orientation is applied by Paper Mirror; HEIF transforms are applied by the
  Windows codec.
- Location is shown as coordinates only. No map is shown, because Paper Mirror makes
  no network requests.
- Instant Alpha removes connected pixels of similar colour (the drag distance sets the
  tolerance), with hard edges. Machine-learning subject extraction ("Remove
  Background") is not available.
- Markup on images (shapes, text, signatures) stays editable until the window closes,
  and is merged into the pixels in every saved file. Duplicate keeps it editable.
- Importing from cameras and scanners is not supported. Use File > New from Clipboard
  together with Windows+Shift+S for screenshots.
- The largest image accepted is 40,000 pixels per side and 250 megapixels per frame
  (400 megapixels across all frames).

## Windows integration and packaging (ADR 0008)
- Printing uses the classic Windows Print dialog. It offers all pages, one page range
  or the thumbnail selection, but no "current page" option. Preview's extra print
  options (scale percentage, several pages per sheet, layout direction) are not
  offered. Pages are fitted to the paper and centred, and the driver handles copies
  and collation.
- The newer Windows Print dialog isn't used, because it froze on a PC with an offline
  network printer (ADR 0008).
- The Share sheet and the Print dialog were checked on one Windows 11 PC by driving
  the real UI (ADR 0008). They aren't part of the automated test suite.
- Windows doesn't let programs make themselves the default. Paper Mirror registers
  itself and opens Settings > Default apps, where you choose.
- The installer and program aren't code-signed, so SmartScreen shows a warning the
  first time they run.
- There are no automatic updates, because Paper Mirror makes no network requests.
  Running a newer setup updates in place.

## 3D models (ADR 0009)
- STL, OBJ (with MTL colours), PLY and glTF/GLB are shown. USD/USDZ/USDA/USDC,
  Alembic, MaterialX and OpenVDB are recognised but not displayed, and each gets an
  explanation. No maintained local reader exists for them without large C++
  libraries, and MaterialX and OpenVDB need renderers Paper Mirror doesn't have.
- The view is drawn on the CPU, with no GPU. Textures, PBR material properties
  (metal, roughness, transparency), animation, skins and morph targets are ignored;
  surfaces show vertex or base colours.
- A model of about 1 million triangles takes about 70 ms per frame while dragging and
  about 110 ms when still (8-core CPU), so large models orbit at about 15 frames per
  second.
- Editing is limited to orientation: Rotate Left/Right about the vertical axis, and
  Flip Up Axis. Saving writes STL, OBJ, PLY or GLB. A glTF (.gltf) file is saved as
  GLB, through Save As. Export also writes an image of the view.
- Models can't be printed directly; export an image of the view first. Markup and
  measurements aren't available on models.
- External glTF buffers and OBJ material libraries must sit in the same folder as the
  model.
- Models can't be exported as a movie (turntable) or a PDF; export an image of the view.
