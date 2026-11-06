//! Text layout and glyph outlines from the Windows system fonts, used to draw text in
//! saved annotation appearances (see ADR 0006). Fonts load lazily and are cached.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::annot::{Align, Seg};
use super::geometry::{Point, Rect};

/// Font families offered in the text style controls, with their files.
pub const FAMILIES: &[(&str, &str)] = &[
    ("Arial", "arial.ttf"),
    ("Times New Roman", "times.ttf"),
    ("Courier New", "cour.ttf"),
    ("Georgia", "georgia.ttf"),
    ("Verdana", "verdana.ttf"),
    ("Segoe UI", "segoeui.ttf"),
];

/// Tried in order for characters the chosen family lacks.
const FALLBACKS: &[(&str, u32)] = &[("segoeui.ttf", 0), ("seguisym.ttf", 0), ("msyh.ttc", 0), ("YuGothM.ttc", 0)];

struct FontFile {
    data: Vec<u8>,
    index: u32,
}

type Cache = Mutex<HashMap<String, Option<Arc<FontFile>>>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fonts_dir() -> std::path::PathBuf {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    std::path::PathBuf::from(windir).join("Fonts")
}

fn load(file: &str, index: u32) -> Option<Arc<FontFile>> {
    let key = format!("{file}#{index}");
    let mut c = cache().lock().ok()?;
    if let Some(v) = c.get(&key) {
        return v.clone();
    }
    let v = std::fs::read(fonts_dir().join(file))
        .ok()
        .filter(|d| ttf_parser::Face::parse(d, index).is_ok())
        .map(|data| Arc::new(FontFile { data, index }));
    c.insert(key, v.clone());
    v
}

fn family_file(family: &str) -> &'static str {
    FAMILIES.iter().find(|(f, _)| f.eq_ignore_ascii_case(family)).map(|(_, file)| *file).unwrap_or("arial.ttf")
}

/// A font family plus fallbacks, ready for measuring and outlining.
pub struct FontStack {
    files: Vec<Arc<FontFile>>,
}

impl FontStack {
    pub fn new(family: &str) -> Option<FontStack> {
        let mut files = vec![load(family_file(family), 0)?];
        for (f, i) in FALLBACKS {
            if let Some(ff) = load(f, *i) {
                files.push(ff);
            }
        }
        Some(FontStack { files })
    }

    fn face(&self, i: usize) -> ttf_parser::Face<'_> {
        let f = &self.files[i];
        ttf_parser::Face::parse(&f.data, f.index).expect("validated on load")
    }

    /// The first font in the stack that has a glyph for `c`.
    fn glyph(&self, c: char) -> Option<(usize, ttf_parser::GlyphId)> {
        (0..self.files.len()).find_map(|i| self.face(i).glyph_index(c).map(|g| (i, g)))
    }

    fn advance(&self, c: char, size: f32) -> f32 {
        match self.glyph(c) {
            Some((i, g)) => {
                let face = self.face(i);
                face.glyph_hor_advance(g).unwrap_or(0) as f32 * size / face.units_per_em() as f32
            }
            None => size * 0.5,
        }
    }

    pub fn measure(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.advance(c, size)).sum()
    }

    /// (ascent, line height) for `size`, from the primary font.
    pub fn metrics(&self, size: f32) -> (f32, f32) {
        let face = self.face(0);
        let k = size / face.units_per_em() as f32;
        let asc = face.ascender() as f32 * k;
        let desc = face.descender() as f32 * k;
        let gap = face.line_gap() as f32 * k;
        (asc, asc - desc + gap)
    }

    /// Breaks `text` into lines no wider than `width`, honouring explicit newlines.
    /// Words longer than a line are broken between characters.
    pub fn wrap(&self, text: &str, size: f32, width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let para = para.trim_end_matches('\r');
            let mut line = String::new();
            for word in para.split_inclusive(' ') {
                let candidate = format!("{line}{word}");
                if self.measure(candidate.trim_end(), size) <= width || line.is_empty() {
                    if line.is_empty() && self.measure(word.trim_end(), size) > width {
                        // Hard-break an over-long word.
                        for c in word.chars() {
                            if !line.is_empty() && self.measure(&format!("{line}{c}"), size) > width {
                                lines.push(std::mem::take(&mut line));
                            }
                            line.push(c);
                        }
                    } else {
                        line = candidate;
                    }
                } else {
                    lines.push(line.trim_end().to_string());
                    line = word.to_string();
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }

    /// Glyph outlines for wrapped text in `rect` (page display space, y down).
    pub fn outline(&self, lines: &[String], rect: Rect, size: f32, align: Align) -> Vec<Seg> {
        let (ascent, line_h) = self.metrics(size);
        let mut segs = Vec::new();
        for (li, line) in lines.iter().enumerate() {
            let w = self.measure(line, size);
            let x0 = match align {
                Align::Left => rect.x,
                Align::Center => rect.x + (rect.w - w) / 2.0,
                Align::Right => rect.right() - w,
            };
            let baseline = rect.y + ascent + li as f32 * line_h;
            let mut pen = x0;
            for c in line.chars() {
                if let Some((fi, g)) = self.glyph(c) {
                    let face = self.face(fi);
                    let k = size / face.units_per_em() as f32;
                    let mut b = Builder { segs: &mut segs, ox: pen, oy: baseline, k, last: Point::default() };
                    face.outline_glyph(g, &mut b);
                    pen += face.glyph_hor_advance(g).unwrap_or(0) as f32 * k;
                } else {
                    pen += size * 0.5;
                }
            }
        }
        segs
    }
}

struct Builder<'a> {
    segs: &'a mut Vec<Seg>,
    ox: f32,
    oy: f32,
    k: f32,
    last: Point,
}

