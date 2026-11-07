# 0009 — 3D models: feasibility and viewer

Date: 2026-11-07 · Status: accepted

## Context
The product spec asks for viewing, limited editing and export of USD/USDZ/USDA/USDC,
PLY, STL, Alembic (Ogawa), glTF/GLB, OBJ, MaterialX and OpenVDB, and calls this
explicitly a stretch area. Paper Mirror draws with Slint's software renderer (ADR 0002:
no GPU dependency, small memory), runs offline and treats files as untrusted.

## Feasibility (surveyed 2026-11)
| Format | Local, safe, maintainable reader? | Decision |
|---|---|---|
| STL (ASCII, binary) | Trivial format; own parser. | View, export |
| OBJ + MTL colours | Simple text format; own parser. | View, export |
| PLY (ASCII, binary LE/BE) | Simple header plus data; own parser. | View, export |
| glTF 2.0 / GLB | `gltf` 1.4.1 (MIT/Apache, pure Rust, widely used) for JSON/GLB and accessors. | View, export as other formats |
| USDZ / USDA / USDC | No maintained pure-Rust reader. USDC is a complex binary crate format, and OpenUSD is a large C++ dependency. | Not supported: clear message |
| Alembic (Ogawa) | Only the C++ reference library (and HDF5). | Not supported |
| MaterialX | A material description, not geometry; needs a renderer with shader generation. | Not supported |
| OpenVDB | Sparse volumes; C++ library with Boost/TBB. A volume renderer is out of scope. | Not supported |

## Decision
1. **Model.** Each file becomes one triangle mesh: positions, per-vertex normals
   (smoothed when the file has none; STL is flat-shaded) and optional colours (vertex
   colours, OBJ `Kd` or glTF `baseColorFactor`). glTF node transforms are applied, and
   textures are ignored. Hard limits: 20 million triangles and 2 GB of input.
2. **Rendering.** A CPU rasteriser runs in parallel bands on scoped threads, with a
   z-buffer, near-plane clipping, two-sided Lambert plus specular lighting from a
   head-light and a key light, and 2× supersampling when the view is still. It renders
   on a per-window thread with generation tokens, at reduced resolution while dragging.
   No GPU, no shaders, the same memory profile as the rest of the app.
3. **Interaction.** Drag orbits, Shift+drag or right-drag pans, the wheel or Ctrl+wheel
   zooms, and View > Actual Size resets the view. Tools > Rotate Left and Rotate Right
   turn the model about the vertical axis. Tools > Flip Up Axis switches between Y-up
   and Z-up, for CAD files.
4. **Export (limited editing).** The orientation edits are written into the mesh.
   Export goes to binary STL, OBJ, binary PLY or GLB (geometry, normals and colours),
   or to an image of the current view (PNG, JPEG, TIFF and so on) at twice the window
   size.
5. **Unsupported formats** are detected by signature or extension and get a specific
   explanation instead of a generic error.

## Dependencies
`gltf` 1.4.1 (MIT OR Apache-2.0) with only `utils` and `names`. Its `import` feature,
which pulls in image decoding and URI handling, is off. Buffers are resolved by Paper
Mirror: GLB blobs, `data:` URIs, and sibling `.bin` files, never paths outside the
model's folder.

## Consequences
Typical models (up to a few million triangles) orbit interactively on a multi-core
CPU. Textures, PBR materials, animation and skins aren't shown. The unsupported
formats are listed in docs/known-limitations.md.
