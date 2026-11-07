# Feature checklist

Status of each requirement in `docs/discovery.md`. **Done** means an automated test
covers it, or a manual check is recorded (shown as *snapshot*: a release-build
screenshot taken with `--snapshot`). **Partial** and **Not done** are explained in
`docs/known-limitations.md`. Test names refer to `tests/*.rs`, and `unit:` means a unit
test inside `src/`.

## PDF viewing
| Feature | Status | Evidence |
|---|---|---|
| Open by dialog, drag-and-drop, command line, recent documents | Done | ui_shell `shell_end_to_end`; dialog and drop are native (manual) |
| Encrypted PDFs: password prompt, retries | Done | ui_shell, pdf_backend `password_protected_documents` |
| Malformed or non-PDF files handled gracefully | Done | pdf_backend `rejects_malformed_and_non_pdf`, `missing_file_is_an_io_error` |
| Thumbnails, contact sheet, table of contents, bookmarks | Done | ui_viewing `viewing_features`, pdf_viewing `outline_tree_and_destinations` |
| Continuous, single and two-page views; page navigation | Done | ui_viewing |
| Zoom in/out, actual size, fit page/width, zoom to selection | Done | ui_viewing; unit: core::layout |
| Sharp rendering at high zoom (detail tiles) | Done | pdf_backend `clipped_tile_matches_full_render` |
| Magnifier (loupe) | Done | ui_viewing; snapshot (M2) |
| Search with match navigation | Done | pdf_viewing `search_finds_needle_on_every_page_with_sane_rects`, ui_viewing |
| Text selection and copy; copy a region as an image | Done | ui_selection `text_selection_and_copy` |
| Links (internal and external, safe schemes only) | Done | pdf_viewing `links_resolve_to_pages_and_uris`; unit: shell `is_safe_url` |
| Document info (Inspector) | Done | pdf_viewing `document_info`, ui_viewing |
| Slideshow | Done | ui_viewing |

## Markup
| Feature | Status | Evidence |
|---|---|---|
| Highlight, underline, strikethrough | Done | ui_markup `markup_end_to_end` |
| Sketch (shape recognition), draw, shapes (line, arrow, rectangle, rounded, oval, bubble, star), loupe annotation | Done | ui_markup; unit: core::annot |
| Text boxes and notes in any script the system fonts cover | Partial | pdf_annotations; no complex-script shaping |
| Style controls; move, resize, delete; undo/redo | Done | ui_markup |
| Editable after save; standard PDF annotations with appearances | Done | pdf_annotations `annotations_round_trip_through_save_and_reopen`, `saved_appearances_render_in_place` |
| Flattened export | Done | pdf_annotations `flattening_merges_annotations_into_content` |
| True redaction (text, images, graphics) | Done | pdf_annotations `redaction_removes_text_but_keeps_the_rest_of_the_line`, `redaction_blacks_out_image_pixels_in_the_data` |

## Forms and signatures
| Feature | Status | Evidence |
|---|---|---|
| Detect and fill text, check, radio, choice fields; save | Done | pdf_forms `fills_and_saves_every_field_type`, ui_forms |
| Signatures drawn with the mouse or pen (a finger reaches the same drawing pad, not tested separately), or traced from an image | Done | ui_forms `forms_and_signatures`, `signature_images_are_traced` |
| Signatures stored encrypted for the user (DPAPI) | Done | unit: storage::signatures, platform_windows::dpapi |

## Pages, effects, security, files
| Feature | Status | Evidence |
|---|---|---|
| Rotate, delete, drag-reorder, insert blank, insert/combine from file, crop, multi-select, undo | Done | pdf_pages, ui_pages `page_organisation_end_to_end` |
| Insert images as pages | Done | ui_images |
| PDF effects, Reduce File Size | Done | pdf_pages `effects_change_colours_but_keep_text`, `reduce_file_size_downsamples_large_images` |
| Password protection (AES-256) | Done | pdf_annotations `saving_with_protection_and_preserving_encryption`, pdf_backend `encrypted_bytes_do_not_contain_plain_text` |
| Save, Save As, Export, Duplicate, Revert To Last Saved/Original, Lock | Done | ui_pages, ui_markup |
| Original kept before the first overwrite | Done | ui_markup, ui_images |
| Print (Windows Print dialog) | Done | pdf_print `prints_selected_pages_to_a_pdf_printer`, ui_platform `print_and_share`, unit: print_dialog; real-UI check (Ctrl+P → dialog → Microsoft Print to PDF → saved 3-page file), ADR 0008 |
| Share (Windows Share sheet) | Done | ui_platform covers preparing the file; real-UI check (Share button → Windows 11 Share sheet for the file), ADR 0008 |

