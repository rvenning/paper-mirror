//! The annotation model shared by the live overlay and the PDF writer.
//!
//! Geometry is in page display space (points, origin top-left of the displayed page).
//! Each annotation turns into drawing primitives (`prims`), which are emitted either
//! as SVG path commands for the Slint overlay or as a PDF content stream for the
//! saved appearance, so both always agree.

use serde::{Deserialize, Serialize};

use super::geometry::{Affine, Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const HIGHLIGHT_YELLOW: Color = Color::rgb(255, 214, 10);
    pub const NOTE_YELLOW: Color = Color::rgb(255, 225, 105);
    pub const RED: Color = Color::rgb(229, 72, 77);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    pub fn parse(s: &str) -> Option<Color> {
        let h = s.trim().trim_start_matches('#');
        let v = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            6 => Some(Color { r: v(0)?, g: v(2)?, b: v(4)?, a: 255 }),
            8 => Some(Color { r: v(0)?, g: v(2)?, b: v(4)?, a: v(6)? }),
            _ => None,
        }
    }

    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }

    /// PDF colour operands (0..1).
    pub fn pdf(&self) -> (f32, f32, f32) {
        (self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub stroke: Color,
    pub fill: Option<Color>,
    pub width: f32,
    pub dashed: bool,
    pub opacity: f32,
    pub text_color: Color,
    pub font: String,
    pub font_size: f32,
    pub align: Align,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            stroke: Color::RED,
            fill: None,
            width: 2.0,
            dashed: false,
            opacity: 1.0,
            text_color: Color::BLACK,
            font: "Arial".into(),
            font_size: 14.0,
            align: Align::Left,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Shape {
    Highlight {
        rects: Vec<Rect>,
    },
    Underline {
        rects: Vec<Rect>,
    },
    StrikeOut {
        rects: Vec<Rect>,
    },
    Ink {
        strokes: Vec<Vec<Point>>,
    },
    Line {
        from: Point,
        to: Point,
        arrow: bool,
    },
    Rectangle,
    RoundedRectangle,
    Oval,
    Star,
    SpeechBubble {
        tail: Point,
    },
    TextBox {
        text: String,
    },
    Note {
        text: String,
    },
    Signature {
        strokes: Vec<Vec<Point>>,
    },
    /// A magnified view of `source` (page area) drawn in the bounds.
    ZoomLens {
        source: Rect,
    },
    /// Area to be permanently redacted when the document is saved.
    Redaction,
}

impl Shape {
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Highlight { .. } => "Highlight",
            Shape::Underline { .. } => "Underline",
            Shape::StrikeOut { .. } => "Strikethrough",
            Shape::Ink { .. } => "Drawing",
            Shape::Line { arrow: true, .. } => "Arrow",
            Shape::Line { .. } => "Line",
            Shape::Rectangle => "Rectangle",
            Shape::RoundedRectangle => "Rounded rectangle",
            Shape::Oval => "Oval",
            Shape::Star => "Star",
            Shape::SpeechBubble { .. } => "Speech bubble",
            Shape::TextBox { .. } => "Text",
            Shape::Note { .. } => "Note",
            Shape::Signature { .. } => "Signature",
            Shape::ZoomLens { .. } => "Loupe",
            Shape::Redaction => "Redaction",
        }
    }

    /// Text markup follows the text and cannot be moved or resized.
    pub fn is_markup(&self) -> bool {
        matches!(self, Shape::Highlight { .. } | Shape::Underline { .. } | Shape::StrikeOut { .. })
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Shape::TextBox { text } | Shape::Note { text } => Some(text),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: u64,
    pub page: usize,
    /// Box for box-like shapes; derived from points for ink, lines and markup.
    pub bounds: Rect,
    pub shape: Shape,
    pub style: Style,
}

/// Note icons are a fixed size, like Preview's.
pub const NOTE_SIZE: f32 = 20.0;
const KAPPA: f32 = 0.552_284_8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(Point),
    Line(Point),
    Cubic(Point, Point, Point),
    Close,
}

/// A drawing primitive in page display space.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Stroke {
        segs: Vec<Seg>,
        color: Color,
        width: f32,
        dashed: bool,
        opacity: f32,
        round: bool,
    },
    Fill {
        segs: Vec<Seg>,
        color: Color,
        opacity: f32,
        multiply: bool,
    },
    /// Text drawn natively by the overlay and as glyph outlines in saved appearances.
    Text {
        rect: Rect,
        text: String,
        color: Color,
        font: String,
        size: f32,
        align: Align,
    },
    /// The zoom-lens image: `source` magnified into a circle inscribed in `rect`.
    Lens {
        rect: Rect,
        source: Rect,
    },
}

