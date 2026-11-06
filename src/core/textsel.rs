//! Text selection model: hit-testing points against character boxes, caret ranges,
//! word expansion and merged highlight rectangles. Pure logic, so it is unit-tested
//! without PDFium; geometry comes from `pdf::text::page_geometry`.

use super::geometry::{Point, Rect};

/// Characters of one page in PDFium's text order. `units` is UTF-16 and index-aligned
/// with `boxes` (PDFium indexes UTF-16 code units). Generated characters (spaces and
/// line breaks PDFium infers) have empty boxes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub units: Vec<u16>,
    pub boxes: Vec<Rect>,
}

/// A caret position: a page and a character offset (0..=len).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Caret {
    pub page: usize,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    pub anchor: Caret,
    pub focus: Caret,
}

impl TextSelection {
    pub fn ordered(&self) -> (Caret, Caret) {
        if self.anchor <= self.focus { (self.anchor, self.focus) } else { (self.focus, self.anchor) }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }

    /// Character range selected on `page`, given that page's character count.
    pub fn range_on(&self, page: usize, len: usize) -> Option<(usize, usize)> {
        let (a, b) = self.ordered();
        if page < a.page || page > b.page {
            return None;
        }
        let start = if page == a.page { a.index.min(len) } else { 0 };
        let end = if page == b.page { b.index.min(len) } else { len };
        (end > start).then_some((start, end))
    }
}

