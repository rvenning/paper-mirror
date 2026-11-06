# 0002 — Slint 1.18.1, licence, renderer and style

Date: 2026-11-06 · Status: accepted

## Decision
- Slint 1.18.1 (pinned `=1.18.1`), winit backend, **software renderer only**, Fluent
  widget style, AccessKit accessibility, native menu bar (muda) via `MenuBar`.
- Licence: **Slint Royalty-free Desktop, Mobile and Web Applications Licence 2.0**. Its
  attribution condition is met by the `AboutSlint` widget in Help > About Paper Mirror.
  Restrictions honoured: not an embedded system, Slint APIs are not re-exposed, notices
  kept intact.
- `unstable-winit-030` (pinned with Slint) is used only to receive OS file drops
  (`WindowEvent::DroppedFile`), which Slint 1.18 does not surface.

## Evidence (prototype, release build, 120-page fixture, same machine)
| Renderer | Event loop running | First page | Working set after first paint |
|---|---|---|---|
| software | 60–350 ms | 255–553 ms | 34.3 MiB |
| femtovg (OpenGL) | 248–1058 ms | 767–6937 ms | 87.6–144 MiB |

The software renderer starts faster, uses less than half the memory and avoids GPU
driver initialisation stalls. Page bitmaps are produced by PDFium anyway, so a GPU path
renderer buys little. femtovg was removed from the build.

## Consequences
Very large windows on 4K displays repaint on the CPU; Slint's partial rendering keeps
scrolling cost proportional to the damaged area. Re-evaluate only with evidence.
