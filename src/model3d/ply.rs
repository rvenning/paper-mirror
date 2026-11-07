//! PLY (Stanford), ASCII and binary little/big endian. Reads vertex positions,
//! normals and colours and face index lists (fan-triangulated); other elements and
//! properties are skipped.

use super::{MAX_TRIANGLES, MAX_VERTICES, Mesh};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Ty {
    fn parse(s: &str) -> Option<Ty> {
        Some(match s {
            "char" | "int8" => Ty::I8,
            "uchar" | "uint8" => Ty::U8,
            "short" | "int16" => Ty::I16,
            "ushort" | "uint16" => Ty::U16,
            "int" | "int32" => Ty::I32,
            "uint" | "uint32" => Ty::U32,
            "float" | "float32" => Ty::F32,
            "double" | "float64" => Ty::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32 | Ty::U32 | Ty::F32 => 4,
            Ty::F64 => 8,
        }
    }
}

#[derive(Clone, Debug)]
struct Prop {
    name: String,
    ty: Ty,
    /// Count type for list properties.
    list: Option<Ty>,
}

#[derive(Clone, Debug)]
struct Element {
    name: String,
    count: usize,
    props: Vec<Prop>,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Ascii,
    Le,
    Be,
}

/// Reads values one at a time from ASCII tokens or binary data.
struct Reader<'a> {
    format: Format,
    data: &'a [u8],
    pos: usize,
    tokens: std::str::SplitAsciiWhitespace<'a>,
}

impl Reader<'_> {
    fn read(&mut self, ty: Ty) -> Result<f64, String> {
        const BAD: &str = "The PLY file ends early or is damaged.";
        if self.format == Format::Ascii {
            return self.tokens.next().and_then(|t| t.parse::<f64>().ok()).ok_or_else(|| BAD.to_string());
        }
        let n = ty.size();
        let b = self.data.get(self.pos..self.pos + n).ok_or(BAD)?;
        self.pos += n;
        let mut a = [0u8; 8];
        a[..n].copy_from_slice(b);
        if self.format == Format::Be {
            a[..n].reverse();
        }
        Ok(match ty {
            Ty::I8 => a[0] as i8 as f64,
            Ty::U8 => a[0] as f64,
            Ty::I16 => i16::from_le_bytes([a[0], a[1]]) as f64,
            Ty::U16 => u16::from_le_bytes([a[0], a[1]]) as f64,
            Ty::I32 => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::U32 => u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::F32 => f32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::F64 => f64::from_le_bytes(a),
        })
    }
}

