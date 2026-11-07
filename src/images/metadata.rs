//! EXIF: reading for the Inspector, location (shown only on request), and rewriting
//! for saved files (orientation reset, optional location removal, no thumbnail or
//! maker note, whose internal offsets would not survive relocation).

use std::io::Cursor;

use exif::{Exif, Field, In, Reader, Tag, Value};

fn parse(raw: &[u8]) -> Option<Exif> {
    Reader::new().read_raw(raw.to_vec()).ok()
}

/// Strips an "Exif\0\0" prefix and checks the data parses.
pub fn normalise_exif(raw: &[u8]) -> Option<Vec<u8>> {
    let raw = raw.strip_prefix(b"Exif\0\0").unwrap_or(raw);
    parse(raw).map(|_| raw.to_vec())
}

/// EXIF from a TIFF or HEIF container, compacted to the metadata fields (a TIFF's
/// "EXIF" is the whole file otherwise).
pub fn exif_from_container(bytes: &[u8]) -> Option<Vec<u8>> {
    let exif = Reader::new().read_from_container(&mut Cursor::new(bytes)).ok()?;
    write_fields(&exif, false, false)
}

pub fn orientation(raw: &[u8]) -> Option<u16> {
    let e = parse(raw)?;
    e.get_field(Tag::Orientation, In::PRIMARY)?.value.get_uint(0).map(|v| v as u16).filter(|v| (1..=8).contains(v))
}

/// XResolution/YResolution in pixels per inch.
pub fn resolution(raw: &[u8]) -> Option<(f32, f32)> {
    let e = parse(raw)?;
    let r = |t| match &e.get_field(t, In::PRIMARY)?.value {
        Value::Rational(v) if !v.is_empty() && v[0].denom != 0 => Some(v[0].to_f64() as f32),
        _ => None,
    };
    let unit = e.get_field(Tag::ResolutionUnit, In::PRIMARY).and_then(|f| f.value.get_uint(0)).unwrap_or(2);
    let (x, y) = (r(Tag::XResolution)?, r(Tag::YResolution)?);
    if x <= 0.0 || y <= 0.0 {
        return None;
    }
    Some(if unit == 3 { (x * 2.54, y * 2.54) } else { (x, y) })
}

/// Latitude and longitude in degrees, if the image carries a location.
pub fn location(raw: &[u8]) -> Option<(f64, f64)> {
    let e = parse(raw)?;
    let deg = |t: Tag| match &e.get_field(t, In::PRIMARY)?.value {
        Value::Rational(v) if v.len() >= 3 && v.iter().all(|r| r.denom != 0) => {
            Some(v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0)
        }
        _ => None,
    };
    let reference = |t: Tag| match &e.get_field(t, In::PRIMARY)?.value {
        Value::Ascii(v) => v.first().and_then(|s| s.first().copied()),
        _ => None,
    };
    let mut lat = deg(Tag::GPSLatitude)?;
    let mut lon = deg(Tag::GPSLongitude)?;
    if reference(Tag::GPSLatitudeRef) == Some(b'S') {
        lat = -lat;
    }
    if reference(Tag::GPSLongitudeRef) == Some(b'W') {
        lon = -lon;
    }
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
}

pub fn has_location(raw: &[u8]) -> bool {
    parse(raw).is_some_and(|e| e.fields().any(|f| f.tag.context() == exif::Context::Gps))
}

/// Formats a location as "51.50074° N, 0.12457° W".
pub fn format_location((lat, lon): (f64, f64)) -> String {
    format!(
        "{:.5}° {}, {:.5}° {}",
        lat.abs(),
        if lat < 0.0 { 'S' } else { 'N' },
        lon.abs(),
        if lon < 0.0 { 'W' } else { 'E' }
    )
}

/// Inspector rows: (label, value) for the fields people look for, in a fixed order.
pub fn summary(raw: &[u8]) -> Vec<(String, String)> {
    let Some(e) = parse(raw) else { return Vec::new() };
    const ROWS: &[(Tag, &str)] = &[
        (Tag::Make, "Camera make"),
        (Tag::Model, "Camera model"),
        (Tag::LensModel, "Lens"),
        (Tag::DateTimeOriginal, "Date taken"),
        (Tag::ExposureTime, "Exposure"),
        (Tag::FNumber, "Aperture"),
        (Tag::PhotographicSensitivity, "ISO"),
        (Tag::FocalLength, "Focal length"),
        (Tag::FocalLengthIn35mmFilm, "Focal length (35 mm)"),
        (Tag::ExposureBiasValue, "Exposure bias"),
        (Tag::Flash, "Flash"),
        (Tag::WhiteBalance, "White balance"),
        (Tag::Software, "Software"),
        (Tag::Artist, "Artist"),
        (Tag::Copyright, "Copyright"),
        (Tag::ImageDescription, "Description"),
    ];
    ROWS.iter()
        .filter_map(|(tag, label)| {
            let f = e.get_field(*tag, In::PRIMARY)?;
            let v = f.display_value().with_unit(&e).to_string();
            let v = v.trim().trim_matches('"').trim().to_string();
            (!v.is_empty()).then(|| (label.to_string(), v))
        })
        .collect()
}

