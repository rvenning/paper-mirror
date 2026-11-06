//! Page layout for the document canvas: display modes, zoom, fit calculations,
//! virtualised visible-page queries and view <-> page coordinate transforms.
//!
//! All view values are logical pixels. Page sizes are in PDF points of the page as
//! displayed (i.e. already swapped for 90/270 degree rotation).

use super::geometry::{Point, Rect, Size};

/// Logical pixels per PDF point at 100 % ("Actual Size"): Windows uses 96 logical
/// pixels per inch, PDF uses 72 points per inch.
pub const PX_PER_PT: f32 = 96.0 / 72.0;
pub const MIN_ZOOM: f32 = 0.08;
pub const MAX_ZOOM: f32 = 32.0;
/// Margin around the page column, in logical pixels.
pub const MARGIN: f32 = 16.0;
/// Gap between consecutive pages and between the two pages of a spread.
pub const GAP: f32 = 12.0;

/// Preview's View menu: Continuous Scroll, Single Page, Two Pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayMode {
    #[default]
    Continuous,
    SinglePage,
    TwoPages,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ZoomMode {
    /// Explicit zoom factor where 1.0 is Actual Size.
    Custom(f32),
    #[default]
    FitWidth,
    FitPage,
}

pub fn clamp_zoom(z: f32) -> f32 {
    if z.is_finite() { z.clamp(MIN_ZOOM, MAX_ZOOM) } else { 1.0 }
}

/// Preset zoom steps used by Zoom In / Zoom Out.
const ZOOM_STEPS: &[f32] =
    &[0.08, 0.12, 0.25, 0.33, 0.5, 0.67, 0.75, 0.85, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0];

pub fn zoom_in_step(z: f32) -> f32 {
    ZOOM_STEPS.iter().copied().find(|s| *s > z * 1.001).unwrap_or(MAX_ZOOM)
}

pub fn zoom_out_step(z: f32) -> f32 {
    ZOOM_STEPS.iter().rev().copied().find(|s| *s < z * 0.999).unwrap_or(MIN_ZOOM)
}

/// Result of laying out a document at a given zoom.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// One rectangle per page in content coordinates. Pages not shown in the current
    /// mode (Single Page) get an empty rectangle.
    pub rects: Vec<Rect>,
    pub content: Size,
    /// Logical pixels per point.
    pub scale: f32,
    pub mode: DisplayMode,
}

/// Groups pages into display rows (one page, or a two-page spread).
fn rows(page_count: usize, mode: DisplayMode) -> Vec<(usize, Option<usize>)> {
    match mode {
        DisplayMode::TwoPages => {
            (0..page_count).step_by(2).map(|i| (i, (i + 1 < page_count).then_some(i + 1))).collect()
        }
        _ => (0..page_count).map(|i| (i, None)).collect(),
    }
}

fn row_size(pages: &[Size], row: (usize, Option<usize>)) -> Size {
    let a = pages[row.0];
    match row.1 {
        Some(b) => {
            let b = pages[b];
            Size::new(a.w + b.w, a.h.max(b.h))
        }
        None => a,
    }
}

/// Row containing `page` for the given mode.
pub fn row_of(page: usize, mode: DisplayMode) -> (usize, Option<usize>) {
    match mode {
        DisplayMode::TwoPages => {
            let first = page - page % 2;
            (first, Some(first + 1))
        }
        _ => (page, None),
    }
}

/// Computes the zoom factor (1.0 = Actual Size) for a fit mode.
pub fn fit_zoom(pages: &[Size], mode: DisplayMode, fit: ZoomMode, viewport: Size, current: usize) -> f32 {
    if pages.is_empty() {
        return 1.0;
    }
    let gap_w = if mode == DisplayMode::TwoPages { GAP } else { 0.0 };
    let avail_w = (viewport.w - 2.0 * MARGIN - gap_w).max(32.0);
    let avail_h = (viewport.h - 2.0 * MARGIN).max(32.0);
    let current = current.min(pages.len() - 1);
    match fit {
        ZoomMode::Custom(z) => clamp_zoom(z),
        ZoomMode::FitWidth => {
            // Fit the widest row so horizontal scrolling never appears.
            let widest = rows(pages.len(), mode)
                .into_iter()
                .map(|r| {
                    let r = (r.0, r.1.filter(|b| *b < pages.len()));
                    row_size(pages, r).w
                })
                .fold(1.0_f32, f32::max);
            clamp_zoom(avail_w / (widest * PX_PER_PT))
        }
        ZoomMode::FitPage => {
            let (a, b) = row_of(current, mode);
            let row = row_size(pages, (a, b.filter(|b| *b < pages.len())));
            clamp_zoom((avail_w / (row.w * PX_PER_PT)).min(avail_h / (row.h * PX_PER_PT)))
        }
    }
}