impl Builder<'_> {
    fn p(&self, x: f32, y: f32) -> Point {
        // Font units are y-up; display space is y-down.
        Point::new(self.ox + x * self.k, self.oy - y * self.k)
    }
}

impl ttf_parser::OutlineBuilder for Builder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last = self.p(x, y);
        self.segs.push(Seg::Move(self.last));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.last = self.p(x, y);
        self.segs.push(Seg::Line(self.last));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        // Elevate the quadratic to a cubic.
        let q = self.p(x1, y1);
        let e = self.p(x, y);
        let s = self.last;
        let c1 = Point::new(s.x + 2.0 / 3.0 * (q.x - s.x), s.y + 2.0 / 3.0 * (q.y - s.y));
        let c2 = Point::new(e.x + 2.0 / 3.0 * (q.x - e.x), e.y + 2.0 / 3.0 * (q.y - e.y));
        self.segs.push(Seg::Cubic(c1, c2, e));
        self.last = e;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b, e) = (self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        self.segs.push(Seg::Cubic(a, b, e));
        self.last = e;
    }
    fn close(&mut self) {
        self.segs.push(Seg::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arial_measures_and_wraps() {
        let f = FontStack::new("Arial").expect("Arial ships with Windows");
        let w = f.measure("Hello", 12.0);
        assert!(w > 20.0 && w < 40.0, "{w}");
        let lines = f.wrap("The quick brown fox jumps over the lazy dog", 12.0, 80.0);
        assert!(lines.len() >= 3, "{lines:?}");
        assert!(lines.iter().all(|l| f.measure(l, 12.0) <= 80.0 + 0.01));
        assert_eq!(f.wrap("a\nb", 12.0, 500.0), vec!["a", "b"]);
        // An over-long word is broken between characters.
        let long = f.wrap("Supercalifragilisticexpialidocious", 12.0, 50.0);
        assert!(long.len() > 1 && long.iter().all(|l| f.measure(l, 12.0) <= 50.0 + 0.01));
    }

    #[test]
    fn outlines_cover_fallback_scripts() {
        let f = FontStack::new("Arial").unwrap();
        let rect = Rect::new(10.0, 10.0, 300.0, 100.0);
        let segs = f.outline(&["Hi 你好".into()], rect, 14.0, Align::Left);
        let moves = segs.iter().filter(|s| matches!(s, Seg::Move(_))).count();
        // H, i, and the two CJK glyphs (via a fallback font) all produce contours.
        assert!(moves >= 4, "{moves}");
        for s in &segs {
            if let Seg::Move(p) | Seg::Line(p) = s {
                assert!(p.x >= 9.0 && p.y >= 9.0 && p.y <= 40.0, "{p:?}");
            }
        }
    }

    #[test]
    fn unknown_family_falls_back_to_arial() {
        assert!(FontStack::new("No Such Font").is_some());
    }
}
