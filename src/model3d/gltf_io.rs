//! glTF 2.0 (JSON or GLB) through the `gltf` crate, without its importer: buffers
//! come from the GLB blob, `data:` URIs or files in the same folder (see
//! `model3d::sidecar`). The default scene is flattened with node transforms; colours
//! are vertex colours times the material's base colour. Textures are ignored.

use super::{DEFAULT_COLOR, MAX_TRIANGLES, MAX_VERTICES, Mesh};

type Mat4 = [[f32; 4]; 4];

const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// Column-major product a × b.
fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0f32; 4]; 4];
    for (c, col) in r.iter_mut().enumerate() {
        for (row, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][row] * b[c][k]).sum();
        }
    }
    r
}

fn apply_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
        m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
        m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
    ]
}

/// Cofactor of the upper 3×3 (= det × inverse transpose), signed so normals keep
/// pointing outwards under mirroring transforms.
fn normal_matrix(m: &Mat4) -> [[f32; 3]; 3] {
    let a = |r: usize, c: usize| m[c][r];
    let cof = [
        [
            a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1),
            a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2),
            a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0),
        ],
        [
            a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2),
            a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0),
            a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1),
        ],
        [
            a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1),
            a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2),
            a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0),
        ],
    ];
    let det = a(0, 0) * cof[0][0] + a(0, 1) * cof[0][1] + a(0, 2) * cof[0][2];
    let s = if det < 0.0 { -1.0 } else { 1.0 };
    cof.map(|row| row.map(|v| v * s))
}