pub fn parse(bytes: &[u8]) -> Result<Mesh, String> {
    let end = bytes.windows(10).position(|w| w == b"end_header").ok_or("The PLY file has no header.")?;
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "The PLY header is not text.")?;
    let mut body = end + 10;
    // The header ends with a line break (\n or \r\n).
    if bytes.get(body) == Some(&b'\r') {
        body += 1;
    }
    if bytes.get(body) == Some(&b'\n') {
        body += 1;
    }
    let mut format = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in header.lines() {
        let w: Vec<&str> = line.split_ascii_whitespace().collect();
        match w.as_slice() {
            ["format", f, ..] => {
                format = Some(match *f {
                    "ascii" => Format::Ascii,
                    "binary_little_endian" => Format::Le,
                    "binary_big_endian" => Format::Be,
                    _ => return Err("Unknown PLY format.".into()),
                })
            }
            ["element", name, count] => elements.push(Element {
                name: name.to_string(),
                count: count.parse().map_err(|_| "Bad PLY element count.")?,
                props: Vec::new(),
            }),
            ["property", "list", c, t, name] => elements.last_mut().ok_or("Bad PLY header.")?.props.push(Prop {
                name: name.to_string(),
                ty: Ty::parse(t).ok_or("Unknown PLY type.")?,
                list: Some(Ty::parse(c).ok_or("Unknown PLY type.")?),
            }),
            ["property", t, name] => elements.last_mut().ok_or("Bad PLY header.")?.props.push(Prop {
                name: name.to_string(),
                ty: Ty::parse(t).ok_or("Unknown PLY type.")?,
                list: None,
            }),
            _ => {}
        }
    }
    let format = format.ok_or("The PLY file has no format line.")?;
    let ascii_body = if format == Format::Ascii { std::str::from_utf8(&bytes[body..]).unwrap_or("") } else { "" };
    let mut r = Reader { format, data: bytes, pos: body, tokens: ascii_body.split_ascii_whitespace() };
    let mut m = Mesh::default();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut has_normals = false;
    for el in &elements {
        let is_vertex = el.name == "vertex";
        let is_face = el.name == "face";
        if is_vertex && el.count > MAX_VERTICES || is_face && el.count > MAX_TRIANGLES {
            return Err("The model is too large.".into());
        }
        let idx = |n: &str| el.props.iter().position(|p| p.name == n);
        let (x, y, z) = (idx("x"), idx("y"), idx("z"));
        let normal = (idx("nx"), idx("ny"), idx("nz"));
        let color = (idx("red"), idx("green"), idx("blue"));
        has_normals |= is_vertex && normal.0.is_some() && normal.1.is_some() && normal.2.is_some();
        let has_color = is_vertex && color.0.is_some() && color.1.is_some() && color.2.is_some();
        let mut vals = vec![0f64; el.props.len()];
        for _ in 0..el.count {
            let mut indices: Vec<u32> = Vec::new();
            for (k, p) in el.props.iter().enumerate() {
                match p.list {
                    Some(ct) => {
                        let n = r.read(ct)?;
                        if !(0.0..=1_000_000.0).contains(&n) {
                            return Err("The PLY file is damaged.".into());
                        }
                        let wanted = is_face && (p.name == "vertex_indices" || p.name == "vertex_index");
                        for _ in 0..n as usize {
                            let v = r.read(p.ty)?;
                            if wanted {
                                if v < 0.0 || v > u32::MAX as f64 {
                                    return Err("The PLY file has a bad vertex index.".into());
                                }
                                indices.push(v as u32);
                            }
                        }
                    }
                    None => vals[k] = r.read(p.ty)?,
                }
            }
            if is_vertex {
                let g = |i: Option<usize>| i.map(|i| vals[i] as f32).unwrap_or(0.0);
                m.positions.push([g(x), g(y), g(z)]);
                if has_normals {
                    m.normals.push(super::normalize([g(normal.0), g(normal.1), g(normal.2)]));
                }
                if has_color {
                    let c = |i: Option<usize>| {
                        let p = &el.props[i.unwrap()];
                        let v = vals[i.unwrap()];
                        // Integer colours are 0-255; floating-point ones 0-1.
                        let v = if matches!(p.ty, Ty::F32 | Ty::F64) { v * 255.0 } else { v };
                        v.round().clamp(0.0, 255.0) as u8
                    };
                    colors.push([c(color.0), c(color.1), c(color.2)]);
                }
            }
            if is_face {
                for k in 1..indices.len().saturating_sub(1) {
                    m.triangles.push([indices[0], indices[k], indices[k + 1]]);
                }
                if m.triangles.len() > MAX_TRIANGLES {
                    return Err("The model is too large.".into());
                }
            }
        }
    }
    if !has_normals {
        m.smooth_normals();
    }
    if !colors.is_empty() {
        m.colors = Some(colors);
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_quad_with_colours() {
        let ply = b"ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0 255 0 0\n1 0 0 255 0 0\n1 1 0 255 0 0\n0 1 0 255 0 0\n4 0 1 2 3\n";
        let m = parse(ply).unwrap();
        assert_eq!(m.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        assert_eq!(m.colors.unwrap()[3], [255, 0, 0]);
        assert_eq!(m.normals[0], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn binary_round_trip_and_extra_elements() {
        let cube = crate::model3d::tests::cube();
        let bin = crate::model3d::export::ply_binary(&cube);
        let m = parse(&bin).unwrap();
        assert_eq!(m.triangles, cube.triangles);
        assert_eq!(m.positions, cube.positions);
        // An unknown element before the faces is skipped correctly (big endian too).
        let be = b"ply\r\nformat binary_big_endian 1.0\r\nelement vertex 3\r\nproperty float x\r\nproperty float y\r\nproperty float z\r\nelement extra 1\r\nproperty list uchar short junk\r\nelement face 1\r\nproperty list uchar uint vertex_index\r\nend_header\r\n";
        let mut b = be.to_vec();
        for v in [[0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in v {
                b.extend_from_slice(&c.to_be_bytes());
            }
        }
        b.extend_from_slice(&[2, 0, 7, 0, 9]);
        b.push(3);
        for i in [0u32, 1, 2] {
            b.extend_from_slice(&i.to_be_bytes());
        }
        let m = parse(&b).unwrap();
        assert_eq!(m.triangles, vec![[0, 1, 2]]);
        assert_eq!(m.positions[1], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn truncated_and_malformed() {
        let cube = crate::model3d::tests::cube();
        let bin = crate::model3d::export::ply_binary(&cube);
        assert!(parse(&bin[..bin.len() - 7]).is_err());
        assert!(parse(b"ply\nformat weird 1.0\nend_header\n").is_err());
        assert!(parse(b"no header").is_err());
    }
}
