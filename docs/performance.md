# Performance

Measured 2026-11-07 on the release build (`cargo build --release`, Slint software
renderer). The handoff asked that targets not be invented before a baseline existed
(discovery conflict 8). The baseline below comes from the M0 prototype; the guardrails
at the end are set from it.

## Test machine
Intel Core i7-7700 (4 cores / 8 threads, 3.6 GHz, 2017), 16 GB RAM, NVMe SSD,
Windows 11 Home 10.0.26100. This is a modest machine; most current PCs are faster.

## How to reproduce
```powershell
cargo build --release
.\scripts\measure.ps1 -Runs 5 -Extra 'C:\path\to\some.pdf', 'C:\path\to\photo.jpg'
cargo run --release --example bench3d -- 1000000
```
`--measure` prints time since process start and the process memory at fixed points:
- `event-loop-running`: the window can take input.
- `document-opened`: the file is parsed.
- `first-page-rendered`: the first page's pixels have arrived.
- `idle`: a second after startup, with no document.

Each figure below is the median of five runs. Working set is resident memory as Task
Manager shows it.

## Baseline (M0 prototype, 2026-11-04)
| Measure | Software renderer | femtovg (OpenGL) |
|---|---|---|
| Event loop running | 60–350 ms | — |
| First page rendered (3-page fixture) | 255–553 ms | — |
| Working set with a document | ~34 MiB | 88–144 MiB, multi-second stalls |

The software renderer was kept (ADR 0002).

## Release build (M10)
| Scenario | Interactive (event loop) | Opened | First page rendered | Working set | Peak |
|---|---|---|---|---|---|
| Start, no document | 58 ms | — | — | 28 MiB idle | 28 MiB |
| PDF, 3 pages (text fixture) | 58 ms | 183 ms | **238 ms** | 36 MiB | 37 MiB |
| PDF, 120 pages (fixture) | 54 ms | 164 ms | **220 ms** | 37 MiB | 37 MiB |
| Image-heavy PDF manual, 5.3 MB (third-party file on the test machine) | 59 ms | 182 ms | **338 ms** | 44 MiB | 57 MiB |
| PNG 1200 × 800 with transparency | 50 ms | 162 ms | **233 ms** | 41 MiB | 46 MiB |
| JPEG 6000 × 4000 (24 MP) | 52 ms | 195 ms | **305 ms** | 102 MiB | 190 MiB |
| PLY model, 1.0 M triangles (25 MB) | 58 ms | 192 ms | **393 ms** | 58 MiB | 84 MiB |

Other measurements:
- **3D rasteriser** (`examples/bench3d.rs`): 1.0 M triangles take 67 ms per frame
  while dragging (half resolution) and 107 ms per still 1280 × 800 frame with 2×
  supersampling. 100 k triangles take 11 ms and 42 ms.
- **Large images:** converting pixels in place when building the page document cut
  the 24 MP JPEG peak from 284 MiB to 192 MiB (M7).
- **Pages:** rendering, thumbnails and search run on the engine thread with
  generation tokens, so a stale request never blocks the latest one. Rendered pages
  live in a 160 MB LRU, thumbnails in 24 MB, and the contact sheet in 32 MB (ADR 0004).
- **Sizes:** `paper-mirror.exe` 25.0 MB, `pdfium.dll` 7.8 MB, setup program 14.4 MB,
  portable zip 13.9 MB.

## Compared with the baseline
- Time to interactive fell from 60–350 ms to 50–59 ms. Window creation and the first
  frame are no longer delayed by the document (it opens on worker threads).
- Time to the first page fell from 255–553 ms to 220–340 ms for PDFs, despite M1–M9
  adding menus, forms, markup, images and 3D.
- Memory with a typical PDF is 36–44 MiB, against about 34 MiB for the much smaller
  prototype. Idle memory without a document is 28 MiB.

## Guardrails
Checked by `scripts/measure.ps1` before a release, on comparable hardware:

| Guardrail | Limit | Now |
|---|---|---|
| Interactive after launch | < 150 ms | 50–59 ms |
| First page of the fixtures | < 500 ms | 220–238 ms |
| First page of a large image or PDF | < 800 ms | 305–393 ms |
| Idle working set, no document | < 40 MiB | 28 MiB |
| Working set, typical PDF | < 64 MiB | 36–44 MiB |
| Peak working set, 24 MP photo | < 256 MiB | 190 MiB |
| 1 M-triangle model, still frame | < 200 ms | 107 ms |

## Not measured
- Frame times while scrolling, and input latency, weren't measured. Rendering is
  asynchronous: scrolling shows the cached or blurred thumbnail at once, and sharp
  pages replace it as they arrive.
- Cold start right after a reboot wasn't measured separately. The figures above are
  warm (files in the OS cache).
