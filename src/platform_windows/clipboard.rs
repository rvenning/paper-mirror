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
    out.extend_from_slice(&((width * height * 4) as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 dpi in pixels per metre
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let row = width as usize * 4;
    for y in (0..height as usize).rev() {
        for px in rgba[y * row..(y + 1) * row].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

pub fn set_image(width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    put(CF_DIB, &rgba_to_dib(width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

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