/// Lays out all pages. `viewport` is used to centre content narrower than the window
/// and, in Single Page mode, to centre the page vertically.
pub fn layout(pages: &[Size], mode: DisplayMode, zoom: f32, viewport: Size, current: usize) -> Layout {
    let scale = clamp_zoom(zoom) * PX_PER_PT;
    let mut rects = vec![Rect::default(); pages.len()];
    if pages.is_empty() {
        return Layout { rects, content: viewport, scale, mode };
    }
    let all_rows = rows(pages.len(), mode);
    let row_width = |r: (usize, Option<usize>)| {
        let s = row_size(pages, r);
        s.w * scale + if r.1.is_some() { GAP } else { 0.0 }
    };

    let shown: Vec<(usize, Option<usize>)> = match mode {
        DisplayMode::SinglePage => vec![(current.min(pages.len() - 1), None)],
        _ => all_rows,
    };
    let widest = shown.iter().map(|r| row_width(*r)).fold(0.0_f32, f32::max);
    let content_w = (widest + 2.0 * MARGIN).max(viewport.w);

    let mut y = MARGIN;
    for row in &shown {
        let rs = row_size(pages, *row);
        let row_h = rs.h * scale;
        let mut x = (content_w - row_width(*row)) / 2.0;
        for idx in std::iter::once(row.0).chain(row.1) {
            let p = pages[idx];
            let (w, h) = (p.w * scale, p.h * scale);
            rects[idx] = Rect::new(x, y + (row_h - h) / 2.0, w, h);
            x += w + GAP;
        }
        y += row_h + GAP;
    }
    let mut content_h = y - GAP + MARGIN;
    if mode == DisplayMode::SinglePage && content_h < viewport.h {
        // Centre a single page vertically when it fits.
        let dy = (viewport.h - content_h) / 2.0;
        for r in rects.iter_mut().filter(|r| !r.is_empty()) {
            r.y += dy;
        }
        content_h = viewport.h;
    }
    Layout { rects, content: Size::new(content_w, content_h.max(viewport.h)), scale, mode }
}

impl Layout {
    /// Indices of pages intersecting the vertical band `[top, bottom]`, in order.
    /// In the scrolling modes rects are sorted by `y`, so a binary search finds the start.
    pub fn visible(&self, top: f32, bottom: f32) -> Vec<usize> {
        if self.mode == DisplayMode::SinglePage {
            return (0..self.rects.len())
                .filter(|i| {
                    let r = &self.rects[*i];
                    !r.is_empty() && r.bottom() >= top && r.y <= bottom
                })
                .collect();
        }
        let start = self.rects.partition_point(|r| r.bottom() < top);
        let start = start.saturating_sub(1);
        self.rects
            .iter()
            .enumerate()
            .skip(start)
            .filter(|(_, r)| !r.is_empty())
            .take_while(|(_, r)| r.y <= bottom)
            .filter(|(_, r)| r.bottom() >= top)
            .map(|(i, _)| i)
            .collect()
    }

    /// The page that best represents the viewport: the one covering the viewport's
    /// vertical centre, otherwise the nearest one.
    pub fn current_page(&self, top: f32, viewport_h: f32) -> usize {
        let probe = top + viewport_h * 0.35;
        let mut best = 0;
        let mut best_d = f32::MAX;
        for (i, r) in self.rects.iter().enumerate() {
            if r.is_empty() {
                continue;
            }
            let d = if probe < r.y {
                r.y - probe
            } else if probe > r.bottom() {
                probe - r.bottom()
            } else {
                0.0
            };
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best
    }

    /// Maps a content point to `(page, point in page display space)`. Page display
    /// space is points with the origin at the top-left of the displayed page.
    pub fn page_at(&self, p: Point) -> Option<(usize, Point)> {
        self.rects.iter().position(|r| !r.is_empty() && r.contains(p)).map(|i| (i, self.to_page(i, p)))
    }

    /// Nearest page for a point that may fall between pages (used for drags).
    pub fn nearest_page(&self, p: Point) -> Option<usize> {
        self.rects
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_empty())
            .min_by(|(_, a), (_, b)| {
                let da = dist_to_rect(a, p);
                let db = dist_to_rect(b, p);
                da.total_cmp(&db)
            })
            .map(|(i, _)| i)
    }

