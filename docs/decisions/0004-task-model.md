# 0004 — Threading and task model

Date: 2026-11-06 · Status: accepted

## Decision
- One **PDF engine thread** owns PDFium and every open document. The UI submits
  closures with a priority (Interactive > Render > Thumbnail > Background).
- Jobs may carry a **generation token**; the UI bumps a counter on scroll/zoom so
  queued renders for positions the user already left are dropped without running.
- Results return to the UI thread with `slint::invoke_from_event_loop`; pixel buffers are
  `SharedPixelBuffer`s that PDFium renders into directly (zero copy).
- Jobs run under `catch_unwind`, so one bad document cannot kill the engine.
- No async runtime: a channel, a binary heap and plain threads cover the need with zero
  extra dependencies. Image decoding (pure Rust) uses short-lived worker threads.

## Consequences
Long PDF operations (search, export) are written as incremental jobs that re-queue
themselves so renders stay responsive.
