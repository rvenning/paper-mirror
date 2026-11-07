//! 3D models (ADR 0009): one triangle mesh per file, parsed from STL, OBJ, PLY or
//! glTF/GLB, drawn by a CPU rasteriser, and exported as STL, OBJ, PLY, GLB or an image.

pub mod export;
pub mod gltf_io;
pub mod obj;
pub mod ply;
pub mod raster;
pub mod stl;

use std::path::Path;

use crate::core::filetype::ModelFormat;

/// Largest model accepted.
pub const MAX_TRIANGLES: usize = 20_000_000;
pub const MAX_VERTICES: usize = 60_000_000;
pub const MAX_FILE_BYTES: u64 = 2 << 30;

pub const DEFAULT_COLOR: [u8; 3] = [178, 186, 200];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    /// One per position.
    pub normals: Vec<[f32; 3]>,
    /// One per position when present.
    pub colors: Option<Vec<[u8; 3]>>,
    pub triangles: Vec<[u32; 3]>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-20 && l.is_finite() { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 0.0, 1.0] }
}

/// Unnormalised face normal (length = twice the area).
pub fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    cross(sub(b, a), sub(c, a))
}

impl Mesh {
    /// A flat-shaded mesh: every triangle gets its own three vertices.
    pub fn flat(corners: Vec<[f32; 3]>, colors: Option<Vec<[u8; 3]>>) -> Mesh {
        let n = corners.len() / 3;
        let mut normals = Vec::with_capacity(n * 3);
        for t in corners.as_chunks::<3>().0 {
            let nn = normalize(face_normal(t[0], t[1], t[2]));
            normals.extend_from_slice(&[nn, nn, nn]);
        }
        let triangles = (0..n as u32).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect();
        Mesh { positions: corners, normals, colors, triangles }
    }

    /// Area-weighted vertex normals over shared vertices.
    pub fn smooth_normals(&mut self) {
        let mut acc = vec![[0f32; 3]; self.positions.len()];
        for t in &self.triangles {
            let [a, b, c] = t.map(|i| self.positions[i as usize]);
            let n = face_normal(a, b, c);
            for &i in t {
                let v = &mut acc[i as usize];
                v[0] += n[0];
                v[1] += n[1];
                v[2] += n[2];
            }
        }
        self.normals = acc.into_iter().map(normalize).collect();
    }

