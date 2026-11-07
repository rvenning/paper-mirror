//! The Adjust Size dialog's arithmetic: width, height and resolution in several
//! units, "Scale proportionally" and "Resample image", as in Preview.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Pixels,
    Percent,
    Inches,
    Centimetres,
    Millimetres,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::Pixels, Unit::Percent, Unit::Inches, Unit::Centimetres, Unit::Millimetres];

    pub fn name(self) -> &'static str {
        match self {
            Unit::Pixels => "pixels",
            Unit::Percent => "percent",
            Unit::Inches => "inches",
            Unit::Centimetres => "cm",
            Unit::Millimetres => "mm",
        }
    }

    fn physical(self) -> bool {
        matches!(self, Unit::Inches | Unit::Centimetres | Unit::Millimetres)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeModel {
    /// The image as it is now.
    pub base_w: u32,
    pub base_h: u32,
    pub base_dpi: f32,
    /// The result being edited.
    pub w: u32,
    pub h: u32,
    pub dpi: f32,
    pub unit: Unit,
    pub proportional: bool,
    pub resample: bool,
}

pub const MAX_SIDE: u32 = 30_000;

impl SizeModel {
    pub fn new(w: u32, h: u32, dpi: f32) -> SizeModel {
        let dpi = if dpi > 0.0 { dpi } else { 72.0 };
        SizeModel {
            base_w: w,
            base_h: h,
            base_dpi: dpi,
            w,
            h,
            dpi,
            unit: Unit::Pixels,
            proportional: true,
            resample: true,
        }
    }

    fn px_to_unit(&self, px: u32, base: u32) -> f64 {
        let inches = px as f64 / self.dpi as f64;
        match self.unit {
            Unit::Pixels => px as f64,
            Unit::Percent => px as f64 / base as f64 * 100.0,
            Unit::Inches => inches,
            Unit::Centimetres => inches * 2.54,
            Unit::Millimetres => inches * 25.4,
        }
    }

    fn inches(&self, v: f64) -> f64 {
        match self.unit {
            Unit::Centimetres => v / 2.54,
            Unit::Millimetres => v / 25.4,
            _ => v,
        }
    }

    fn unit_to_px(&self, v: f64, base: u32) -> u32 {
        let px = match self.unit {
            Unit::Pixels => v,
            Unit::Percent => base as f64 * v / 100.0,
            _ => self.inches(v) * self.dpi as f64,
        };
        (px.round().max(1.0) as u32).min(MAX_SIDE)
    }

    pub fn width_text(&self) -> String {
        fmt(self.px_to_unit(self.w, self.base_w), self.unit)
    }

    pub fn height_text(&self) -> String {
        fmt(self.px_to_unit(self.h, self.base_h), self.unit)
    }

    pub fn resolution_text(&self) -> String {
        fmt(self.dpi as f64, Unit::Inches)
    }

    /// Whether width and height can be typed (without resampling, pixel counts are
    /// fixed, so only physical units make sense).
    pub fn size_editable(&self) -> bool {
        self.resample || self.unit.physical()
    }

    pub fn set_width(&mut self, text: &str) {
        let Some(v) = parse(text) else { return };
        if self.resample {
            self.w = self.unit_to_px(v, self.base_w);
            if self.proportional {
                self.h =
                    ((self.w as f64 * self.base_h as f64 / self.base_w as f64).round().max(1.0) as u32).min(MAX_SIDE);
            }
        } else if self.unit.physical() {
            let inches = self.inches(v);
            if inches > 0.0 {
                self.dpi = (self.w as f64 / inches) as f32;
            }
        }
    }

    pub fn set_height(&mut self, text: &str) {
        let Some(v) = parse(text) else { return };
        if self.resample {
            self.h = self.unit_to_px(v, self.base_h);
            if self.proportional {
                self.w =
                    ((self.h as f64 * self.base_w as f64 / self.base_h as f64).round().max(1.0) as u32).min(MAX_SIDE);
            }
        } else if self.unit.physical() {
            let inches = self.inches(v);
            if inches > 0.0 {
                self.dpi = (self.h as f64 / inches) as f32;
            }
        }
    }

    /// With resampling the printed size stays and the pixel count follows; without,
    /// the pixels stay and the printed size follows.
    pub fn set_resolution(&mut self, text: &str) {
        let Some(v) = parse(text).filter(|v| *v >= 1.0 && *v <= 10_000.0) else { return };
        let v = v as f32;
        if self.resample {
            let k = v / self.dpi;
            self.w = ((self.w as f32 * k).round().max(1.0) as u32).min(MAX_SIDE);
            self.h = ((self.h as f32 * k).round().max(1.0) as u32).min(MAX_SIDE);
        }
        self.dpi = v;
    }

    pub fn set_resample(&mut self, on: bool) {
        self.resample = on;
        if !on {
            self.w = self.base_w;
            self.h = self.base_h;
        }
    }

    pub fn set_proportional(&mut self, on: bool) {
        self.proportional = on;
        if on {
            self.h = ((self.w as f64 * self.base_h as f64 / self.base_w as f64).round().max(1.0) as u32).min(MAX_SIDE);
        }
    }

    pub fn changes_pixels(&self) -> bool {
        (self.w, self.h) != (self.base_w, self.base_h)
    }

    pub fn changes_anything(&self) -> bool {
        self.changes_pixels() || (self.dpi - self.base_dpi).abs() > 0.01
    }

    pub fn result_text(&self) -> String {
        format!(
            "{} × {} pixels, {} × {} in at {} pixels/inch",
            self.w,
            self.h,
            fmt(self.w as f64 / self.dpi as f64, Unit::Inches),
            fmt(self.h as f64 / self.dpi as f64, Unit::Inches),
            fmt(self.dpi as f64, Unit::Inches)
        )
    }
}

fn parse(text: &str) -> Option<f64> {
    text.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

fn fmt(v: f64, unit: Unit) -> String {
    if unit == Unit::Pixels {
        return format!("{}", v.round() as i64);
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proportional_pixel_resize() {
        let mut m = SizeModel::new(4000, 3000, 300.0);
        m.set_width("1000");
        assert_eq!((m.w, m.h), (1000, 750));
        assert!(m.changes_pixels());
        m.unit = Unit::Percent;
        assert_eq!(m.width_text(), "25");
        m.set_height("50");
        assert_eq!((m.w, m.h), (2000, 1500));
        m.set_proportional(false);
        m.set_width("10");
        assert_eq!((m.w, m.h), (400, 1500));
    }

    #[test]
    fn resolution_with_and_without_resampling() {
        let mut m = SizeModel::new(3000, 2000, 300.0);
        m.unit = Unit::Inches;
        assert_eq!(m.width_text(), "10");
        // Resample: same 10 in at 150 ppi needs half the pixels.
        m.set_resolution("150");
        assert_eq!((m.w, m.h), (1500, 1000));
        // No resample: pixels fixed, printed size follows.
        let mut m = SizeModel::new(3000, 2000, 300.0);
        m.set_resample(false);
        m.unit = Unit::Inches;
        m.set_resolution("150");
        assert_eq!((m.w, m.h), (3000, 2000));
        assert_eq!(m.width_text(), "20");
        m.set_width("5");
        assert_eq!(m.resolution_text(), "600");
        assert!(!m.changes_pixels() && m.changes_anything());
        m.unit = Unit::Pixels;
        assert!(!m.size_editable());
    }

    #[test]
    fn bad_input_is_ignored_and_sizes_are_capped() {
        let mut m = SizeModel::new(100, 50, 72.0);
        m.set_width("abc");
        m.set_width("-5");
        assert_eq!((m.w, m.h), (100, 50));
        m.set_width("1e9");
        assert_eq!(m.w, MAX_SIDE);
        m.unit = Unit::Centimetres;
        m.set_width("2,54");
        assert_eq!(m.w, 72);
        assert!(m.result_text().starts_with("72 × 36 pixels"));
    }
}