fn rect_segs(r: Rect) -> Vec<Seg> {
    vec![
        Seg::Move(Point::new(r.x, r.y)),
        Seg::Line(Point::new(r.right(), r.y)),
        Seg::Line(Point::new(r.right(), r.bottom())),
        Seg::Line(Point::new(r.x, r.bottom())),
        Seg::Close,
    ]
}

pub fn ellipse_segs(r: Rect) -> Vec<Seg> {
    let (cx, cy, rx, ry) = (r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0, r.h / 2.0);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    vec![
        Seg::Move(Point::new(cx + rx, cy)),
        Seg::Cubic(Point::new(cx + rx, cy + ky), Point::new(cx + kx, cy + ry), Point::new(cx, cy + ry)),
        Seg::Cubic(Point::new(cx - kx, cy + ry), Point::new(cx - rx, cy + ky), Point::new(cx - rx, cy)),
        Seg::Cubic(Point::new(cx - rx, cy - ky), Point::new(cx - kx, cy - ry), Point::new(cx, cy - ry)),
        Seg::Cubic(Point::new(cx + kx, cy - ry), Point::new(cx + rx, cy - ky), Point::new(cx + rx, cy)),
        Seg::Close,
    ]
}

fn rounded_rect_segs(r: Rect, radius: f32) -> Vec<Seg> {
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0);
    let k = rad * (1.0 - KAPPA);
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    vec![
        Seg::Move(Point::new(x0 + rad, y0)),
        Seg::Line(Point::new(x1 - rad, y0)),
        Seg::Cubic(Point::new(x1 - k, y0), Point::new(x1, y0 + k), Point::new(x1, y0 + rad)),
        Seg::Line(Point::new(x1, y1 - rad)),
        Seg::Cubic(Point::new(x1, y1 - k), Point::new(x1 - k, y1), Point::new(x1 - rad, y1)),
        Seg::Line(Point::new(x0 + rad, y1)),
        Seg::Cubic(Point::new(x0 + k, y1), Point::new(x0, y1 - k), Point::new(x0, y1 - rad)),
        Seg::Line(Point::new(x0, y0 + rad)),
        Seg::Cubic(Point::new(x0, y0 + k), Point::new(x0 + k, y0), Point::new(x0 + rad, y0)),
        Seg::Close,
    ]
}

fn star_segs(r: Rect) -> Vec<Seg> {
    let c = r.center();
    let mut segs = Vec::new();
    for i in 0..10 {
        let ang = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let k = if i % 2 == 0 { 1.0 } else { 0.4 };
        let p = Point::new(c.x + ang.cos() * r.w / 2.0 * k, c.y + ang.sin() * r.h / 2.0 * k);
        segs.push(if i == 0 { Seg::Move(p) } else { Seg::Line(p) });
    }
    segs.push(Seg::Close);
    segs
}

/// Rounded-rectangle outline with a tail reaching `tail`. The tail leaves the edge
/// facing the tail point, so the outline is a single closed path.
fn bubble_segs(r: Rect, tail: Point) -> Vec<Seg> {
    let rad = (r.w.min(r.h) * 0.2).min(14.0);
    if r.contains(tail) {
        return rounded_rect_segs(r, rad);
    }
    let c = r.center();
    // Normalised offsets decide which edge the tail grows from.
    let (dx, dy) = ((tail.x - c.x) / r.w.max(1.0), (tail.y - c.y) / r.h.max(1.0));
    let base = |len: f32| (len * 0.18).clamp(6.0, 30.0);
    let mut segs = rounded_rect_segs(r, rad);
    segs.pop(); // reopen to splice the tail in
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    if dy.abs() >= dx.abs() {
        let b = base(r.w);
        let mid = tail.x.clamp(x0 + rad + b, x1 - rad - b);
        let (pa, pb, edge_y) = if dy > 0.0 {
            (Point::new(mid + b / 2.0, y1), Point::new(mid - b / 2.0, y1), y1)
        } else {
            (Point::new(mid - b / 2.0, y0), Point::new(mid + b / 2.0, y0), y0)
        };
        let _ = edge_y;
        // Insert after the segment that runs along that edge.
        let idx = if dy > 0.0 { 5 } else { 1 };
        segs.splice(idx..idx, [Seg::Line(pa), Seg::Line(tail), Seg::Line(pb)]);
    } else {
        let b = base(r.h);
        let mid = tail.y.clamp(y0 + rad + b, y1 - rad - b);
        let (pa, pb) = if dx > 0.0 {
            (Point::new(x1, mid - b / 2.0), Point::new(x1, mid + b / 2.0))
        } else {
            (Point::new(x0, mid + b / 2.0), Point::new(x0, mid - b / 2.0))
        };
        let idx = if dx > 0.0 { 3 } else { 7 };
        segs.splice(idx..idx, [Seg::Line(pa), Seg::Line(tail), Seg::Line(pb)]);
    }
    segs.push(Seg::Close);
    segs
}