    pub fn to_page(&self, page: usize, p: Point) -> Point {
        let r = self.rects[page];
        Point::new((p.x - r.x) / self.scale, (p.y - r.y) / self.scale)
    }

    pub fn to_view(&self, page: usize, p: Point) -> Point {
        let r = self.rects[page];
        Point::new(r.x + p.x * self.scale, r.y + p.y * self.scale)
    }

    pub fn rect_to_view(&self, page: usize, r: Rect) -> Rect {
        let o = self.to_view(page, Point::new(r.x, r.y));
        Rect::new(o.x, o.y, r.w * self.scale, r.h * self.scale)
    }

    pub fn rect_to_page(&self, page: usize, r: Rect) -> Rect {
        let o = self.to_page(page, Point::new(r.x, r.y));
        Rect::new(o.x, o.y, r.w / self.scale, r.h / self.scale)
    }

    /// Scroll offset that puts the top of `page` at the top of the viewport.
    pub fn scroll_to_page(&self, page: usize) -> f32 {
        self.rects.get(page).map(|r| (r.y - MARGIN).max(0.0)).unwrap_or(0.0)
    }
}

fn dist_to_rect(r: &Rect, p: Point) -> f32 {
    let dx = (r.x - p.x).max(0.0).max(p.x - r.right());
    let dy = (r.y - p.y).max(0.0).max(p.y - r.bottom());
    dx * dx + dy * dy
}

