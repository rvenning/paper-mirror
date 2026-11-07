//! Wavefront OBJ: positions (with the common "v x y z r g b" colour extension),
//! normals, polygon faces (fan-triangulated, negative indices allowed) and material
//! colours (`Kd`) from MTL files in the same folder. Texture coordinates are ignored.

use std::collections::HashMap;

use super::{DEFAULT_COLOR, MAX_TRIANGLES, MAX_VERTICES, Mesh};

fn unit_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn floats<'a>(it: impl Iterator<Item = &'a str>) -> Vec<f32> {
    it.map_while(|s| s.parse::<f32>().ok()).collect()
}

fn parse_mtl(bytes: &[u8], out: &mut HashMap<String, [u8; 3]>) {
    let text = String::from_utf8_lossy(bytes);
    let mut current: Option<String> = None;
    for line in text.lines() {
        let mut it = line.split_ascii_whitespace();
        match it.next() {
            Some("newmtl") => current = Some(it.collect::<Vec<_>>().join(" ")),
            Some("Kd") => {
                let v = floats(it);
                if let (Some(name), [r, g, b, ..]) = (&current, v.as_slice()) {
                    out.insert(name.clone(), [unit_to_u8(*r), unit_to_u8(*g), unit_to_u8(*b)]);
                }
            }
            _ => {}
        }
    }
}

/// Resolves a 1-based (or negative, relative) OBJ index.
fn resolve(i: i64, len: usize) -> Option<usize> {
    let idx = if i > 0 { i - 1 } else { len as i64 + i };
    (idx >= 0 && (idx as usize) < len).then_some(idx as usize)
}

pub fn parse(bytes: &[u8], mut sidecar: impl FnMut(&str) -> Option<Vec<u8>>) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut vcolors: Vec<Option<[u8; 3]>> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut materials: HashMap<String, [u8; 3]> = HashMap::new();
    let mut material: Option<[u8; 3]> = None;
    // Output vertices, shared by (position, normal, colour).
    let mut out = Mesh::default();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut any_color = false;
    let mut any_normal = true;
    let mut map: HashMap<(usize, Option<usize>, [u8; 3]), u32> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_ascii_whitespace();
        match it.next() {
            Some("v") => {
                let v = floats(it);
                if v.len() < 3 {
                    return Err("The OBJ file has a malformed vertex.".into());
                }
                positions.push([v[0], v[1], v[2]]);
                vcolors.push((v.len() >= 6).then(|| [unit_to_u8(v[3]), unit_to_u8(v[4]), unit_to_u8(v[5])]));
                if positions.len() > MAX_VERTICES {
                    return Err("The model is too large.".into());
                }
            }
            Some("vn") => {
                let v = floats(it);
                if v.len() >= 3 {
                    normals.push(super::normalize([v[0], v[1], v[2]]));
                }
            }
            Some("mtllib") => {
                for name in it {
                    if let Some(b) = sidecar(name) {
                        parse_mtl(&b, &mut materials);
                    }
                }
            }
            Some("usemtl") => {
                let name = it.collect::<Vec<_>>().join(" ");
                material = materials.get(&name).copied();
            }
            Some("f") => {
                let mut face: Vec<u32> = Vec::new();
                for corner in it {
                    let mut parts = corner.split('/');
                    let vi = parts.next().and_then(|s| s.parse::<i64>().ok()).and_then(|i| resolve(i, positions.len()));
                    let Some(vi) = vi else { return Err("The OBJ file has a face with a bad vertex index.".into()) };
                    let ni = parts.nth(1).and_then(|s| s.parse::<i64>().ok()).and_then(|i| resolve(i, normals.len()));
                    any_normal &= ni.is_some();
                    let color = vcolors[vi].or(material);
                    any_color |= color.is_some();
                    let color = color.unwrap_or(DEFAULT_COLOR);
                    let next = out.positions.len() as u32;
                    let id = *map.entry((vi, ni, color)).or_insert_with(|| {
                        out.positions.push(positions[vi]);
                        out.normals.push(ni.map(|n| normals[n]).unwrap_or([0.0, 0.0, 1.0]));
                        colors.push(color);
                        next
                    });
                    face.push(id);
                }
                for k in 1..face.len().saturating_sub(1) {
                    out.triangles.push([face[0], face[k], face[k + 1]]);
                }
                if out.triangles.len() > MAX_TRIANGLES {
                    return Err("The model is too large.".into());
                }
            }
            _ => {}
        }
    }
    if !any_normal {
        out.smooth_normals();
    }
    out.colors = any_color.then_some(colors);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quads_negative_indices_normals_and_materials() {
        let obj =
            b"mtllib cube.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvn 0 0 1\nusemtl Red\nf -4//1 -3//1 -2//1 -1//1\n";
        let m = parse(obj, |name| (name == "cube.mtl").then(|| b"newmtl Red\nKd 1 0 0\n".to_vec())).unwrap();
        assert_eq!(m.triangles.len(), 2);
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.normals[0], [0.0, 0.0, 1.0]);
        assert_eq!(m.colors.as_ref().unwrap()[0], [255, 0, 0]);
    }

    #[test]
    fn vertex_colours_and_computed_normals() {
        let obj = b"v 0 0 0 0 1 0\nv 1 0 0 0 1 0\nv 0 1 0 0 1 0\nf 1 2 3\n";
        let m = parse(obj, |_| None).unwrap();
        assert_eq!(m.colors.unwrap()[2], [0, 255, 0]);
        assert_eq!(m.normals[0], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn bad_indices_are_errors() {
        assert!(parse(b"v 0 0 0\nf 1 2 3\n", |_| None).is_err());
        assert!(parse(b"v 0 0\n", |_| None).is_err());
    }
}