/// IFD0 tags worth keeping: descriptive ones. Structural tags (sizes, compression,
/// strips) describe the old encoding and are dropped.
const KEEP_PRIMARY: &[Tag] = &[
    Tag::ImageDescription,
    Tag::Make,
    Tag::Model,
    Tag::XResolution,
    Tag::YResolution,
    Tag::ResolutionUnit,
    Tag::Software,
    Tag::DateTime,
    Tag::Artist,
    Tag::Copyright,
];

/// EXIF for a saved file: orientation 1 (pixels are already upright), no thumbnail
/// or maker note, and no location if `strip_location`.
pub fn rewrite(raw: &[u8], strip_location: bool) -> Option<Vec<u8>> {
    let e = parse(raw)?;
    write_fields(&e, strip_location, true)
}

fn write_fields(e: &Exif, strip_location: bool, upright: bool) -> Option<Vec<u8>> {
    let orientation = Field { tag: Tag::Orientation, ifd_num: In::PRIMARY, value: Value::Short(vec![1]) };
    let original_orientation = e.get_field(Tag::Orientation, In::PRIMARY);
    let mut w = exif::experimental::Writer::new();
    let mut any = false;
    for f in e.fields() {
        if f.ifd_num != In::PRIMARY || f.tag == Tag::MakerNote || f.tag == Tag::Orientation {
            continue;
        }
        let keep = match f.tag.context() {
            exif::Context::Tiff => KEEP_PRIMARY.contains(&f.tag),
            exif::Context::Gps => !strip_location,
            _ => true,
        };
        // Values the writer cannot encode are skipped rather than failing the save.
        if keep && !matches!(f.value, Value::Unknown(..)) {
            w.push_field(f);
            any = true;
        }
    }
    match (upright, original_orientation) {
        (true, _) => w.push_field(&orientation),
        (false, Some(o)) => w.push_field(o),
        _ => {}
    }
    if !any {
        return None;
    }
    let mut out = Cursor::new(Vec::new());
    w.write(&mut out, e.little_endian()).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use exif::Rational;

    /// A small EXIF block with camera, orientation and location fields.
    pub fn sample_exif(orientation: u16) -> Vec<u8> {
        let fields = [
            Field { tag: Tag::Make, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"Paper Cam".to_vec()]) },
            Field { tag: Tag::Model, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"PM-1".to_vec()]) },
            Field { tag: Tag::Orientation, ifd_num: In::PRIMARY, value: Value::Short(vec![orientation]) },
            Field {
                tag: Tag::XResolution,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![Rational { num: 300, denom: 1 }]),
            },
            Field {
                tag: Tag::YResolution,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![Rational { num: 300, denom: 1 }]),
            },
            Field { tag: Tag::ResolutionUnit, ifd_num: In::PRIMARY, value: Value::Short(vec![2]) },
            Field {
                tag: Tag::ExposureTime,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![Rational { num: 1, denom: 125 }]),
            },
            Field { tag: Tag::GPSLatitudeRef, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"S".to_vec()]) },
            Field {
                tag: Tag::GPSLatitude,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![
                    Rational { num: 33, denom: 1 },
                    Rational { num: 51, denom: 1 },
                    Rational { num: 3600, denom: 100 },
                ]),
            },
            Field { tag: Tag::GPSLongitudeRef, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"E".to_vec()]) },
            Field {
                tag: Tag::GPSLongitude,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![
                    Rational { num: 151, denom: 1 },
                    Rational { num: 12, denom: 1 },
                    Rational { num: 0, denom: 1 },
                ]),
            },
        ];
        let mut w = exif::experimental::Writer::new();
        for f in &fields {
            w.push_field(f);
        }
        let mut out = Cursor::new(Vec::new());
        w.write(&mut out, true).unwrap();
        out.into_inner()
    }

    #[test]
    fn reads_summary_orientation_resolution_and_location() {
        let raw = sample_exif(6);
        assert_eq!(orientation(&raw), Some(6));
        assert_eq!(resolution(&raw), Some((300.0, 300.0)));
        let (lat, lon) = location(&raw).unwrap();
        assert!((lat + 33.86).abs() < 1e-6 && (lon - 151.2).abs() < 1e-6, "{lat} {lon}");
        assert_eq!(format_location((lat, lon)), "33.86000° S, 151.20000° E");
        let rows = summary(&raw);
        assert!(rows.iter().any(|(k, v)| k == "Camera make" && v == "Paper Cam"), "{rows:?}");
        assert!(rows.iter().any(|(k, v)| k == "Exposure" && v.contains("1/125")), "{rows:?}");
    }

    #[test]
    fn rewrite_resets_orientation_and_strips_location_on_request() {
        let raw = sample_exif(6);
        let kept = rewrite(&raw, false).unwrap();
        assert_eq!(orientation(&kept), Some(1));
        assert!(has_location(&kept));
        let stripped = rewrite(&raw, true).unwrap();
        assert!(!has_location(&stripped));
        assert!(location(&stripped).is_none());
        assert!(summary(&stripped).iter().any(|(k, _)| k == "Camera model"));
        assert_eq!(normalise_exif(&[b"Exif\0\0".as_slice(), &raw].concat()), Some(raw.clone()));
        assert!(normalise_exif(b"garbage").is_none());
    }
}