/// After a zoom change, computes the new scroll offset that keeps the document point
/// under `anchor` (a viewport-relative position) fixed on screen.
pub fn anchored_scroll(old: &Layout, new: &Layout, old_scroll: Point, anchor: Point) -> Point {
    let content_pt = Point::new(old_scroll.x + anchor.x, old_scroll.y + anchor.y);
    let target =
        match old.page_at(content_pt).or_else(|| old.nearest_page(content_pt).map(|i| (i, old.to_page(i, content_pt))))
        {
            Some((page, pp)) if page < new.rects.len() && !new.rects[page].is_empty() => new.to_view(page, pp),
            _ => {
                let f = new.scale / old.scale;
                Point::new(content_pt.x * f, content_pt.y * f)
            }
        };
    Point::new((target.x - anchor.x).max(0.0), (target.y - anchor.y).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(n: usize) -> Vec<Size> {
        vec![Size::new(612.0, 792.0); n]
    }

    #[test]
    fn continuous_layout_stacks_pages() {
        let l = layout(&letter(3), DisplayMode::Continuous, 1.0, Size::new(800.0, 600.0), 0);
        let h = 792.0 * PX_PER_PT;
        assert_eq!(l.rects[0].y, MARGIN);
        assert!((l.rects[1].y - (MARGIN + h + GAP)).abs() < 0.01);
        assert!((l.content.h - (MARGIN * 2.0 + 3.0 * h + 2.0 * GAP)).abs() < 0.01);
        // Wider than the viewport: content width is page width plus margins.
        assert!((l.content.w - (612.0 * PX_PER_PT + 2.0 * MARGIN)).abs() < 0.01);
    }

    #[test]
    fn narrow_pages_are_centred() {
        let l = layout(&letter(1), DisplayMode::Continuous, 0.5, Size::new(1000.0, 600.0), 0);
        let w = 612.0 * 0.5 * PX_PER_PT;
        assert!((l.rects[0].x - (1000.0 - w) / 2.0).abs() < 0.01);
        assert_eq!(l.content.w, 1000.0);
    }

    #[test]
    fn two_page_spreads_and_odd_tail() {
        let l = layout(&letter(3), DisplayMode::TwoPages, 1.0, Size::new(400.0, 400.0), 0);
        assert_eq!(l.rects[0].y, l.rects[1].y);
        assert!(l.rects[1].x > l.rects[0].right());
        assert!(l.rects[2].y > l.rects[0].bottom());
        // The odd last page is centred on its own row.
        let mid = l.content.w / 2.0;
        assert!((l.rects[2].center().x - mid).abs() < 0.01);
    }

    #[test]
    fn single_page_mode_shows_only_current() {
        let l = layout(&letter(5), DisplayMode::SinglePage, 0.3, Size::new(800.0, 900.0), 2);
        assert!(l.rects[0].is_empty() && l.rects[4].is_empty());
        assert!(!l.rects[2].is_empty());
        assert_eq!(l.visible(0.0, 900.0), vec![2]);
    }

    #[test]
    fn fit_width_and_page() {
        let vp = Size::new(800.0, 600.0);
        let z = fit_zoom(&letter(2), DisplayMode::Continuous, ZoomMode::FitWidth, vp, 0);
        let l = layout(&letter(2), DisplayMode::Continuous, z, vp, 0);
        assert!((l.rects[0].w - (800.0 - 2.0 * MARGIN)).abs() < 0.5);
        let z = fit_zoom(&letter(2), DisplayMode::Continuous, ZoomMode::FitPage, vp, 0);
        let l = layout(&letter(2), DisplayMode::Continuous, z, vp, 0);
        assert!((l.rects[0].h - (600.0 - 2.0 * MARGIN)).abs() < 0.5);
        assert!(l.rects[0].w < 800.0);
    }

    #[test]
    fn mixed_page_sizes_fit_widest() {
        let pages = vec![Size::new(612.0, 792.0), Size::new(1224.0, 792.0)];
        let vp = Size::new(800.0, 600.0);
        let z = fit_zoom(&pages, DisplayMode::Continuous, ZoomMode::FitWidth, vp, 0);
        let l = layout(&pages, DisplayMode::Continuous, z, vp, 0);
        assert!(l.content.w <= 800.0 + 0.5);
    }

    #[test]
    fn visible_range_is_virtualised() {
        let l = layout(&letter(500), DisplayMode::Continuous, 1.0, Size::new(800.0, 600.0), 0);
        let top = l.rects[250].y + 10.0;
        let v = l.visible(top, top + 600.0);
        assert_eq!(v.first(), Some(&250));
        assert!(v.len() <= 2, "only pages in the band are visible: {v:?}");
        assert_eq!(l.current_page(top, 600.0), 250);
    }

    #[test]
    fn coordinate_round_trip() {
        let l = layout(&letter(2), DisplayMode::Continuous, 1.7, Size::new(800.0, 600.0), 0);
        let pp = Point::new(100.0, 200.0);
        let v = l.to_view(1, pp);
        let (page, back) = l.page_at(v).unwrap();
        assert_eq!(page, 1);
        assert!((back.x - pp.x).abs() < 0.01 && (back.y - pp.y).abs() < 0.01);
        assert!(l.page_at(Point::new(1.0, 1.0)).is_none());
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let pages = letter(10);
        let vp = Size::new(800.0, 600.0);
        let old = layout(&pages, DisplayMode::Continuous, 1.0, vp, 0);
        let scroll = Point::new(0.0, old.rects[3].y + 100.0);
        let anchor = Point::new(300.0, 250.0);
        let (page, pp) = old.page_at(Point::new(scroll.x + anchor.x, scroll.y + anchor.y)).unwrap();
        let new = layout(&pages, DisplayMode::Continuous, 2.0, vp, 0);
        let s = anchored_scroll(&old, &new, scroll, anchor);
        let (page2, pp2) = new.page_at(Point::new(s.x + anchor.x, s.y + anchor.y)).unwrap();
        assert_eq!(page, page2);
        assert!((pp.x - pp2.x).abs() < 0.05 && (pp.y - pp2.y).abs() < 0.05);
    }

    #[test]
    fn zoom_steps_are_monotonic_and_clamped() {
        assert_eq!(zoom_in_step(1.0), 1.25);
        assert_eq!(zoom_out_step(1.0), 0.85);
        assert_eq!(zoom_in_step(MAX_ZOOM), MAX_ZOOM);
        assert_eq!(zoom_out_step(MIN_ZOOM), MIN_ZOOM);
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
        assert_eq!(clamp_zoom(1000.0), MAX_ZOOM);
    }
}
