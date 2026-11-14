//! A CPU rasteriser for the 3D view (no GPU needed, ADR 0009). Vertices are
//! transformed once; the image is split into horizontal bands rendered in parallel,
//! each with its own z-buffer rows. Triangles crossing the near plane are clipped.

use super::{DEFAULT_COLOR, Mesh, normalize};

pub const FOV_Y: f32 = 35.0 * std::f32::consts::PI / 180.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Degrees around the vertical axis.
    pub yaw: f32,
    /// Degrees above the horizon.
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
    /// Bounding-sphere radius of the model (for zoom limits and clipping).
    pub radius: f32,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Camera {
    /// A three-quarter view that fits the whole model.
    pub fn fit(mesh: &Mesh) -> Camera {
        let (lo, hi) = mesh.bounds();
        let target = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        let radius = (dot(d, d).sqrt() / 2.0).max(1e-6);
        Camera { yaw: -32.0, pitch: 22.0, distance: radius / (FOV_Y / 2.0).sin() * 1.08, target, radius }
    }

    /// Unit vectors: right, up, forward (from the eye towards the target).
    pub fn basis(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let (y, p) = (self.yaw.to_radians(), self.pitch.to_radians());
        let back = [p.cos() * y.sin(), p.sin(), p.cos() * y.cos()];
        let f = [-back[0], -back[1], -back[2]];
        let r = normalize([f[1] * 0.0 - f[2] * 1.0, f[2] * 0.0 - f[0] * 0.0, f[0] * 1.0 - f[1] * 0.0]);
        let u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
        (r, u, f)
    }

    pub fn eye(&self) -> [f32; 3] {
        let (_, _, f) = self.basis();
        [
            self.target[0] - f[0] * self.distance,
            self.target[1] - f[1] * self.distance,
            self.target[2] - f[2] * self.distance,
        ]
    }

    /// Orbit by a pointer movement in logical pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw = (self.yaw - dx * 0.4).rem_euclid(360.0);
        self.pitch = (self.pitch + dy * 0.4).clamp(-89.0, 89.0);
    }

    /// Pan so the model follows the pointer (`view_h` is the view height in pixels).
    pub fn pan(&mut self, dx: f32, dy: f32, view_h: f32) {
        let (r, u, _) = self.basis();
        let k = 2.0 * self.distance * (FOV_Y / 2.0).tan() / view_h.max(1.0);
        for i in 0..3 {
            self.target[i] += (-dx * r[i] + dy * u[i]) * k;
        }
    }

    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(self.radius * 0.05, self.radius * 60.0);
    }
}

/// A vertex after transformation: clip-space x, y, w, view-space normal, colour.
#[derive(Clone, Copy, Default)]
struct V {
    x: f32,
    y: f32,
    w: f32,
    n: [f32; 3],
    c: [f32; 3],
}

impl V {
    fn lerp(a: &V, b: &V, t: f32) -> V {
        let l = |p: f32, q: f32| p + (q - p) * t;
        V {
            x: l(a.x, b.x),
            y: l(a.y, b.y),
            w: l(a.w, b.w),
            n: [l(a.n[0], b.n[0]), l(a.n[1], b.n[1]), l(a.n[2], b.n[2])],
            c: [l(a.c[0], b.c[0]), l(a.c[1], b.c[1]), l(a.c[2], b.c[2])],
        }
    }
}

/// A vertex in screen space: position, 1/w, and attributes.
#[derive(Clone, Copy)]
struct S {
    x: f32,
    y: f32,
    iw: f32,
    n: [f32; 3],
    c: [f32; 3],
}

/// The background behind a model: a soft vertical gradient (top, bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    /// For the light theme and for exported images.
    Light,
    /// For the dark theme.
    Dark,
}

impl Backdrop {
    fn colors(self) -> ([f32; 3], [f32; 3]) {
        match self {
            Backdrop::Light => ([238.0, 240.0, 244.0], [206.0, 211.0, 220.0]),
            Backdrop::Dark => ([44.0, 47.0, 54.0], [24.0, 26.0, 31.0]),
        }
    }
}

/// Renders `mesh` seen through `cam` into a `w` × `h` RGBA image, supersampled `ss`×,
/// on the light backdrop.
pub fn render(mesh: &Mesh, cam: &Camera, w: u32, h: u32, ss: u32) -> Vec<u8> {
    render_on(mesh, cam, w, h, ss, Backdrop::Light)
}