fn apply_normal(n: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    super::normalize([
        n[0][0] * v[0] + n[0][1] * v[1] + n[0][2] * v[2],
        n[1][0] * v[0] + n[1][1] * v[1] + n[1][2] * v[2],
        n[2][0] * v[0] + n[2][1] * v[1] + n[2][2] * v[2],
    ])
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        acc = (acc << 6) | val(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse(bytes: &[u8], mut sidecar: impl FnMut(&str) -> Option<Vec<u8>>) -> Result<Mesh, String> {
    let g = gltf::Gltf::from_slice(bytes).map_err(|e| format!("The glTF file is damaged ({e})."))?;
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for b in g.buffers() {
        let data = match b.source() {
            gltf::buffer::Source::Bin => g.blob.clone().ok_or("The GLB file has no binary chunk.")?,
            gltf::buffer::Source::Uri(uri) => {
                if let Some(rest) = uri.strip_prefix("data:") {
                    let (_, payload) = rest.split_once(',').ok_or("A glTF data URI is malformed.")?;
                    base64_decode(payload).ok_or("A glTF data URI is malformed.")?
                } else {
                    sidecar(&percent_decode(uri))
                        .ok_or_else(|| format!("The glTF buffer “{uri}” is missing. It must be next to the model."))?
                }
            }
        };
        if data.len() < b.length() {
            return Err("A glTF buffer is shorter than declared.".into());
        }
        buffers.push(data);
    }
    let mut out = Mesh::default();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut any_color = false;
    let mut add = |mesh: gltf::Mesh, m: &Mat4| -> Result<(), String> {
        let nm = normal_matrix(m);
        for prim in mesh.primitives() {
            use gltf::mesh::Mode;
            let mode = prim.mode();
            if !matches!(mode, Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan) {
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|v| &v[..]));
            let Some(pos) = reader.read_positions() else { continue };
            let pos: Vec<[f32; 3]> = pos.collect();
            if out.positions.len() + pos.len() > MAX_VERTICES {
                return Err("The model is too large.".into());
            }
            let normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(|n| n.collect());
            let vcol: Option<Vec<[u8; 3]>> = reader.read_colors(0).map(|c| c.into_rgb_u8().collect());
            let f = prim.material().pbr_metallic_roughness().base_color_factor();
            let tint =
                |c: [u8; 3]| [0, 1, 2].map(|k| (c[k] as f32 * f[k].clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u8);
            let has_material = prim.material().index().is_some();
            any_color |= vcol.is_some() || has_material;
            let base = out.positions.len() as u32;
            for (i, p) in pos.iter().enumerate() {
                out.positions.push(apply_point(m, *p));
                out.normals
                    .push(normals.as_ref().and_then(|n| n.get(i)).map(|v| apply_normal(&nm, *v)).unwrap_or([0.0; 3]));
                let c = vcol.as_ref().and_then(|c| c.get(i)).copied().unwrap_or([255, 255, 255]);
                colors.push(if vcol.is_some() || has_material { tint(c) } else { DEFAULT_COLOR });
            }
            let idx: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            let n = pos.len() as u32;
            let mut push = |t: [u32; 3]| -> Result<(), String> {
                if t.iter().any(|i| *i >= n) {
                    return Err("The glTF file has an index out of range.".into());
                }
                out.triangles.push(t.map(|i| i + base));
                Ok(())
            };
            match mode {
                Mode::Triangles => {
                    for t in idx.as_chunks::<3>().0 {
                        push([t[0], t[1], t[2]])?;
                    }
                }
                Mode::TriangleStrip => {
                    for k in 2..idx.len() {
                        push(if k % 2 == 0 {
                            [idx[k - 2], idx[k - 1], idx[k]]
                        } else {
                            [idx[k - 1], idx[k - 2], idx[k]]
                        })?;
                    }
                }
                _ => {
                    for k in 2..idx.len() {
                        push([idx[0], idx[k - 1], idx[k]])?;
                    }
                }
            }
            if out.triangles.len() > MAX_TRIANGLES {
                return Err("The model is too large.".into());
            }
            // Primitives without normals get smooth ones (over this primitive).
            if normals.is_none() {
                let mut part = Mesh {
                    positions: out.positions[base as usize..].to_vec(),
                    normals: Vec::new(),
                    colors: None,
                    triangles: out.triangles.iter().filter(|t| t[0] >= base).map(|t| t.map(|i| i - base)).collect(),
                };
                part.smooth_normals();
                out.normals.truncate(base as usize);
                out.normals.extend(part.normals);
            }
        }
        Ok(())
    };
    fn walk(
        node: gltf::Node,
        parent: &Mat4,
        depth: usize,
        add: &mut dyn FnMut(gltf::Mesh, &Mat4) -> Result<(), String>,
    ) -> Result<(), String> {
        if depth > 64 {
            return Err("The glTF scene is nested too deeply.".into());
        }
        let m = mul(parent, &node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            add(mesh, &m)?;
        }
        for child in node.children() {
            walk(child, &m, depth + 1, add)?;
        }
        Ok(())
    }
    match g.default_scene().or_else(|| g.scenes().next()) {
        Some(scene) => {
            for node in scene.nodes() {
                walk(node, &IDENTITY, 0, &mut add)?;
            }
        }
        None => {
            for mesh in g.meshes() {
                add(mesh, &IDENTITY)?;
            }
        }
    }
    out.colors = any_color.then_some(colors);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glb_round_trip_with_colours() {
        let mut cube = crate::model3d::tests::cube();
        cube.colors = Some(vec![[200, 30, 30]; 8]);
        let glb = crate::model3d::export::glb(&cube);
        let m = parse(&glb, |_| None).unwrap();
        assert_eq!(m.triangles, cube.triangles);
        assert_eq!(m.positions, cube.positions);
        assert_eq!(m.colors.unwrap()[0], [200, 30, 30]);
    }

    #[test]
    fn gltf_with_data_uri_transform_and_no_normals() {
        // One triangle, translated by (10, 0, 0) and scaled by 2 on a node.
        let pos: Vec<u8> = [[0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .iter()
            .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut enc = String::new();
        for c in pos.chunks(3) {
            let n = c.iter().fold(0u32, |a, b| (a << 8) | *b as u32) << (8 * (3 - c.len()));
            for k in 0..=c.len() {
                enc.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char);
            }
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
            "nodes":[{{"mesh":0,"translation":[10,0,0],"scale":[2,2,2]}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}}],
            "bufferViews":[{{"buffer":0,"byteLength":36}}],
            "buffers":[{{"byteLength":36,"uri":"data:application/octet-stream;base64,{enc}"}}]}}"#
        );
        let m = parse(json.as_bytes(), |_| None).unwrap();
        assert_eq!(m.positions, vec![[10.0, 0.0, 0.0], [12.0, 0.0, 0.0], [10.0, 2.0, 0.0]]);
        assert_eq!(m.normals[0], [0.0, 0.0, 1.0]);
        assert!(m.colors.is_none());
    }

    #[test]
    fn missing_external_buffer_is_explained() {
        let json = br#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"mesh.bin"}]}"#;
        assert!(parse(json, |_| None).unwrap_err().contains("mesh.bin"));
        assert!(parse(b"{not json", |_| None).is_err());
        assert_eq!(percent_decode("my%20mesh.bin"), "my mesh.bin");
    }
}
