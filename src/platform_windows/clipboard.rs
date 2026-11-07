//! Windows clipboard: Unicode text and 32-bit DIB images.

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;

/// Opens the clipboard, retrying briefly if another process holds it.
fn open() -> Result<(), String> {
    for _ in 0..10 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    Err("The clipboard is in use by another application.".into())
}

fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
    open()?;
    let result = unsafe {
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if h.is_null() {
            CloseClipboard();
            return Err("Out of memory.".into());
        }
        let p = GlobalLock(h) as *mut u8;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        // On success the system owns the memory.
        if SetClipboardData(format, h as HANDLE).is_null() {
            GlobalFree(h);
            Err("Could not place data on the clipboard.".into())
        } else {
            Ok(())
        }
    };
    unsafe { CloseClipboard() };
    result
}

pub fn set_text(text: &str) -> Result<(), String> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes: Vec<u8> = wide.iter().flat_map(|u| u.to_le_bytes()).collect();
    put(CF_UNICODETEXT, &bytes)
}

pub fn get_text() -> Option<String> {
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
        return None;
    }
    open().ok()?;
    let out = unsafe {
        let h = GetClipboardData(CF_UNICODETEXT);
        if h.is_null() {
            None
        } else {
            let p = GlobalLock(h as _) as *const u16;
            if p.is_null() {
                None
            } else {
                let max = GlobalSize(h as _) / 2;
                let slice = std::slice::from_raw_parts(p, max);
                let end = slice.iter().position(|u| *u == 0).unwrap_or(max);
                let s = String::from_utf16_lossy(&slice[..end]);
                GlobalUnlock(h as _);
                Some(s)
            }
        }
    };
    unsafe { CloseClipboard() };
    out
}

/// Builds a packed DIB (BITMAPINFOHEADER + bottom-up BGRA rows) from RGBA pixels.
pub fn rgba_to_dib(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + rgba.len());
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes()); // positive: bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bit count
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(width * height * 4).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 dpi in pixels per metre
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let row = width as usize * 4;
    for y in (0..height as usize).rev() {
        for px in rgba[y * row..(y + 1) * row].as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

pub fn set_image(width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    put(CF_DIB, &rgba_to_dib(width, height, rgba))
}

/// Decodes a packed DIB (24- or 32-bit, BI_RGB or BI_BITFIELDS) into RGBA.
pub fn dib_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let u32_at = |i: usize| dib.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let header = u32_at(0)? as usize;
    if !(40..=124).contains(&header) {
        return None;
    }
    let width = u32_at(4)? as i32;
    let height = u32_at(8)? as i32;
    let bits = u16::from_le_bytes([*dib.get(14)?, *dib.get(15)?]);
    let compression = u32_at(16)?;
    let used_colours = u32_at(32)? as usize;
    if width <= 0 || height == 0 || !(bits == 24 || bits == 32) || !(compression == 0 || compression == 3) {
        return None;
    }
    let (w, h) = (width as u32, height.unsigned_abs());
    if w > 40_000 || h > 40_000 || w as u64 * h as u64 > 250_000_000 {
        return None;
    }
    let masks_after_header = if compression == 3 && header == 40 { 12 } else { 0 };
    let offset = header + masks_after_header + used_colours * 4;
    let stride = ((w as usize * bits as usize).div_ceil(32)) * 4;
    let data = dib.get(offset..offset + stride * h as usize)?;
    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    let bpp = bits as usize / 8;
    for y in 0..h as usize {
        // Positive height: rows are stored bottom-up.
        let src_row = if height > 0 { h as usize - 1 - y } else { y };
        let line = &data[src_row * stride..src_row * stride + w as usize * bpp];
        for x in 0..w as usize {
            let p = &line[x * bpp..x * bpp + bpp];
            let a = if bpp == 4 { p[3] } else { 255 };
            rgba[(y * w as usize + x) * 4..][..4].copy_from_slice(&[p[2], p[1], p[0], a]);
        }
    }
    // Many programs leave the fourth byte of 32-bit DIBs at zero: that means opaque.
    if bpp == 4 && rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
    }
    Some((w, h, rgba))
}

/// The image on the clipboard, if any (as RGBA).
pub fn get_image() -> Option<(u32, u32, Vec<u8>)> {
    if unsafe { IsClipboardFormatAvailable(CF_DIB) } == 0 {
        return None;
    }
    open().ok()?;
    let bytes = unsafe {
        let h = GetClipboardData(CF_DIB);
        if h.is_null() {
            None
        } else {
            let p = GlobalLock(h as _) as *const u8;
            if p.is_null() {
                None
            } else {
                let v = std::slice::from_raw_parts(p, GlobalSize(h as _)).to_vec();
                GlobalUnlock(h as _);
                Some(v)
            }
        }
    };
    unsafe { CloseClipboard() };
    dib_to_rgba(&bytes?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_round_trip() {
        let rgba = [255, 0, 0, 255, 0, 255, 0, 128, 1, 2, 3, 255, 4, 5, 6, 0];
        assert_eq!(dib_to_rgba(&rgba_to_dib(2, 2, &rgba)), Some((2, 2, rgba.to_vec())));
        assert!(dib_to_rgba(b"short").is_none());
    }

    #[test]
    fn dib_layout() {
        // 2x1 image: red, green.
        let dib = rgba_to_dib(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!(dib.len(), 40 + 8);
        assert_eq!(&dib[40..48], &[0, 0, 255, 255, 0, 255, 0, 255]);
        // 1x2 image is stored bottom-up.
        let dib = rgba_to_dib(1, 2, &[1, 1, 1, 255, 2, 2, 2, 255]);
        assert_eq!(&dib[40..44], &[2, 2, 2, 255]);
    }

    #[test]
    fn text_round_trip() {
        // The clipboard is shared with the user: restore what was there.
        let previous = get_text();
        if set_text("Paper Mirror ✓ clipboard").is_ok() {
            assert_eq!(get_text().as_deref(), Some("Paper Mirror ✓ clipboard"));
        }
        if let Some(p) = previous {
            let _ = set_text(&p);
        }
    }
}