fn polyline(points: &[Point]) -> Vec<Seg> {
    let mut segs = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        segs.push(if i == 0 { Seg::Move(*p) } else { Seg::Line(*p) });
    }
    if points.len() == 1 {
        // A dot: a zero-length segment draws a round cap.
        segs.push(Seg::Line(Point::new(points[0].x + 0.01, points[0].y)));
    }
    segs
}

/// Arrowhead triangle at `to`, pointing away from `from`.
fn arrow_head(from: Point, to: Point, width: f32) -> Vec<Seg> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let size = (width * 4.0).max(9.0).min(len * 0.8);
    let back = Point::new(to.x - ux * size, to.y - uy * size);
    let (px, py) = (-uy * size * 0.45, ux * size * 0.45);
    vec![
        Seg::Move(to),
        Seg::Line(Point::new(back.x + px, back.y + py)),
        Seg::Line(Point::new(back.x - px, back.y - py)),
        Seg::Close,
    ]
}

fn note_icon(r: Rect) -> Vec<Prim> {
    let body = Prim::Fill { segs: rounded_rect_segs(r, 2.5), color: Color::NOTE_YELLOW, opacity: 1.0, multiply: false };
    let border = Prim::Stroke {
        segs: rounded_rect_segs(r, 2.5),
        color: Color::rgb(176, 138, 0),
        width: 0.8,
        dashed: false,
        opacity: 1.0,
        round: false,
    };
    let mut lines = Vec::new();
    for i in 0..3 {
        let y = r.y + r.h * (0.32 + i as f32 * 0.18);
        lines.push(Seg::Move(Point::new(r.x + r.w * 0.22, y)));
        lines.push(Seg::Line(Point::new(r.x + r.w * if i == 2 { 0.6 } else { 0.78 }, y)));
    }
    let text = Prim::Stroke {
        segs: lines,
        color: Color::rgb(120, 94, 0),
        width: 1.0,
        dashed: false,
        opacity: 1.0,
        round: true,
    };
    vec![body, border, text]
}

impl Annotation {
    pub fn new(id: u64, page: usize, bounds: Rect, shape: Shape, style: Style) -> Annotation {
        let mut a = Annotation { id, page, bounds, shape, style };
        a.recompute_bounds();
        a
    }

    /// For point-based shapes the bounds follow the geometry.
    pub fn recompute_bounds(&mut self) {
        let pts: Vec<Point> = match &self.shape {
            Shape::Ink { strokes } | Shape::Signature { strokes } => strokes.iter().flatten().copied().collect(),
            Shape::Line { from, to, .. } => vec![*from, *to],
            Shape::Highlight { rects } | Shape::Underline { rects } | Shape::StrikeOut { rects } => {
                if let Some(first) = rects.first() {
                    self.bounds = rects.iter().skip(1).fold(*first, |acc, r| acc.union(r));
                }
                return;
            }
            Shape::Note { .. } => {
                self.bounds = Rect::new(self.bounds.x, self.bounds.y, NOTE_SIZE, NOTE_SIZE);
                return;
            }
            _ => return,
        };
        if let Some(first) = pts.first() {
            let mut r = Rect::new(first.x, first.y, 0.0, 0.0);
            for p in &pts[1..] {
                r = r.union(&Rect::new(p.x, p.y, 0.0, 0.0));
            }
            self.bounds = r;
        }
    }