## Images
| Feature | Status | Evidence |
|---|---|---|
| PNG, JPEG, GIF frames, BMP, ICO, WebP, OpenEXR, TIFF pages | Done | unit: images::decode, images::encode |
| HEIC/HEIF (Windows codecs) | Done | unit: wic `heic_round_trip_when_codecs_are_installed` (codec present on the test machine) |
| JPEG 2000 (view) | Done | pdf_images `jpeg2000_decodes_through_pdfium`, ui_images |
| Pixel-exact viewing and saving | Done | pdf_images `image_pages_round_trip_pixel_for_pixel` |
| Markup merged into pixels on save | Done | pdf_images `markup_is_baked_only_where_drawn`, ui_images |
| Rotate, flip, crop to pixels, Adjust Size, Adjust Color, Instant Alpha | Done | ui_images `images_end_to_end`; unit: images::edit, core::imagesize |
| EXIF in the Inspector; location shown only on request; Remove Location | Done | ui_images; unit: images::metadata |
| Colour profiles used for display and kept on save | Done | unit: images `srgb_profile_needs_no_transform`, encode `png_keeps_icc_and_exif` |
| Convert/export keeping transparency and metadata | Done | ui_images, unit: encode |
| Print and share images | Done | Same paths as PDFs |
| ML background removal | Not done | Instant Alpha instead |
| Import from camera or scanner | Not done | New from Clipboard instead (ui_images) |

## 3D
| Feature | Status | Evidence |
|---|---|---|
| STL, OBJ, PLY, glTF/GLB: view, orbit, pan, zoom | Done | ui_model `models_end_to_end`; unit: model3d::*; snapshot (M9) |
| Limited editing (orientation) and export to STL/OBJ/PLY/GLB/images | Done | ui_model, unit: model3d::export |
| USD/USDZ, Alembic, MaterialX, OpenVDB | Not done | Explained on open (ui_model); ADR 0009 |
| Export as movie or PDF | Not done | Export an image of the view instead |

## Shell and platform
| Feature | Status | Evidence |
|---|---|---|
| Preview-like window, toolbar, sidebar, Markup toolbar, full menus, context menus | Done | ui_shell; snapshots (M1–M9) |
| Keyboard shortcuts documented (Help > Keyboard Shortcuts) and in sync with menus | Done | unit: core::commands `ui_menus_match_registry` |
| Light/dark appearance, resizable sidebar, multiple windows | Done | ui_shell |
| High-DPI (per-monitor v2 manifest) | Done | Manifest in place |
| Touch: tap, one-finger pan, pinch to zoom, finger drawing in markup tools, 3D orbit, slideshow swipe | Done | Injected touch on the release build, ADR 0011; unit: view `pinch_tests`, touch |
| Pen: selects, draws and clicks like a mouse | Done | Injected pen on the release build, ADR 0011 |
| Accessibility: names, roles, states and actions for UI Automation (Narrator); page text readable; menus via MSAA | Done | UI Automation and MSAA audit of the release build, ADR 0011 |
| Own licence (MIT) and code signing in packaging | Done | ADR 0010; installer `pack_install_and_uninstall` (signed layout); test-signed package run |
| Opt-in file associations, Default apps | Done | unit: assoc `register_and_unregister_under_a_scratch_key`, installer test |
| Per-user installer, update, uninstall; portable zip | Done | installer `pack_install_and_uninstall`; manual run of the packaged setup (M8) |
| No network, telemetry, background services or auto-start | Done | ADR 0005; no networking crates among the linked crates listed in THIRD-PARTY-NOTICES.txt |
| Bounded caches, lazy rendering, responsive UI | Done | ADR 0004; docs/performance.md |
