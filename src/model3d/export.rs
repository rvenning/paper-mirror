//! Writing meshes: binary STL, OBJ (with the "v x y z r g b" colour extension),
//! binary PLY and GLB (glTF 2.0 binary).

use std::fmt::Write as _;

use super::{Mesh, face_normal, normalize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelExport {
    Stl,
    Obj,
    Ply,
    Glb,
}

impl ModelExport {
    pub const ALL: [ModelExport; 4] = [ModelExport::Glb, ModelExport::Obj, ModelExport::Stl, ModelExport::Ply];

    pub fn name(self) -> &'static str {
        match self {
            ModelExport::Stl => "STL",
            ModelExport::Obj => "OBJ",
            ModelExport::Ply => "PLY",
            ModelExport::Glb => "glTF binary (GLB)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ModelExport::Stl => "stl",
            ModelExport::Obj => "obj",
            ModelExport::Ply => "ply",
            ModelExport::Glb => "glb",
        }
    }

    pub fn from_extension(ext: &str) -> Option<ModelExport> {
        ModelExport::ALL.into_iter().find(|f| f.extension().eq_ignore_ascii_case(ext))
    }

    pub fn write(self, m: &Mesh) -> Vec<u8> {
        match self {
            ModelExport::Stl => stl_binary(m),
            ModelExport::Obj => obj(m).into_bytes(),
            ModelExport::Ply => ply_binary(m),
            ModelExport::Glb => glb(m),
        }
    }
}

pub fn stl_binary(m: &Mesh) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + m.triangles.len() * 50);
    let mut header = [0u8; 80];
    let label = b"Paper Mirror binary STL";
    header[..label.len()].copy_from_slice(label);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(m.triangles.len() as u32).to_le_bytes());
    for t in &m.triangles {
        let [a, b, c] = t.map(|i| m.positions[i as usize]);
        for v in [normalize(face_normal(a, b, c)), a, b, c] {
            for x in v {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

pub fn obj(m: &Mesh) -> String {
    let mut s = String::with_capacity(m.positions.len() * 60 + m.triangles.len() * 30);
    s.push_str("# Paper Mirror\n");
    for (i, p) in m.positions.iter().enumerate() {
        let _ = write!(s, "v {} {} {}", p[0], p[1], p[2]);
        if let Some(c) = m.colors.as_ref().map(|c| c[i]) {
            let _ = write!(s, " {:.4} {:.4} {:.4}", c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
        }
        s.push('\n');
    }
    for n in &m.normals {
        let _ = writeln!(s, "vn {} {} {}", n[0], n[1], n[2]);
    }
    for t in &m.triangles {
        let [a, b, c] = t.map(|i| i + 1);
        let _ = writeln!(s, "f {a}//{a} {b}//{b} {c}//{c}");
    }
    s
}

pub fn ply_binary(m: &Mesh) -> Vec<u8> {
    let mut h = String::from("ply\nformat binary_little_endian 1.0\ncomment Paper Mirror\n");
    let _ = writeln!(h, "element vertex {}", m.positions.len());
    h.push_str("property float x\nproperty float y\nproperty float z\n");
    h.push_str("property float nx\nproperty float ny\nproperty float nz\n");
    if m.colors.is_some() {
        h.push_str("property uchar red\nproperty uchar green\nproperty uchar blue\n");
    }
    let _ = writeln!(h, "element face {}", m.triangles.len());
    h.push_str("property list uchar int vertex_indices\nend_header\n");
    let mut out = h.into_bytes();
    for (i, p) in m.positions.iter().enumerate() {
        for v in p.iter().chain(m.normals[i].iter()) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if let Some(c) = &m.colors {
            out.extend_from_slice(&c[i]);
        }
    }
    for t in &m.triangles {
        out.push(3);
        for i in t {
            out.extend_from_slice(&(*i as i32).to_le_bytes());
        }
    }
    out
}

fn pad4(v: &mut Vec<u8>, byte: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(byte);
    }
}

pub fn glb(m: &Mesh) -> Vec<u8> {
    let (lo, hi) = m.bounds();
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut view = |bin: &mut Vec<u8>, data: Vec<u8>, target: u32| {
        pad4(bin, 0);
        let offset = bin.len();
        bin.extend_from_slice(&data);
        views.push(format!(r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{},"target":{target}}}"#, data.len()));
        views.len() - 1
    };
    let f32s = |v: &[[f32; 3]]| v.iter().flat_map(|p| p.iter().flat_map(|x| x.to_le_bytes())).collect::<Vec<u8>>();
    let n = m.positions.len();
    let pv = view(&mut bin, f32s(&m.positions), 34962);
    let nv = view(&mut bin, f32s(&m.normals), 34962);
    let cv = m.colors.as_ref().map(|c| {
        // Normalised unsigned bytes, padded to 4 bytes per element as glTF requires.
        let data: Vec<u8> = c.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect();
        view(&mut bin, data, 34962)
    });
    let iv = view(&mut bin, m.triangles.iter().flat_map(|t| t.iter().flat_map(|i| i.to_le_bytes())).collect(), 34963);
    pad4(&mut bin, 0);
    let mut accessors = vec![
        format!(
            r#"{{"bufferView":{pv},"componentType":5126,"count":{n},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]}}"#,
            lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
        ),
        format!(r#"{{"bufferView":{nv},"componentType":5126,"count":{n},"type":"VEC3"}}"#),
        format!(r#"{{"bufferView":{iv},"componentType":5125,"count":{},"type":"SCALAR"}}"#, m.triangles.len() * 3),
    ];
    let mut attributes = String::from(r#""POSITION":0,"NORMAL":1"#);
    if let Some(cv) = cv {
        accessors
            .push(format!(r#"{{"bufferView":{cv},"componentType":5121,"normalized":true,"count":{n},"type":"VEC4"}}"#));
        attributes.push_str(r#","COLOR_0":3"#);
    }
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"Paper Mirror"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"meshes":[{{"primitives":[{{"attributes":{{{attributes}}},"indices":2,"mode":4}}]}}],"accessors":[{}],"bufferViews":[{}],"buffers":[{{"byteLength":{}}}]}}"#,
        accessors.join(","),
        views.join(","),
        bin.len()
    );
    let mut json = json.into_bytes();
    pad4(&mut json, b' ');
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_reads_back() {
        let mut cube = crate::model3d::tests::cube();
        cube.colors = Some((0..8).map(|i| [i as u8 * 30, 100, 200]).collect());
        for f in ModelExport::ALL {
            let bytes = f.write(&cube);
            let back = match f {
                ModelExport::Stl => crate::model3d::stl::parse(&bytes),
                ModelExport::Obj => crate::model3d::obj::parse(&bytes, |_| None),
                ModelExport::Ply => crate::model3d::ply::parse(&bytes),
                ModelExport::Glb => crate::model3d::gltf_io::parse(&bytes, |_| None),
            }
            .unwrap_or_else(|e| panic!("{f:?}: {e}"));
            back.validate().unwrap();
            assert_eq!(back.triangles.len(), 12, "{f:?}");
            assert_eq!(back.bounds(), cube.bounds(), "{f:?}");
            if f != ModelExport::Stl {
                let c = back.colors.as_ref().unwrap_or_else(|| panic!("{f:?} lost colours"));
                assert!(c.contains(&[60, 100, 200]), "{f:?}");
            }
        }
        assert_eq!(ModelExport::from_extension("GLB"), Some(ModelExport::Glb));
    }
}