    /// Checks indices and values; drops degenerate input early.
    pub fn validate(&self) -> Result<(), String> {
        if self.triangles.is_empty() {
            return Err("The file contains no triangles.".into());
        }
        if self.triangles.len() > MAX_TRIANGLES || self.positions.len() > MAX_VERTICES {
            return Err(format!("The model is too large ({} triangles).", self.triangles.len()));
        }
        let n = self.positions.len() as u32;
        if self.triangles.iter().any(|t| t.iter().any(|i| *i >= n)) {
            return Err("The model refers to vertices that don't exist.".into());
        }
        if self.normals.len() != self.positions.len()
            || self.colors.as_ref().is_some_and(|c| c.len() != self.positions.len())
        {
            return Err("The model's vertex data is inconsistent.".into());
        }
        if self.positions.iter().any(|p| p.iter().any(|v| !v.is_finite())) {
            return Err("The model has invalid coordinates.".into());
        }
        Ok(())
    }

    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for t in &self.triangles {
            for &i in t {
                let p = self.positions[i as usize];
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        if lo[0] > hi[0] { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
    }

    /// Applies a linear map (rotations only, so normals use the same matrix).
    fn rotate(&mut self, m: [[f32; 3]; 3]) {
        let apply = |p: [f32; 3]| {
            [
                m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
                m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
                m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
            ]
        };
        for p in &mut self.positions {
            *p = apply(*p);
        }
        for n in &mut self.normals {
            *n = apply(*n);
        }
    }

    /// Quarter turns about the vertical (Y) axis, clockwise seen from above.
    pub fn rotate_quarter(&mut self, turns: i32) {
        for _ in 0..turns.rem_euclid(4) {
            self.rotate([[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
        }
    }

    /// Z-up (CAD, 3D printing) to Y-up: (x, y, z) -> (x, z, -y).
    pub fn flip_up_axis(&mut self) {
        self.rotate([[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]]);
    }
}

/// Facts shown in the Inspector.
#[derive(Clone, Debug, Default)]
pub struct ModelInfo {
    pub format: &'static str,
    pub triangles: usize,
    pub vertices: usize,
    pub has_colors: bool,
    pub size: [f32; 3],
}

pub fn format_name(f: ModelFormat) -> &'static str {
    match f {
        ModelFormat::Stl => "STL",
        ModelFormat::Obj => "OBJ",
        ModelFormat::Ply => "PLY",
        ModelFormat::Gltf => "glTF",
        ModelFormat::Glb => "glTF binary (GLB)",
        ModelFormat::Usd => "USD",
        ModelFormat::Usdz => "USDZ",
        ModelFormat::Abc => "Alembic",
        ModelFormat::Mtlx => "MaterialX",
        ModelFormat::Vdb => "OpenVDB",
    }
}

/// Why a recognised 3D format can't be opened, or None if it can.
pub fn unsupported_reason(f: ModelFormat) -> Option<&'static str> {
    match f {
        ModelFormat::Usd | ModelFormat::Usdz => Some(
            "USD and USDZ scenes need Pixar's OpenUSD libraries, which Paper Mirror doesn't include. Export the model as glTF (GLB), OBJ, STL or PLY to view it here.",
        ),
        ModelFormat::Abc => Some(
            "Alembic caches need the Alembic C++ libraries, which Paper Mirror doesn't include. Export a frame as OBJ, glTF or PLY to view it here.",
        ),
        ModelFormat::Mtlx => Some(
            "MaterialX files describe materials, not shapes, so there is nothing to display. Paper Mirror shows geometry from glTF, OBJ, STL and PLY files.",
        ),
        ModelFormat::Vdb => Some(
            "OpenVDB files hold volumes (smoke, clouds, fluids), which need a volume renderer. Paper Mirror shows surface models from glTF, OBJ, STL and PLY files.",
        ),
        _ => None,
    }
}

/// Reads and parses a model file. `path` locates sidecar files (OBJ materials, glTF
/// buffers), which must sit in the same folder.
pub fn load(path: &Path, format: ModelFormat) -> Result<Mesh, String> {
    if let Some(reason) = unsupported_reason(format) {
        return Err(reason.into());
    }
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_FILE_BYTES {
        return Err("The file is too large to open.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let dir = path.parent();
    let mesh = match format {
        ModelFormat::Stl => stl::parse(&bytes)?,
        ModelFormat::Obj => obj::parse(&bytes, |name| sidecar(dir, name))?,
        ModelFormat::Ply => ply::parse(&bytes)?,
        ModelFormat::Gltf | ModelFormat::Glb => gltf_io::parse(&bytes, |name| sidecar(dir, name))?,
        _ => unreachable!("unsupported formats return above"),
    };
    mesh.validate()?;
    Ok(mesh)
}

/// A file next to the model. Only plain names (no folders, drives or "..") are allowed.
fn sidecar(dir: Option<&Path>, name: &str) -> Option<Vec<u8>> {
    let name = name.trim();
    let bad =
        name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." || name.starts_with('.');
    if bad {
        return None;
    }
    let p = dir?.join(name);
    let len = std::fs::metadata(&p).ok()?.len();
    (len <= MAX_FILE_BYTES).then(|| std::fs::read(p).ok()).flatten()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A unit cube (12 triangles) with shared vertices, outward-facing.
    pub fn cube() -> Mesh {
        let p = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let t = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
            [1, 2, 6],
            [1, 6, 5],
            [0, 4, 7],
            [0, 7, 3],
        ];
        let mut m = Mesh { positions: p.to_vec(), normals: Vec::new(), colors: None, triangles: t.to_vec() };
        m.smooth_normals();
        m
    }

    #[test]
    fn cube_is_valid_and_bounded() {
        let c = cube();
        c.validate().unwrap();
        assert_eq!(c.bounds(), ([0.0; 3], [1.0; 3]));
        // Smoothed corner normal points diagonally outwards.
        let n = c.normals[6];
        assert!(n.iter().all(|v| *v > 0.3), "{n:?}");
    }

    #[test]
    fn rotations_keep_shape() {
        let mut c = cube();
        c.flip_up_axis();
        let (lo, hi) = c.bounds();
        assert_eq!((lo[2], hi[2]), (-1.0, 0.0));
        c.rotate_quarter(4);
        c.rotate_quarter(1);
        let (lo, hi) = c.bounds();
        assert!((hi[0] - lo[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn invalid_meshes_are_rejected() {
        let mut c = cube();
        c.triangles.push([0, 1, 99]);
        assert!(c.validate().is_err());
        let mut c = cube();
        c.positions[0][0] = f32::NAN;
        assert!(c.validate().is_err());
        assert!(Mesh::default().validate().is_err());
    }

    #[test]
    fn sidecar_names_cannot_escape_the_folder() {
        let dir = std::env::temp_dir();
        assert!(sidecar(Some(&dir), "../secret.mtl").is_none());
        assert!(sidecar(Some(&dir), r"C:\Windows\win.ini").is_none());
        assert!(sidecar(Some(&dir), "sub/file.bin").is_none());
    }
}
