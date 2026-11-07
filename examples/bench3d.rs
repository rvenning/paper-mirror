//! Times the 3D rasteriser on a generated torus with about `n` triangles and writes
//! it as a coloured PLY for visual checks.
//!
//!     cargo run --release --example bench3d -- 1000000 [out.ply]

use std::time::Instant;

use paper_mirror::model3d::Mesh;
use paper_mirror::model3d::export::ModelExport;
use paper_mirror::model3d::raster::{Camera, render};

fn torus(target_triangles: usize) -> Mesh {
    let segs = ((target_triangles as f32 / 2.0).sqrt() * 1.6) as usize;
    let rings = (target_triangles / 2 / segs).max(3);
    let (big, small) = (1.0f32, 0.38f32);
    let mut m = Mesh::default();
    let mut colors = Vec::new();
    for i in 0..segs {
        let u = i as f32 / segs as f32 * std::f32::consts::TAU;
        for j in 0..rings {
            let v = j as f32 / rings as f32 * std::f32::consts::TAU;
            // A gentle ripple so the shading has something to show.
            let r = small * (1.0 + 0.06 * (7.0 * u).sin() * (5.0 * v).cos());
            let p = [(big + r * v.cos()) * u.cos(), r * v.sin(), (big + r * v.cos()) * u.sin()];
            m.positions.push(p);
            colors.push([(120.0 + 100.0 * u.cos().abs()) as u8, 140, (200.0 - 80.0 * v.sin().abs()) as u8]);
        }
    }
    for i in 0..segs {
        for j in 0..rings {
            let a = (i * rings + j) as u32;
            let b = (((i + 1) % segs) * rings + j) as u32;
            let c = (((i + 1) % segs) * rings + (j + 1) % rings) as u32;
            let d = (i * rings + (j + 1) % rings) as u32;
            m.triangles.push([a, b, c]);
            m.triangles.push([a, c, d]);
        }
    }
    m.colors = Some(colors);
    m.smooth_normals();
    m
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
    let mesh = torus(n);
    mesh.validate().unwrap();
    println!("triangles: {}, vertices: {}", mesh.triangles.len(), mesh.positions.len());
    let cam = Camera::fit(&mesh);
    for (label, w, h, ss) in [("drag (half res)", 640, 400, 1), ("still 1280x800 x2 SSAA", 1280, 800, 2)] {
        let _ = render(&mesh, &cam, w, h, ss);
        let runs = 5;
        let t = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(render(&mesh, &cam, w, h, ss));
        }
        println!("{label}: {:.1} ms per frame", t.elapsed().as_secs_f64() * 1000.0 / runs as f64);
    }
    if let Some(out) = std::env::args().nth(2) {
        std::fs::write(&out, ModelExport::Ply.write(&mesh)).unwrap();
        println!("wrote {out}");
    }
}