impl PageText {
    pub fn len(&self) -> usize {
        self.units.len()
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    fn has_box(&self, i: usize) -> bool {
        !self.boxes[i].is_empty()
    }

    /// Whether `p` lies on (or within `slop` points of) any character.
    pub fn is_over_text(&self, p: Point, slop: f32) -> bool {
        self.boxes.iter().any(|b| !b.is_empty() && b.inflate(slop).contains(p))
    }

    /// Caret position nearest to `p`: before or after the closest character
    /// depending on which half of it the point falls in. Lines are found by vertical
    /// overlap first so that points in the margin select to the line end.
    pub fn caret_at(&self, p: Point) -> usize {
        let mut best: Option<(usize, f32)> = None;
        for i in 0..self.len() {
            if !self.has_box(i) {
                continue;
            }
            let b = self.boxes[i];
            let dy = if p.y < b.y {
                b.y - p.y
            } else if p.y > b.bottom() {
                p.y - b.bottom()
            } else {
                0.0
            };
            let dx = if p.x < b.x {
                b.x - p.x
            } else if p.x > b.right() {
                p.x - b.right()
            } else {
                0.0
            };
            // Being on the right line matters far more than horizontal distance.
            let score = dy * 1000.0 + dx;
            if best.is_none_or(|(_, s)| score < s) {
                best = Some((i, score));
            }
        }
        match best {
            None => 0,
            Some((i, _)) => {
                if p.x > self.boxes[i].center().x {
                    i + 1
                } else {
                    i
                }
            }
        }
    }

    /// Expands a caret to the word around it (letters, digits and joiners).
    pub fn word_at(&self, index: usize) -> (usize, usize) {
        let is_word = |i: usize| {
            self.units
                .get(i)
                .and_then(|u| char::from_u32(*u as u32))
                .is_some_and(|c| c.is_alphanumeric() || c == '\'' || c == '’' || c == '-' || c == '_')
        };
        let mut i = index.min(self.len().saturating_sub(1));
        if !is_word(i) && i > 0 && is_word(i - 1) {
            i -= 1;
        }
        if !is_word(i) {
            return (i, (i + 1).min(self.len()));
        }
        let mut a = i;
        while a > 0 && is_word(a - 1) {
            a -= 1;
        }
        let mut b = i + 1;
        while b < self.len() && is_word(b) {
            b += 1;
        }
        (a, b)
    }

    /// Highlight rectangles for `[start, end)`, merged per line.
    pub fn range_rects(&self, start: usize, end: usize) -> Vec<Rect> {
        let mut out: Vec<Rect> = Vec::new();
        for i in start..end.min(self.len()) {
            if !self.has_box(i) {
                continue;
            }
            let b = self.boxes[i];
            if let Some(last) = out.last_mut() {
                let overlap = (last.bottom().min(b.bottom()) - last.y.max(b.y)).max(0.0);
                let same_line = overlap >= 0.5 * b.h.min(last.h);
                if same_line && b.x >= last.x - 1.0 {
                    *last = last.union(&b);
                    continue;
                }
            }
            out.push(b);
        }
        out
    }

    /// Text of `[start, end)` with PDFium's CRLF line breaks normalised to `\r\n`
    /// (the Windows clipboard convention) and stray NULs removed.
    pub fn text(&self, start: usize, end: usize) -> String {
        let end = end.min(self.len());
        if start >= end {
            return String::new();
        }
        let s = String::from_utf16_lossy(&self.units[start..end]);
        let s = s.replace('\0', "");
        // PDFium emits "\r\n" for line breaks; make lone "\n" or "\r" consistent too.
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push_str("\r\n");
                }
                '\n' => out.push_str("\r\n"),
                c => out.push(c),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two lines: "ab cd" at y 10..20 and "ef" at y 30..40; char width 10.
    fn sample() -> PageText {
        let mut units: Vec<u16> = "ab cd\r\nef".encode_utf16().collect();
        let mut boxes = Vec::new();
        for (i, _) in "ab cd".chars().enumerate() {
            boxes.push(Rect::new(10.0 + i as f32 * 10.0, 10.0, 10.0, 10.0));
        }
        boxes.push(Rect::default()); // \r
        boxes.push(Rect::default()); // \n
        boxes.push(Rect::new(10.0, 30.0, 10.0, 10.0));
        boxes.push(Rect::new(20.0, 30.0, 10.0, 10.0));
        units.truncate(boxes.len());
        PageText { units, boxes }
    }

    #[test]
    fn caret_hit_testing() {
        let t = sample();
        assert_eq!(t.caret_at(Point::new(11.0, 15.0)), 0); // left half of 'a'
        assert_eq!(t.caret_at(Point::new(19.0, 15.0)), 1); // right half of 'a'
        assert_eq!(t.caret_at(Point::new(500.0, 15.0)), 5); // far right of line 1: end of "cd"
        assert_eq!(t.caret_at(Point::new(0.0, 35.0)), 7); // left margin of line 2
        assert_eq!(t.caret_at(Point::new(29.0, 45.0)), 9); // below the last line, near "f"
    }

    #[test]
    fn words_and_text() {
        let t = sample();
        assert_eq!(t.word_at(1), (0, 2));
        assert_eq!(t.word_at(3), (3, 5));
        assert_eq!(t.text(0, 9), "ab cd\r\nef");
        assert_eq!(t.text(3, 5), "cd");
        assert_eq!(t.text(5, 5), "");
    }

    #[test]
    fn rects_merge_per_line() {
        let t = sample();
        let r = t.range_rects(0, 9);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], Rect::new(10.0, 10.0, 50.0, 10.0));
        assert_eq!(r[1], Rect::new(10.0, 30.0, 20.0, 10.0));
        assert!(t.is_over_text(Point::new(15.0, 15.0), 0.0));
        assert!(!t.is_over_text(Point::new(15.0, 25.0), 0.0));
    }

    #[test]
    fn selection_ranges_across_pages() {
        let s = TextSelection { anchor: Caret { page: 3, index: 10 }, focus: Caret { page: 1, index: 4 } };
        assert_eq!(s.ordered().0, Caret { page: 1, index: 4 });
        assert_eq!(s.range_on(0, 50), None);
        assert_eq!(s.range_on(1, 50), Some((4, 50)));
        assert_eq!(s.range_on(2, 50), Some((0, 50)));
        assert_eq!(s.range_on(3, 50), Some((0, 10)));
        assert_eq!(s.range_on(4, 50), None);
        let empty = TextSelection { anchor: Caret { page: 0, index: 2 }, focus: Caret { page: 0, index: 2 } };
        assert!(empty.is_empty() && empty.range_on(0, 9).is_none());
    }
}
