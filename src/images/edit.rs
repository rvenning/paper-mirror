//! Pixel edits: flip, resize, colour adjustment, auto levels and Instant Alpha.

use super::Frame;

pub fn flip_horizontal(f: &mut Frame) {
    let row = f.width as usize * 4;
    for line in f.rgba.chunks_exact_mut(row) {
        let n = line.len() / 4;
        for x in 0..n / 2 {
            let (a, b) = (x * 4, (n - 1 - x) * 4);
            for c in 0..4 {
                line.swap(a + c, b + c);
            }
        }
    }
}

pub fn flip_vertical(f: &mut Frame) {
    let row = f.width as usize * 4;
    let h = f.height as usize;
    for y in 0..h / 2 {
        let (top, rest) = f.rgba.split_at_mut((h - 1 - y) * row);
        top[y * row..(y + 1) * row].swap_with_slice(&mut rest[..row]);
    }
}

/// Resamples to `width` × `height` (Catmull-Rom on premultiplied alpha, so transparent
/// pixels do not bleed their hidden colour into the edges).
pub fn resize(f: &Frame, width: u32, height: u32) -> Frame {
    let width = width.max(1);
    let height = height.max(1);
    let mut pre = f.rgba.clone();
    for p in pre.as_chunks_mut::<4>().0.iter_mut() {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
    let img = image::RgbaImage::from_raw(f.width, f.height, pre).expect("sized");
    let out = image::imageops::resize(&img, width, height, image::imageops::FilterType::CatmullRom);
    let mut rgba = out.into_raw();
    for p in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Frame { width, height, rgba, delay_ms: f.delay_ms }
}

/// Preview's Adjust Color controls. The default value changes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorAdjust {
    /// Levels: input black and white points (0..1).
    pub black: f32,
    pub white: f32,
    /// Exposure in stops (-2..2).
    pub exposure: f32,
    /// -1 (flat) .. 1 (strong).
    pub contrast: f32,
    /// 0 (gray) .. 2 (vivid); 1 is unchanged.
    pub saturation: f32,
    /// -1 (cool) .. 1 (warm).
    pub temperature: f32,
    /// -1 (green) .. 1 (magenta).
    pub tint: f32,
    /// 0..1.
    pub sepia: f32,
    /// 0..1, an unsharp mask.
    pub sharpness: f32,
}

impl Default for ColorAdjust {
    fn default() -> Self {
        ColorAdjust {
            black: 0.0,
            white: 1.0,
            exposure: 0.0,
            contrast: 0.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            sepia: 0.0,
            sharpness: 0.0,
        }
    }
}

impl ColorAdjust {
    pub fn is_identity(&self) -> bool {
        *self == ColorAdjust::default()
    }