/// As [`render`], on the given backdrop.
pub fn render_on(mesh: &Mesh, cam: &Camera, w: u32, h: u32, ss: u32, backdrop: Backdrop) -> Vec<u8> {
    let (bg_top, bg_bottom) = backdrop.colors();
    let ss = ss.clamp(1, 4);
    let (bw, bh) = (w.max(1) * ss, h.max(1) * ss);
    let (r, u, f) = cam.basis();
    let eye = cam.eye();
    let fy = 1.0 / (FOV_Y / 2.0).tan();
    let fx = fy * bh as f32 / bw as f32;
    let near = (cam.distance * 0.002).max(cam.radius * 1e-4);
    // Transform every vertex once.
    let colors = mesh.colors.as_deref();
    let verts: Vec<V> = mesh
        .positions
        .iter()
        .zip(&mesh.normals)
        .enumerate()
        .map(|(i, (p, n))| {
            let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
            let c = colors.map(|c| c[i]).unwrap_or(DEFAULT_COLOR);
            V {
                x: dot(d, r) * fx,
                y: dot(d, u) * fy,
                w: dot(d, f),
                n: [dot(*n, r), dot(*n, u), -dot(*n, f)],
                c: [c[0] as f32, c[1] as f32, c[2] as f32],
            }
        })
        .collect();
    let to_screen = |v: &V| S {
        x: (v.x / v.w * 0.5 + 0.5) * bw as f32,
        y: (0.5 - v.y / v.w * 0.5) * bh as f32,
        iw: 1.0 / v.w,
        n: v.n,
        c: v.c,
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
    let rows_per = (bh as usize).div_ceil(threads);
    let mut color = vec![0u8; (bw * bh * 4) as usize];
    std::thread::scope(|scope| {
        for (band, chunk) in color.chunks_mut(rows_per * bw as usize * 4).enumerate() {
            let verts = &verts;
            let to_screen = &to_screen;
            scope.spawn(move || {
                let y0 = band * rows_per;
                let rows = chunk.len() / (bw as usize * 4);
                let mut depth = vec![0f32; rows * bw as usize];
                // Background gradient.
                for ry in 0..rows {
                    let t = (y0 + ry) as f32 / bh as f32;
                    let px = [0, 1, 2].map(|k| (bg_top[k] + (bg_bottom[k] - bg_top[k]) * t) as u8);
                    for x in 0..bw as usize {
                        chunk[(ry * bw as usize + x) * 4..][..4].copy_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                }
                let mut band = Band { y0, rows, w: bw as usize, color: chunk, depth: &mut depth };
                for t in &mesh.triangles {
                    let [a, b, c] = t.map(|i| verts[i as usize]);
                    let inside = [a.w >= near, b.w >= near, c.w >= near];
                    if inside.iter().all(|i| *i) {
                        band.triangle(&to_screen(&a), &to_screen(&b), &to_screen(&c));
                    } else if inside.iter().any(|i| *i) {
                        // Clip against the near plane (Sutherland–Hodgman on one plane).
                        let poly = [a, b, c];
                        let mut out: Vec<V> = Vec::with_capacity(4);
                        for k in 0..3 {
                            let (p, q) = (&poly[k], &poly[(k + 1) % 3]);
                            let (pi, qi) = (p.w >= near, q.w >= near);
                            if pi {
                                out.push(*p);
                            }
                            if pi != qi {
                                out.push(V::lerp(p, q, (near - p.w) / (q.w - p.w)));
                            }
                        }
                        let s: Vec<S> = out.iter().map(to_screen).collect();
                        for k in 1..s.len().saturating_sub(1) {
                            band.triangle(&s[0], &s[k], &s[k + 1]);
                        }
                    }
                }
            });
        }
    });
    if ss == 1 {
        return color;
    }
    // Box-filter down to the requested size.
    let mut out = vec![0u8; (w * h * 4) as usize];
    let n = ss * ss;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let mut acc = [0u32; 4];
            for sy in 0..ss as usize {
                let row = (y * ss as usize + sy) * bw as usize;
                for sx in 0..ss as usize {
                    let i = (row + x * ss as usize + sx) * 4;
                    for k in 0..4 {
                        acc[k] += color[i + k] as u32;
                    }
                }
            }
            let o = (y * w as usize + x) * 4;
            for k in 0..4 {
                out[o + k] = ((acc[k] + n / 2) / n) as u8;
            }
        }
    }
    out
}

struct Band<'a> {
    y0: usize,
    rows: usize,
    w: usize,
    color: &'a mut [u8],
    depth: &'a mut [f32],
}

const KEY: [f32; 3] = [-0.45, 0.75, 0.48];

