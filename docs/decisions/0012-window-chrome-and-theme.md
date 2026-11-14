# 0012 — Window chrome, accent colour, typeface and motion

Date: 2026-11-14 · Status: accepted

## Context
A visual pass within the existing stack: Slint 1.18 (Fluent style, software
renderer), muda's native menu bar, and the Paper Mirror theme in `ui/theme.slint`.
The screenshots before the pass showed four problems:
- The window's top was three bands: a near-white title bar, a white menu bar and the
  grey toolbar.
- With Paper Mirror set to Dark on a light Windows, the title bar, menu bar and Slint's
  own widgets (the search field) stayed light. Slint, and muda's dark menu bar,
  follow only the system theme.
- The accent was a fixed blue whatever the Windows accent colour.
- Text used Slint's default font rather than Windows 11's.

## Decision
1. **One surface across the top** (`platform_windows::chrome`).
   - The title bar takes the toolbar colour through DWM (caption colour, caption text
     colour and dark mode). Windows 10 ignores the colour attributes.
   - A window subclass paints the native menu bar in the theme's toolbar, text,
     disabled and hover colours by answering `WM_UAHDRAWMENU` / `WM_UAHDRAWMENUITEM`.
     It also covers the light line Windows draws under the menu bar. The technique
     follows muda's `dark_menu_bar.rs` (MIT OR Apache-2.0).
   - In high-contrast mode nothing is painted, so Windows' colours apply.
2. **Dark and light are decided in one place.**
   - Rust reads Windows' app mode (`AppsUseLightTheme`) into `Theme.system-dark`.
   - `Theme.dark` combines it with the Light/Dark/System setting.
   - Rust sets Slint's `Palette.color-scheme` to match, so Fluent widgets follow too.
   - Each window gets this when it registers. Before, a second window opened while
     Dark was forced came up in the system theme.
   - Changes reach the chrome through a `theme-dark` change callback, and through
     `WM_SETTINGCHANGE` for system changes. winit resets the title bar on that
     message, so the colours are re-applied after it.
3. **Accent:** the Windows accent colour (`DWM\AccentColor`) tints the theme's accent,
   focus ring and selection. Only its hue and colourfulness are used (OKLCH); each
   theme keeps its own lightness, so contrast stays as designed. This is the same
   approach the Fluent style takes.
4. **Typeface:** Segoe UI Variable Text, Windows 11's UI font. Slint falls back to its
   default font where it is missing.
5. **Motion:** hover, press and selection backgrounds fade over 90 ms
   (`Theme.motion-fast`). Panels don't slide, because resizing the canvas every frame
   would re-render pages.
6. **Icons stay Paper Mirror's own.** Segoe Fluent Icons has no counterpart for about a
   third of the tools (Sketch, Instant Alpha, Redact, Adjust Colour, Flip, line styles,
   Signature). Mixing two icon styles would look worse than one consistent set, which
   already uses Fluent's stroke weight.
7. **Not done: the GPU (Skia) renderer.** It would add smoother text, soft shadows and
   Mica translucency, for about 8–10 MB more download and a heavier build. The
   software renderer stays (ADR 0002).

## Verification
- The release build was captured in light and dark, before and after, with colour
  samples:
  - title bar, menu bar and toolbar are all `#ececec` in light (before:
    `#f4f5e6` / `#ffffff` / `#ececec`);
  - all `#2b2b2b` in dark (before: light title and menu bars over a dark app).
- Switching View > Appearance at run time repaints all three, in both directions.
- The menu labels are complete, and the search field is dark in Dark.
- Font change confirmed by comparing rendered inspector text pixel by pixel.
- All tests pass. The accent mapping was seen only with the default Windows blue,
  because the test PC's accent wasn't changed.
