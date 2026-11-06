//! Saved signatures, encrypted at rest with DPAPI (current Windows user only).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::geometry::Point;
use crate::platform_windows::dpapi;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    pub id: u64,
    pub name: String,
    /// Normalised strokes: the signature fits a box `aspect` wide and 1 high.
    pub strokes: Vec<Vec<Point>>,
    pub aspect: f32,
    /// Stroke width relative to the signature height.
    #[serde(default = "default_width")]
    pub width: f32,
}

fn default_width() -> f32 {
    0.018
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    signatures: Vec<Signature>,
}

pub fn path() -> PathBuf {
    super::data_dir().join("signatures.dat")
}

pub fn load_from(p: &Path) -> Vec<Signature> {
    let Ok(cipher) = std::fs::read(p) else { return Vec::new() };
    dpapi::unprotect(&cipher)
        .ok()
        .and_then(|plain| serde_json::from_slice::<Store>(&plain).ok())
        .map(|s| s.signatures)
        .unwrap_or_default()
}

pub fn save_to(p: &Path, sigs: &[Signature]) -> Result<(), String> {
    let plain = serde_json::to_vec(&Store { signatures: sigs.to_vec() }).map_err(|e| e.to_string())?;
    let cipher = dpapi::protect(&plain)?;
    super::write_atomic(p, &cipher).map_err(|e| e.to_string())
}

/// Normalises raw strokes (any coordinates) to height 1, origin at the top-left.
pub fn normalise(strokes: &[Vec<Point>]) -> Option<(Vec<Vec<Point>>, f32)> {
    let pts: Vec<Point> = strokes.iter().flatten().copied().collect();
    let first = pts.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
    for p in &pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let h = (y1 - y0).max(1.0);
    let w = (x1 - x0).max(1.0);
    let out = strokes
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.iter().map(|p| Point::new((p.x - x0) / h, (p.y - y0) / h)).collect())
        .collect();
    Some((out, w / h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_encrypted_and_reloaded() {
        let dir = std::env::temp_dir().join(format!("pm-sig-{}", std::process::id()));
        let p = dir.join("signatures.dat");
        let (strokes, aspect) = normalise(&[vec![Point::new(10.0, 20.0), Point::new(110.0, 70.0)]]).unwrap();
        assert_eq!(aspect, 2.0);
        assert_eq!(strokes[0][1], Point::new(2.0, 1.0));
        let sig = Signature { id: 1, name: "Ada Lovelace".into(), strokes, aspect, width: 0.02 };
        save_to(&p, std::slice::from_ref(&sig)).unwrap();
        let raw = std::fs::read(&p).unwrap();
        assert!(!raw.windows(4).any(|w| w == b"Ada "), "stored data is encrypted");
        assert_eq!(load_from(&p), vec![sig]);
        assert!(load_from(&dir.join("missing.dat")).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
