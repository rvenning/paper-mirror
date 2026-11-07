# Known limitations

Collected as each milestone lands; M10 reviews the whole list against the final build.

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
