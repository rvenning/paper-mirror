//! STL, binary and ASCII. Stored facet normals are ignored (often zero or wrong); flat
//! normals come from the vertex order.

use super::{MAX_TRIANGLES, Mesh};

pub fn parse(bytes: &[u8]) -> Result<Mesh, String> {
    let binary_len = |n: usize| 84usize.checked_add(n.checked_mul(50)?);
    if bytes.len() >= 84 {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
        let exact = binary_len(n) == Some(bytes.len());
        let ascii = bytes.trim_ascii_start().starts_with(b"solid") && looks_ascii(bytes);
        if exact || (!ascii && binary_len(n).is_some_and(|l| l <= bytes.len())) {
            return parse_binary(bytes, n);
        }
    }
    parse_ascii(bytes)
}

/// ASCII STL files start with "solid" but so do some binary ones; check the content.
fn looks_ascii(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    head.iter().all(|b| b.is_ascii()) && head.windows(5).any(|w| w.eq_ignore_ascii_case(b"facet"))
}

fn parse_binary(bytes: &[u8], n: usize) -> Result<Mesh, String> {
    if n == 0 {
        return Err("The STL file contains no triangles.".into());
    }
    if n > MAX_TRIANGLES {
        return Err(format!("The model is too large ({n} triangles)."));
    }
    let f = |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let mut corners = Vec::with_capacity(n * 3);
    for i in 0..n {
        let base = 84 + i * 50 + 12;
        for v in 0..3 {
            let o = base + v * 12;
            corners.push([f(o), f(o + 4), f(o + 8)]);
        }
    }
    Ok(Mesh::flat(corners, None))
}

fn parse_ascii(bytes: &[u8]) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut corners = Vec::new();
    let mut tokens = text.split_ascii_whitespace();
    while let Some(t) = tokens.next() {
        if t.eq_ignore_ascii_case("vertex") {
            let mut v = [0f32; 3];
            for c in &mut v {
                *c = tokens.next().and_then(|s| s.parse::<f32>().ok()).ok_or("The STL file has a malformed vertex.")?;
            }
            corners.push(v);
            if corners.len() > MAX_TRIANGLES * 3 {
                return Err("The model is too large.".into());
            }
        }
    }
    if corners.len() < 3 {
        return Err("The STL file contains no triangles.".into());
    }
    corners.truncate(corners.len() / 3 * 3);
    Ok(Mesh::flat(corners, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_binary_round_trip() {
        let ascii = b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        let m = parse(ascii).unwrap();
        assert_eq!(m.triangles.len(), 1);
        assert_eq!(m.normals[0], [0.0, 0.0, 1.0]);
        let bin = crate::model3d::export::stl_binary(&m);
        let back = parse(&bin).unwrap();
        assert_eq!(back.positions, m.positions);
    }

    #[test]
    fn binary_starting_with_solid_is_still_binary() {
        let m = Mesh::flat(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], None);
        let mut bin = crate::model3d::export::stl_binary(&m);
        bin[..5].copy_from_slice(b"solid");
        assert_eq!(parse(&bin).unwrap().triangles.len(), 1);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse(b"solid nothing here").is_err());
        assert!(parse(b"vertex 1 2").is_err());
        let mut huge = vec![0u8; 84];
        huge[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&huge).is_err());
    }
}