    /// Area the annotation visibly occupies (bounds plus half the stroke and the
    /// arrowhead), used for hit-testing, selection handles and appearance boxes.
    pub fn visual_bounds(&self) -> Rect {
        let pad = match &self.shape {
            Shape::Line { arrow: true, .. } => (self.style.width * 4.0).max(9.0),
            s if s.is_markup() => 0.0,
            _ => self.style.width / 2.0 + 0.5,
        };
        let mut r = self.bounds.inflate(pad);
        if let Shape::SpeechBubble { tail } = &self.shape {
            r = r.union(&Rect::new(tail.x, tail.y, 0.0, 0.0)).inflate(pad);
        }
        r
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut Point| {
            p.x += dx;
            p.y += dy;
        };
        match &mut self.shape {
            Shape::Ink { strokes } | Shape::Signature { strokes } => strokes.iter_mut().flatten().for_each(mv),
            Shape::Line { from, to, .. } => {
                mv(from);
                mv(to);
            }
            Shape::SpeechBubble { tail } => mv(tail),
            Shape::Highlight { rects } | Shape::Underline { rects } | Shape::StrikeOut { rects } => {
                rects.iter_mut().for_each(|r| *r = r.translate(dx, dy))
            }
            _ => {}
        }
        self.bounds = self.bounds.translate(dx, dy);
    }

    /// Scales the geometry from the current bounds into `to`.
    pub fn resize_to(&mut self, to: Rect) {
        if self.shape.is_markup() || matches!(self.shape, Shape::Note { .. }) {
            return;
        }
        let from = self.bounds;
        let to = Rect::new(to.x, to.y, to.w.max(2.0), to.h.max(2.0));
        let sx = if from.w > 0.01 { to.w / from.w } else { 1.0 };
        let sy = if from.h > 0.01 { to.h / from.h } else { 1.0 };
        let map = |p: &mut Point| {
            p.x = to.x + (p.x - from.x) * sx;
            p.y = to.y + (p.y - from.y) * sy;
        };
        match &mut self.shape {
            Shape::Ink { strokes } | Shape::Signature { strokes } => strokes.iter_mut().flatten().for_each(map),
            Shape::Line { from: a, to: b, .. } => {
                map(a);
                map(b);
            }
            Shape::SpeechBubble { tail } => map(tail),
            _ => {}
        }
        self.bounds = to;
        self.recompute_bounds();
    }

    /// The same annotation with every coordinate mapped through `t` (used to store
    /// geometry in PDF user space and bring it back to display space). Box shapes keep
    /// an axis-aligned box, which holds for the quarter-turn page rotations PDFs use.
    pub fn transformed(&self, t: &Affine) -> Annotation {
        let mut a = self.clone();
        let map = |p: &mut Point| *p = t.apply(*p);
        match &mut a.shape {
            Shape::Ink { strokes } | Shape::Signature { strokes } => strokes.iter_mut().flatten().for_each(map),
            Shape::Line { from, to, .. } => {
                map(from);
                map(to);
            }
            Shape::SpeechBubble { tail } => map(tail),
            Shape::ZoomLens { source } => *source = t.apply_rect(*source),
            Shape::Highlight { rects } | Shape::Underline { rects } | Shape::StrikeOut { rects } => {
                rects.iter_mut().for_each(|r| *r = t.apply_rect(*r))
            }
            _ => {}
        }
        a.bounds = t.apply_rect(self.bounds);
        if let Shape::Note { .. } = a.shape {
            // Keep the icon's top-left corner where it was.
            let corner = t.apply(Point::new(self.bounds.x, self.bounds.y));
            let other = t.apply(Point::new(self.bounds.x + NOTE_SIZE, self.bounds.y + NOTE_SIZE));
            a.bounds = Rect::new(corner.x.min(other.x), corner.y.min(other.y), NOTE_SIZE, NOTE_SIZE);
        }
        a.style.width *= t.scale_factor();
        a.style.font_size *= t.scale_factor();
        a
    }

    /// Whether a click at `p` (page space) selects this annotation.
    pub fn hit(&self, p: Point, tolerance: f32) -> bool {
        match &self.shape {
            Shape::Ink { strokes } | Shape::Signature { strokes } => {
                let tol = tolerance + self.style.width / 2.0;
                strokes.iter().any(|s| {
                    s.windows(2).any(|w| dist_to_segment(p, w[0], w[1]) <= tol)
                        || (s.len() == 1 && s[0].distance(p) <= tol)
                })
            }
            Shape::Line { from, to, .. } => dist_to_segment(p, *from, *to) <= tolerance + self.style.width / 2.0,
            Shape::Highlight { rects } | Shape::Underline { rects } | Shape::StrikeOut { rects } => {
                rects.iter().any(|r| r.inflate(tolerance / 2.0).contains(p))
            }
            _ => self.visual_bounds().inflate(tolerance).contains(p),
        }
    }