    fn tone_lut(&self) -> [u8; 256] {
        let mut lut = [0u8; 256];
        let range = (self.white - self.black).max(0.01);
        let gain = 2f32.powf(self.exposure);
        for (i, out) in lut.iter_mut().enumerate() {
            let mut v = (i as f32 / 255.0 - self.black) / range;
            v *= gain;
            v = (v - 0.5) * (1.0 + self.contrast) + 0.5;
            *out = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        lut
    }

    /// Adjusts RGBA pixels in place (alpha untouched).
    pub fn apply(&self, rgba: &mut [u8], width: u32, height: u32) {
        if self.is_identity() {
            return;
        }
        let lut = self.tone_lut();
        let colour = self.saturation != 1.0 || self.temperature != 0.0 || self.tint != 0.0 || self.sepia != 0.0;
        for p in rgba.as_chunks_mut::<4>().0.iter_mut() {
            let (mut r, mut g, mut b) =
                (lut[p[0] as usize] as f32, lut[p[1] as usize] as f32, lut[p[2] as usize] as f32);
            if colour {
                r += 25.0 * self.temperature;
                b -= 25.0 * self.temperature;
                g -= 20.0 * self.tint;
                let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                r = l + (r - l) * self.saturation;
                g = l + (g - l) * self.saturation;
                b = l + (b - l) * self.saturation;
                if self.sepia > 0.0 {
                    let (sr, sg, sb) = (
                        0.393 * r + 0.769 * g + 0.189 * b,
                        0.349 * r + 0.686 * g + 0.168 * b,
                        0.272 * r + 0.534 * g + 0.131 * b,
                    );
                    r += (sr - r) * self.sepia;
                    g += (sg - g) * self.sepia;
                    b += (sb - b) * self.sepia;
                }
            }
            p[0] = r.round().clamp(0.0, 255.0) as u8;
            p[1] = g.round().clamp(0.0, 255.0) as u8;
            p[2] = b.round().clamp(0.0, 255.0) as u8;
        }
        if self.sharpness > 0.0 {
            sharpen(rgba, width, height, self.sharpness * 1.5);
        }
    }
}

/// Unsharp mask with a 3×3 box blur.
fn sharpen(rgba: &mut [u8], width: u32, height: u32, amount: f32) {
    let (w, h) = (width as usize, height as usize);
    if w < 3 || h < 3 {
        return;
    }
    let src = rgba.to_vec();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = (y * w + x) * 4;
            for c in 0..3 {
                let mut sum = 0u32;
                for dy in 0..3 {
                    for dx in 0..3 {
                        sum += src[((y + dy - 1) * w + x + dx - 1) * 4 + c] as u32;
                    }
                }
                let blur = sum as f32 / 9.0;
                let v = src[i + c] as f32;
                rgba[i + c] = (v + (v - blur) * amount).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Auto Levels: black and white points at the 0.5% and 99.5% luminance percentiles
/// of the visible pixels.
pub fn auto_levels(frames: &[&Frame]) -> (f32, f32) {
    let mut hist = [0u64; 256];
    for f in frames {
        for p in f.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 0) {
            let l = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).round() as usize;
            hist[l.min(255)] += 1;
        }
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return (0.0, 1.0);
    }
    let cut = (total as f64 * 0.005) as u64;
    let mut acc = 0;
    let lo = hist.iter().position(|n| {
        acc += n;
        acc > cut
    });
    acc = 0;
    let hi = hist.iter().rposition(|n| {
        acc += n;
        acc > cut
    });
    match (lo, hi) {
        (Some(lo), Some(hi)) if hi > lo + 12 => (lo as f32 / 255.0, hi as f32 / 255.0),
        _ => (0.0, 1.0),
    }
}

/// Instant Alpha: pixels connected to `seed` (4-neighbourhood) whose colour is within
/// `tolerance` (0..1, largest channel difference) of the seed colour.
pub fn similar_region(f: &Frame, seed: (u32, u32), tolerance: f32) -> Vec<bool> {
    let (w, h) = (f.width as usize, f.height as usize);
    let mut mask = vec![false; w * h];
    let (sx, sy) = (seed.0 as usize, seed.1 as usize);
    if sx >= w || sy >= h {
        return mask;
    }
    let target = f.pixel(seed.0, seed.1);
    let limit = (tolerance.clamp(0.0, 1.0) * 255.0).round() as i32;
    let matches = |i: usize| {
        let p = &f.rgba[i * 4..i * 4 + 4];
        (0..4).all(|c| (p[c] as i32 - target[c] as i32).abs() <= limit)
    };
    let mut stack = vec![(sx, sy)];
    while let Some((x, y)) = stack.pop() {
        let row = y * w;
        if mask[row + x] || !matches(row + x) {
            continue;
        }
        // Extend this run left and right, then queue the rows above and below.
        let mut l = x;
        while l > 0 && !mask[row + l - 1] && matches(row + l - 1) {
            l -= 1;
        }
        let mut r = x;
        while r + 1 < w && !mask[row + r + 1] && matches(row + r + 1) {
            r += 1;
        }
        for i in l..=r {
            mask[row + i] = true;
        }
        for ny in [y.wrapping_sub(1), y + 1] {
            if ny >= h {
                continue;
            }
            let nrow = ny * w;
            let mut i = l;
            while i <= r {
                if !mask[nrow + i] && matches(nrow + i) {
                    stack.push((i, ny));
                    // Skip the rest of this run; it will be filled from the pushed seed.
                    while i <= r && matches(nrow + i) {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
        }
    }
    mask
}

/// Makes masked pixels fully transparent.
pub fn clear_masked(f: &mut Frame, mask: &[bool]) {
    for (p, m) in f.rgba.as_chunks_mut::<4>().0.iter_mut().zip(mask) {
        if *m {
            p.copy_from_slice(&[0, 0, 0, 0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Frame {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&f(x, y));
            }
        }
        Frame::new(w, h, v)
    }

    #[test]
    fn flips() {
        let mut f = frame(3, 2, |x, y| [x as u8, y as u8, 0, 255]);
        flip_horizontal(&mut f);
        assert_eq!(f.pixel(0, 0), [2, 0, 0, 255]);
        assert_eq!(f.pixel(2, 1), [0, 1, 0, 255]);
        flip_vertical(&mut f);
        assert_eq!(f.pixel(0, 0), [2, 1, 0, 255]);
        let mut odd = frame(1, 3, |_, y| [y as u8, 0, 0, 255]);
        flip_vertical(&mut odd);
        assert_eq!((odd.pixel(0, 0)[0], odd.pixel(0, 1)[0], odd.pixel(0, 2)[0]), (2, 1, 0));
    }

    #[test]
    fn resize_keeps_flat_colour_and_transparency() {
        let f = frame(40, 20, |x, _| if x < 20 { [200, 10, 10, 255] } else { [0, 255, 0, 0] });
        let r = resize(&f, 20, 10);
        assert_eq!((r.width, r.height), (20, 10));
        assert_eq!(r.pixel(2, 5), [200, 10, 10, 255]);
        assert_eq!(r.pixel(18, 5)[3], 0);
        // No green fringe from the hidden colour of transparent pixels.
        let edge = r.pixel(10, 5);
        assert!(edge[1] < 40, "{edge:?}");
    }

    #[test]
    fn colour_adjust_identity_and_direction() {
        let mut px = vec![100, 120, 140, 200];
        ColorAdjust::default().apply(&mut px, 1, 1);
        assert_eq!(px, vec![100, 120, 140, 200]);
        let mut brighter = px.clone();
        ColorAdjust { exposure: 1.0, ..Default::default() }.apply(&mut brighter, 1, 1);
        assert!(brighter[0] > 190 && brighter[3] == 200, "{brighter:?}");
        let mut gray = px.clone();
        ColorAdjust { saturation: 0.0, ..Default::default() }.apply(&mut gray, 1, 1);
        assert!(gray[0].abs_diff(gray[2]) <= 1, "{gray:?}");
        let mut warm = px.clone();
        ColorAdjust { temperature: 1.0, ..Default::default() }.apply(&mut warm, 1, 1);
        assert!(warm[0] > px[0] && warm[2] < px[2]);
    }

    #[test]
    fn auto_levels_stretches_a_dull_image() {
        let f = frame(100, 1, |x, _| {
            let v = 60 + x as u8;
            [v, v, v, 255]
        });
        let (lo, hi) = auto_levels(&[&f]);
        assert!((lo * 255.0 - 60.0).abs() < 2.0 && (hi * 255.0 - 159.0).abs() < 2.0, "{lo} {hi}");
        let mut px = f.rgba.clone();
        ColorAdjust { black: lo, white: hi, ..Default::default() }.apply(&mut px, 100, 1);
        assert!(px[0] <= 3 && px[99 * 4] >= 252);
    }

    #[test]
    fn instant_alpha_takes_only_the_connected_region() {
        // White background with a black bar splitting it; a white island on the right.
        let f = frame(10, 5, |x, _| if x == 5 { [0, 0, 0, 255] } else { [250, 250, 250, 255] });
        let m = similar_region(&f, (0, 0), 0.1);
        assert_eq!(m.iter().filter(|b| **b).count(), 25);
        assert!(!m[5] && !m[9]);
        let mut g = f.clone();
        clear_masked(&mut g, &m);
        assert_eq!(g.pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(g.pixel(9, 4), [250, 250, 250, 255]);
        // Tolerance: a slightly different shade joins only when tolerance allows it.
        let f2 = frame(4, 1, |x, _| [200 + x as u8 * 10, 0, 0, 255]);
        assert_eq!(similar_region(&f2, (0, 0), 0.0).iter().filter(|b| **b).count(), 1);
        assert_eq!(similar_region(&f2, (0, 0), 0.2).iter().filter(|b| **b).count(), 4);
    }
}