impl Band<'_> {
    fn triangle(&mut self, a: &S, b: &S, c: &S) {
        let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if !area.is_finite() || area.abs() < 1e-9 {
            return;
        }
        let min_y = a.y.min(b.y).min(c.y).floor().max(self.y0 as f32) as i64;
        let max_y = a.y.max(b.y).max(c.y).ceil().min((self.y0 + self.rows) as f32) as i64;
        let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i64;
        let max_x = a.x.max(b.x).max(c.x).ceil().min(self.w as f32) as i64;
        if min_y >= max_y || min_x >= max_x {
            return;
        }
        let inv = 1.0 / area;
        let key = normalize(KEY);
        let half = normalize([key[0], key[1], key[2] + 1.0]);
        for py in min_y..max_y {
            let fy = py as f32 + 0.5;
            let row = (py as usize - self.y0) * self.w;
            for px in min_x..max_x {
                let fx = px as f32 + 0.5;
                let w0 = ((c.x - b.x) * (fy - b.y) - (c.y - b.y) * (fx - b.x)) * inv;
                let w1 = ((a.x - c.x) * (fy - c.y) - (a.y - c.y) * (fx - c.x)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let iw = w0 * a.iw + w1 * b.iw + w2 * c.iw;
                let i = row + px as usize;
                if iw <= self.depth[i] {
                    continue;
                }
                self.depth[i] = iw;
                let mut n = normalize([
                    w0 * a.n[0] + w1 * b.n[0] + w2 * c.n[0],
                    w0 * a.n[1] + w1 * b.n[1] + w2 * c.n[1],
                    w0 * a.n[2] + w1 * b.n[2] + w2 * c.n[2],
                ]);
                // Two-sided: back faces are lit as if facing the viewer.
                if n[2] < 0.0 {
                    n = [-n[0], -n[1], -n[2]];
                }
                let diffuse = 0.2 + 0.5 * n[2].max(0.0) + 0.38 * dot(n, key).max(0.0);
                let spec = dot(n, half).max(0.0).powi(40) * 60.0;
                let o = i * 4;
                for k in 0..3 {
                    let base = w0 * a.c[k] + w1 * b.c[k] + w2 * c.c[k];
                    self.color[o + k] = (base * diffuse + spec).clamp(0.0, 255.0) as u8;
                }
                self.color[o + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_fills_the_centre_not_the_corners() {
        let cube = crate::model3d::tests::cube();
        let cam = Camera::fit(&cube);
        let img = render(&cube, &cam, 120, 100, 1);
        let px = |x: usize, y: usize| &img[(y * 120 + x) * 4..][..3];
        let centre = px(60, 50);
        let corner = px(1, 1);
        assert_ne!(centre, corner);
        assert!(corner[0] > 230, "background at the corner: {corner:?}");
        // Supersampled output has the requested size.
        assert_eq!(render(&cube, &cam, 40, 30, 2).len(), 40 * 30 * 4);
    }

    #[test]
    fn dark_backdrop_for_the_dark_theme() {
        let cube = Mesh::flat(Vec::new(), None);
        let cam = Camera { yaw: 0.0, pitch: 0.0, distance: 5.0, target: [0.0; 3], radius: 1.0 };
        let light = render(&cube, &cam, 8, 8, 1);
        let dark = render_on(&cube, &cam, 8, 8, 1, Backdrop::Dark);
        assert!(light[0] > 230 && dark[0] < 60, "light {} dark {}", light[0], dark[0]);
        assert_eq!(dark[3], 255, "opaque");
    }

    #[test]
    fn nearer_triangle_wins() {
        // Two parallel squares facing the camera; the nearer one is red.
        let quad =
            |z: f32| [[-1.0, -1.0, z], [1.0, -1.0, z], [1.0, 1.0, z], [-1.0, -1.0, z], [1.0, 1.0, z], [-1.0, 1.0, z]];
        let mut corners = quad(0.0).to_vec();
        corners.extend(quad(0.5));
        let colors: Vec<[u8; 3]> = (0..12).map(|i| if i < 6 { [0, 0, 255] } else { [255, 0, 0] }).collect();
        let mesh = Mesh::flat(corners, Some(colors));
        let cam = Camera { yaw: 0.0, pitch: 0.0, distance: 5.0, target: [0.0; 3], radius: 1.5 };
        let img = render(&mesh, &cam, 64, 64, 1);
        let p = &img[(32 * 64 + 32) * 4..][..3];
        assert!(p[0] > 100 && p[2] < 50, "front (red) square visible: {p:?}");
    }

    #[test]
    fn camera_inside_the_model_clips_without_panicking() {
        let cube = crate::model3d::tests::cube();
        let mut cam = Camera::fit(&cube);
        cam.distance = 0.3;
        cam.radius = 0.0001;
        let img = render(&cube, &cam, 50, 40, 1);
        assert_eq!(img.len(), 50 * 40 * 4);
        let mut c2 = Camera::fit(&cube);
        c2.orbit(100.0, -500.0);
        assert!(c2.pitch >= -89.0);
        c2.zoom(1e-6);
        assert!(c2.distance >= c2.radius * 0.05);
    }
}