    /// Drawing primitives in page display space.
    pub fn prims(&self) -> Vec<Prim> {
        let s = &self.style;
        let stroke = |segs: Vec<Seg>| Prim::Stroke {
            segs,
            color: s.stroke,
            width: s.width,
            dashed: s.dashed,
            opacity: s.opacity,
            round: true,
        };
        let filled = |segs: &Vec<Seg>| {
            s.fill.map(|c| Prim::Fill { segs: segs.clone(), color: c, opacity: s.opacity, multiply: false })
        };
        let b = self.bounds;
        match &self.shape {
            Shape::Highlight { rects } => rects
                .iter()
                .map(|r| Prim::Fill { segs: rect_segs(*r), color: s.stroke, opacity: s.opacity, multiply: true })
                .collect(),
            Shape::Underline { rects } => rects
                .iter()
                .map(|r| {
                    let y = r.bottom() - (r.h * 0.08).max(0.6);
                    Prim::Stroke {
                        segs: vec![Seg::Move(Point::new(r.x, y)), Seg::Line(Point::new(r.right(), y))],
                        color: s.stroke,
                        width: (r.h * 0.07).clamp(0.75, 2.5),
                        dashed: false,
                        opacity: s.opacity,
                        round: false,
                    }
                })
                .collect(),
            Shape::StrikeOut { rects } => rects
                .iter()
                .map(|r| {
                    let y = r.y + r.h * 0.55;
                    Prim::Stroke {
                        segs: vec![Seg::Move(Point::new(r.x, y)), Seg::Line(Point::new(r.right(), y))],
                        color: s.stroke,
                        width: (r.h * 0.07).clamp(0.75, 2.5),
                        dashed: false,
                        opacity: s.opacity,
                        round: false,
                    }
                })
                .collect(),
            Shape::Ink { strokes } | Shape::Signature { strokes } => {
                strokes.iter().filter(|st| !st.is_empty()).map(|st| stroke(polyline(st))).collect()
            }
            Shape::Line { from, to, arrow } => {
                let mut v = vec![];
                if *arrow {
                    let head = arrow_head(*from, *to, s.width);
                    // Stop the shaft inside the head so the tip stays sharp.
                    let (dx, dy) = (to.x - from.x, to.y - from.y);
                    let len = (dx * dx + dy * dy).sqrt().max(0.001);
                    let size = (s.width * 4.0).max(9.0).min(len * 0.8);
                    let end = Point::new(to.x - dx / len * size * 0.7, to.y - dy / len * size * 0.7);
                    v.push(stroke(vec![Seg::Move(*from), Seg::Line(end)]));
                    v.push(Prim::Fill { segs: head, color: s.stroke, opacity: s.opacity, multiply: false });
                } else {
                    v.push(stroke(vec![Seg::Move(*from), Seg::Line(*to)]));
                }
                v
            }
            Shape::Rectangle | Shape::RoundedRectangle | Shape::Oval | Shape::Star | Shape::SpeechBubble { .. } => {
                let segs = match &self.shape {
                    Shape::Rectangle => rect_segs(b),
                    Shape::RoundedRectangle => rounded_rect_segs(b, (b.w.min(b.h) * 0.15).min(16.0)),
                    Shape::Oval => ellipse_segs(b),
                    Shape::Star => star_segs(b),
                    Shape::SpeechBubble { tail } => bubble_segs(b, *tail),
                    _ => unreachable!(),
                };
                let mut v: Vec<Prim> = filled(&segs).into_iter().collect();
                if s.width > 0.0 {
                    v.push(Prim::Stroke {
                        segs,
                        color: s.stroke,
                        width: s.width,
                        dashed: s.dashed,
                        opacity: s.opacity,
                        round: false,
                    });
                }
                v
            }
            Shape::TextBox { text } => {
                let mut v: Vec<Prim> = filled(&rect_segs(b)).into_iter().collect();
                if s.width > 0.0 && s.stroke.a > 0 {
                    v.push(Prim::Stroke {
                        segs: rect_segs(b),
                        color: s.stroke,
                        width: s.width,
                        dashed: s.dashed,
                        opacity: s.opacity,
                        round: false,
                    });
                }
                v.push(Prim::Text {
                    rect: b.inflate(-(s.width.max(1.0) + 2.0)),
                    text: text.clone(),
                    color: s.text_color,
                    font: s.font.clone(),
                    size: s.font_size,
                    align: s.align,
                });
                v
            }
            Shape::Note { .. } => note_icon(Rect::new(b.x, b.y, NOTE_SIZE, NOTE_SIZE)),
            Shape::ZoomLens { source } => vec![
                Prim::Lens { rect: b, source: *source },
                Prim::Stroke {
                    segs: ellipse_segs(b),
                    color: s.stroke,
                    width: s.width.max(1.0),
                    dashed: false,
                    opacity: 1.0,
                    round: false,
                },
            ],
            Shape::Redaction => {
                let mut v =
                    vec![Prim::Fill { segs: rect_segs(b), color: Color::BLACK, opacity: 0.75, multiply: false }];
                // Hatching marks it as pending, as Preview does.
                let mut hatch = Vec::new();
                let step = 8.0;
                let mut t = step;
                while t < b.w + b.h {
                    let a = Point::new(b.x + (t - b.h).max(0.0), b.y + t.min(b.h));
                    let c = Point::new(b.x + t.min(b.w), b.y + (t - b.w).max(0.0));
                    hatch.push(Seg::Move(a));
                    hatch.push(Seg::Line(c));
                    t += step;
                }
                v.push(Prim::Stroke {
                    segs: hatch,
                    color: Color::rgb(90, 90, 90),
                    width: 1.0,
                    dashed: false,
                    opacity: 1.0,
                    round: false,
                });
                v
            }
        }
    }
}

