# 0011 — Touch, pen and screen-reader support

Date: 2026-11-07 · Status: accepted

## Context
Touch, pen and Narrator support had never been checked. The development PC has no
touch screen or pen, so input was generated with Windows' own injection APIs:
- `InjectTouchInput` for one- and two-finger touch;
- `CreateSyntheticPointerDevice` / `InjectSyntheticPointerInput` for a pen.

Both go through the same input stack as hardware: pointer messages, then promotion to
`WM_TOUCH` and mouse messages. The release build was driven this way, and the results
were read back through screenshots and UI Automation.

## Findings
1. **Touch did nothing**, for two reasons:
   - winit registers for `WM_TOUCH` only when a touch screen is ready when the window
     opens.
   - Otherwise winit takes touch from `WM_POINTER`, placing points through the
     device's HIMETRIC rectangle. That drops input from devices that report none.
2. **A pen would have scrolled instead of drawing.** winit turns pen input into touch
   events, and Slint's ScrollView pans on any touch drag. A pen stroke in a markup tool
   would scroll the page.
3. **One finger could never draw.** In a markup tool the ScrollView took the drag.
4. **Pinch-to-zoom didn't exist.**
5. **The first click after focus moved was lost, for mouse users too.** Overlay
   `FocusScope`s (sidebar, canvas, toolbar buttons, form checkboxes, dialogs) used
   Slint's default `focus-on-click`. A FocusScope that isn't focused takes focus on a
   press and accepts it, so the control underneath never saw the press. For example,
   the first click on a thumbnail, or on the Markup button, did nothing.
6. **Screen-reader gaps:**
   - pages were images, so a PDF's text couldn't be read;
   - thumbnails were announced as bare numbers;
   - decorative images inside them were unnamed;
   - the sidebar tabs weren't a tab list;
   - on/off and tool buttons (Markup tools, sidebar, inspector) and form checkboxes
     didn't expose whether they were on;
   - each Inspector row was read twice, and buttons repeated their own label.

## Decision
1. **`platform_windows::touch` subclasses each document and slideshow window.**
   - Touch and pen pointer messages skip winit and go to `DefWindowProcW`, which
     promotes them:
     - touch becomes `WM_TOUCH` (the window is always registered), which winit turns
       into touch events from pixel positions;
     - pen becomes mouse messages, so it selects, draws and clicks exactly like a
       mouse. Windows also turns a pen press-and-hold into a right-click this way;
       that wasn't exercised.
   - Two duplicates are dropped: the mouse copies Windows makes from touch (identified
     with `GetCurrentInputMessageSource`, because synthetic pens also set the
     documented "touch" bit), and the `WM_TOUCH` Windows also makes from the pen.
   - Pressure is lost; nothing uses it.
2. **One finger:**
   - pans in Text Selection mode (the Windows convention: the finger scrolls, the pen
     writes);
   - draws, selects or moves in the markup tools and Rectangular Selection;
   - orbits in the 3D view.

   In those modes a layer above the ScrollView takes pointer input.
3. **Two fingers** pinch to zoom about their midpoint and pan as the midpoint moves, in
   every mode, through a `ScaleRotateGestureHandler` that wraps the canvas.
   - It must enclose the content, because the first finger's press grabs the pointer
     and gesture events then only travel through the grabber's ancestors.
   - Slint compounds its scale steps (`scale × (1 + Δratio)`), so a three-fold spread
     reads as about 5.4. The true distance ratio is rebuilt from successive scales,
     with a unit test.
4. **Slideshow:** a tap or click shows the next slide, and a swipe to the right shows
   the previous one.
5. **Overlay FocusScopes** set `focus-on-click: false`.
   - The controls underneath take focus themselves.
   - Toolbar buttons leave focus where it was, as in Windows toolbars.
   - The two Escape-to-close scopes now take focus when their panel opens, so Escape
     works immediately.
6. **Screen readers:**
   - Each page is exposed as text, read as "Page N of M." followed by its extracted
     text, once that has loaded; scans and images stay images.
   - Thumbnails are "Page N" (", bookmarked" when they are); their images and number
     labels are hidden.
   - The sidebar tabs form a selectable tab list.
   - Decorative page bitmaps are hidden.
   - On/off and tool buttons and form checkboxes are checkable, so UI Automation
     reports and changes their state through the Toggle pattern.
   - Inspector rows read once ("Name: touch.pdf"; headings without a colon).
   - Plain buttons hide their inner label.

## Verification
- Every finding above was reproduced first, and then checked fixed, on the release
  build with injected input:
  - a tap opens a thumbnail;
  - one finger pans 250 px without selecting;
  - a 120→360 px pinch zooms about 2.7× about the midpoint;
  - a pen stroke selects the same text as a mouse drag;
  - in Draw, pen and finger each draw one stroke, and a pinch adds no stray mark
    (UI Automation counts two drawings);
  - one finger orbits a model and a pinch zooms it;
  - in the slideshow, tap → 2, swipe right → 1, swipe left → 2, pen tap → 3.
- The first-click loss was confirmed in the build from before this change: a click on
  thumbnail 3 stayed on page 1, and a second click worked.
- The UI Automation tree of the release build was inspected through
  `UIAutomationClient` (what Narrator reads), for a PDF, a form, an image with Adjust
  Size open, and with the Markup toolbar and Inspector open:
  - No element has an empty name.
  - Buttons are invoked, tabs and thumbnails selected, and tools toggled through UI
    Automation (Draw freehand on → Text selection off).
  - Tab moves through the toolbar, the search field, the thumbnail list and the
    document in order.
  - The page reads "Page 1 of 3. Fixture page 1 …".
  - Native menus are read through MSAA (what Narrator's built-in proxy uses): each item
    has its name, shortcut, access key, role and state.
  - Narrator itself was not run, because its speech can't be checked from here.

## Consequences
- A real touch screen and pen should be tried when one is at hand. The injection path
  is the same one the hardware drivers feed, but palm rejection, pen buttons and
  hovering over thumbnails can only be seen on hardware.
- Long-press with a finger doesn't open context menus, because touch arrives as raw
  `WM_TOUCH`. Use a pen press-and-hold or a right-click instead.