fn dist_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-6 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

/// SVG path commands for `segs` under `t` (for the Slint overlay).
pub fn svg_path(segs: &[Seg], t: &Affine) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for seg in segs {
        match seg {
            Seg::Move(p) => {
                let p = t.apply(*p);
                let _ = write!(s, "M {:.2} {:.2} ", p.x, p.y);
            }
            Seg::Line(p) => {
                let p = t.apply(*p);
                let _ = write!(s, "L {:.2} {:.2} ", p.x, p.y);
            }
            Seg::Cubic(a, b, c) => {
                let (a, b, c) = (t.apply(*a), t.apply(*b), t.apply(*c));
                let _ = write!(s, "C {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} ", a.x, a.y, b.x, b.y, c.x, c.y);
            }
            Seg::Close => s.push_str("Z "),
        }
    }
    s
}

/// PDF path construction operators for `segs` under `t`.
pub fn pdf_path(segs: &[Seg], t: &Affine, out: &mut String) {
    use std::fmt::Write;
    for seg in segs {
        match seg {
            Seg::Move(p) => {
                let p = t.apply(*p);
                let _ = writeln!(out, "{} {} m", n(p.x), n(p.y));
            }
            Seg::Line(p) => {
                let p = t.apply(*p);
                let _ = writeln!(out, "{} {} l", n(p.x), n(p.y));
            }
            Seg::Cubic(a, b, c) => {
                let (a, b, c) = (t.apply(*a), t.apply(*b), t.apply(*c));
                let _ = writeln!(out, "{} {} {} {} {} {} c", n(a.x), n(a.y), n(b.x), n(b.y), n(c.x), n(c.y));
            }
            Seg::Close => out.push_str("h\n"),
        }
    }
}

/// Formats a number compactly for content streams.
pub fn n(v: f32) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

// ----- Sketch recognition -------------------------------------------------------------

/// Preview's Sketch tool turns a roughly drawn shape into a clean one. Recognises a
/// straight line, a rectangle or an ellipse; anything else stays freehand.
pub fn recognise_sketch(stroke: &[Point]) -> Option<(Shape, Rect)> {
    if stroke.len() < 4 {
        return None;
    }
    let first = stroke[0];
    let last = *stroke.last().unwrap();
    let path_len: f32 = stroke.windows(2).map(|w| w[0].distance(w[1])).sum();
    if path_len < 12.0 {
        return None;
    }
    let mut bounds = Rect::new(first.x, first.y, 0.0, 0.0);
    for p in stroke {
        bounds = bounds.union(&Rect::new(p.x, p.y, 0.0, 0.0));
    }
    // Straight line: the end-to-end distance is nearly the whole path length.
    let chord = first.distance(last);
    if chord / path_len > 0.95 {
        let max_dev = stroke.iter().map(|p| dist_to_segment(*p, first, last)).fold(0.0, f32::max);
        if max_dev < (chord * 0.06).max(3.0) {
            return Some((Shape::Line { from: first, to: last, arrow: false }, bounds));
        }
    }
    // Closed shapes: start and end meet.
    let diag = (bounds.w * bounds.w + bounds.h * bounds.h).sqrt();
    if chord > diag * 0.25 || bounds.w < 10.0 || bounds.h < 10.0 {
        return None;
    }
    // Mean relative distance to the ideal ellipse and rectangle outlines.
    let c = bounds.center();
    let (rx, ry) = (bounds.w / 2.0, bounds.h / 2.0);
    let ellipse_err = stroke
        .iter()
        .map(|p| {
            let (u, v) = ((p.x - c.x) / rx, (p.y - c.y) / ry);
            ((u * u + v * v).sqrt() - 1.0).abs()
        })
        .sum::<f32>()
        / stroke.len() as f32;
    let rect_err = stroke
        .iter()
        .map(|p| {
            let d = [
                (p.x - bounds.x).abs() / rx,
                (bounds.right() - p.x).abs() / rx,
                (p.y - bounds.y).abs() / ry,
                (bounds.bottom() - p.y).abs() / ry,
            ];
            d.into_iter().fold(f32::MAX, f32::min)
        })
        .sum::<f32>()
        / stroke.len() as f32;
    if rect_err < 0.08 && rect_err < ellipse_err {
        Some((Shape::Rectangle, bounds))
    } else if ellipse_err < 0.12 {
        Some((Shape::Oval, bounds))
    } else {
        None
    }
}

/// Lightly smooths a freehand stroke (moving average) and drops near-duplicates.
pub fn smooth_stroke(points: &[Point]) -> Vec<Point> {
    let mut dedup: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if dedup.last().is_none_or(|q| q.distance(*p) >= 0.6) {
            dedup.push(*p);
        }
    }
    if dedup.len() < 3 {
        return dedup;
    }
    let mut out = Vec::with_capacity(dedup.len());
    out.push(dedup[0]);
    for w in dedup.windows(3) {
        out.push(Point::new((w[0].x + 2.0 * w[1].x + w[2].x) / 4.0, (w[0].y + 2.0 * w[1].y + w[2].y) / 4.0));
    }
    out.push(*dedup.last().unwrap());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> Style {
        Style::default()
    }

    #[test]
    fn colors_round_trip() {
        let c = Color::parse("#e5484d").unwrap();
        assert_eq!(c, Color::RED);
        assert_eq!(Color::parse(&c.hex()), Some(c));
        assert_eq!(Color::parse("#11223380").unwrap().a, 0x80);
        assert!(Color::parse("nope").is_none());
    }

    #[test]
    fn json_round_trip() {
        let a = Annotation::new(
            7,
            2,
            Rect::new(10.0, 20.0, 100.0, 40.0),
            Shape::TextBox { text: "Grüße — 你好".into() },
            Style { font_size: 18.0, ..style() },
        );
        let j = serde_json::to_string(&a).unwrap();
        assert!(j.contains("\"type\":\"text-box\""));
        let back: Annotation = serde_json::from_str(&j).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn ink_bounds_move_resize_and_hit() {
        let mut a = Annotation::new(
            1,
            0,
            Rect::default(),
            Shape::Ink { strokes: vec![vec![Point::new(10.0, 10.0), Point::new(50.0, 30.0)]] },
            style(),
        );
        assert_eq!(a.bounds, Rect::new(10.0, 10.0, 40.0, 20.0));
        assert!(a.hit(Point::new(30.0, 20.0), 2.0));
        assert!(!a.hit(Point::new(30.0, 29.0), 2.0));
        a.translate(5.0, 5.0);
        assert_eq!(a.bounds, Rect::new(15.0, 15.0, 40.0, 20.0));
        a.resize_to(Rect::new(0.0, 0.0, 80.0, 40.0));
        assert_eq!(a.bounds, Rect::new(0.0, 0.0, 80.0, 40.0));
        if let Shape::Ink { strokes } = &a.shape {
            assert_eq!(strokes[0][1], Point::new(80.0, 40.0));
        }
    }

    #[test]
    fn markup_cannot_be_resized_and_note_is_fixed_size() {
        let mut h = Annotation::new(
            1,
            0,
            Rect::default(),
            Shape::Highlight { rects: vec![Rect::new(0.0, 0.0, 50.0, 10.0)] },
            style(),
        );
        h.resize_to(Rect::new(0.0, 0.0, 500.0, 500.0));
        assert_eq!(h.bounds, Rect::new(0.0, 0.0, 50.0, 10.0));
        let n = Annotation::new(2, 0, Rect::new(5.0, 5.0, 100.0, 100.0), Shape::Note { text: String::new() }, style());
        assert_eq!(n.bounds, Rect::new(5.0, 5.0, NOTE_SIZE, NOTE_SIZE));
    }

    #[test]
    fn primitives_for_every_shape() {
        let b = Rect::new(10.0, 10.0, 100.0, 60.0);
        let shapes = vec![
            Shape::Highlight { rects: vec![b] },
            Shape::Underline { rects: vec![b] },
            Shape::StrikeOut { rects: vec![b] },
            Shape::Ink { strokes: vec![vec![Point::new(1.0, 1.0)]] },
            Shape::Line { from: Point::new(0.0, 0.0), to: Point::new(50.0, 50.0), arrow: true },
            Shape::Rectangle,
            Shape::RoundedRectangle,
            Shape::Oval,
            Shape::Star,
            Shape::SpeechBubble { tail: Point::new(30.0, 120.0) },
            Shape::SpeechBubble { tail: Point::new(200.0, 30.0) },
            Shape::TextBox { text: "Hi".into() },
            Shape::Note { text: "n".into() },
            Shape::Signature { strokes: vec![vec![Point::new(1.0, 1.0), Point::new(2.0, 3.0)]] },
            Shape::ZoomLens { source: Rect::new(0.0, 0.0, 20.0, 20.0) },
            Shape::Redaction,
        ];
        for s in shapes {
            let a = Annotation::new(1, 0, b, s.clone(), Style { fill: Some(Color::WHITE), ..style() });
            let prims = a.prims();
            assert!(!prims.is_empty(), "{s:?}");
            for p in &prims {
                if let Prim::Stroke { segs, .. } | Prim::Fill { segs, .. } = p {
                    assert!(matches!(segs.first(), Some(Seg::Move(_))), "{s:?} path must start with a move");
                    let svg = svg_path(segs, &Affine::IDENTITY);
                    assert!(svg.starts_with("M "));
                    let mut pdf = String::new();
                    pdf_path(segs, &Affine::IDENTITY, &mut pdf);
                    assert!(pdf.contains(" m\n"));
                }
            }
        }
    }

    #[test]
    fn arrow_has_a_filled_head() {
        let a = Annotation::new(
            1,
            0,
            Rect::default(),
            Shape::Line { from: Point::new(0.0, 0.0), to: Point::new(100.0, 0.0), arrow: true },
            style(),
        );
        assert!(a.prims().iter().any(|p| matches!(p, Prim::Fill { .. })));
        assert!(a.visual_bounds().h >= 18.0, "head widens the visual bounds");
    }

    #[test]
    fn number_formatting() {
        assert_eq!(n(1.0), "1");
        assert_eq!(n(1.25), "1.25");
        assert_eq!(n(-0.0001), "0");
        assert_eq!(n(612.3333), "612.333");
    }

    #[test]
    fn sketch_recognition() {
        // Nearly straight stroke -> line.
        let line: Vec<Point> = (0..20).map(|i| Point::new(i as f32 * 5.0, 50.0 + (i % 2) as f32)).collect();
        assert!(matches!(recognise_sketch(&line), Some((Shape::Line { .. }, _))));
        // Circle -> oval.
        let circle: Vec<Point> = (0..=40)
            .map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::TAU;
                Point::new(100.0 + 40.0 * a.cos(), 100.0 + 30.0 * a.sin())
            })
            .collect();
        assert!(matches!(recognise_sketch(&circle), Some((Shape::Oval, _))));
        // Box -> rectangle.
        let mut rect = Vec::new();
        for i in 0..10 {
            rect.push(Point::new(i as f32 * 10.0, 0.0));
        }
        for i in 0..10 {
            rect.push(Point::new(100.0, i as f32 * 6.0));
        }
        for i in 0..10 {
            rect.push(Point::new(100.0 - i as f32 * 10.0, 60.0));
        }
        for i in 0..10 {
            rect.push(Point::new(0.0, 60.0 - i as f32 * 6.0));
        }
        assert!(matches!(recognise_sketch(&rect), Some((Shape::Rectangle, _))));
        // A zig-zag stays freehand.
        let zig: Vec<Point> =
            (0..20).map(|i| Point::new(i as f32 * 5.0, if i % 2 == 0 { 0.0 } else { 40.0 })).collect();
        assert!(recognise_sketch(&zig).is_none());
    }

    #[test]
    fn smoothing_keeps_endpoints() {
        let pts = vec![Point::new(0.0, 0.0), Point::new(10.0, 10.0), Point::new(20.0, 0.0), Point::new(30.0, 10.0)];
        let s = smooth_stroke(&pts);
        assert_eq!(s.first(), pts.first());
        assert_eq!(s.last(), pts.last());
        assert_eq!(s.len(), pts.len());
    }
}
