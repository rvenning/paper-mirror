//! Windows Imaging Component: decoding TIFF (all pages), HEIC/HEIF/AVIF and other
//! formats with system codecs, and encoding HEIC. `windows-sys` has no COM interface
//! definitions, so the few methods used are called through their vtable slots
//! (documented in wincodec.h; slot numbers count the IUnknown methods 0-2).

use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows_sys::core::{GUID, HRESULT};

use crate::images::decode::check_size;
use crate::images::{Decoded, Frame, ImageInfo};

const CLSID_WIC_IMAGING_FACTORY: GUID = GUID::from_u128(0xcacaf262_9370_4615_a13b_9f5539da4c0a);
const IID_IWIC_IMAGING_FACTORY: GUID = GUID::from_u128(0xec5ec8a9_c395_4314_9c77_54d7a935ff70);
const PIXEL_RGBA32: GUID = GUID::from_u128(0xf5c7ad2d_6a8d_43dd_a7a8_a29935261ae9);
const PIXEL_BGRA32: GUID = GUID::from_u128(0x6fddc324_4e03_4bfe_b185_3d77768dc90f);
const PIXEL_BGR24: GUID = GUID::from_u128(0x6fddc324_4e03_4bfe_b185_3d77768dc90c);
const PIXEL_BGR32: GUID = GUID::from_u128(0x6fddc324_4e03_4bfe_b185_3d77768dc90e);
const CONTAINER_HEIF: GUID = GUID::from_u128(0xe1e62521_6787_405b_a339_500715b5763f);

const GENERIC_WRITE: u32 = 0x4000_0000;
const RPC_E_CHANGED_MODE: HRESULT = 0x8001_0106_u32 as i32;
/// Frames beyond this are ignored (a multi-page TIFF fax can have hundreds).
const MAX_FRAMES: u32 = 1000;

// IWICImagingFactory
const FACTORY_CREATE_DECODER_FROM_STREAM: usize = 4;
const FACTORY_CREATE_ENCODER: usize = 8;
const FACTORY_CREATE_FORMAT_CONVERTER: usize = 10;
const FACTORY_CREATE_STREAM: usize = 14;
const FACTORY_CREATE_COLOR_CONTEXT: usize = 15;
// IWICStream (after IStream's 3-13)
const STREAM_INIT_FROM_FILENAME: usize = 15;
const STREAM_INIT_FROM_MEMORY: usize = 16;
// IWICBitmapDecoder
const DECODER_GET_FRAME_COUNT: usize = 12;
const DECODER_GET_FRAME: usize = 13;
// IWICBitmapSource (frames and converters)
const SOURCE_GET_SIZE: usize = 3;
const SOURCE_GET_RESOLUTION: usize = 5;
const SOURCE_COPY_PIXELS: usize = 7;
// IWICBitmapFrameDecode
const FRAME_GET_COLOR_CONTEXTS: usize = 9;
// IWICFormatConverter
const CONVERTER_INITIALIZE: usize = 8;
// IWICColorContext
const COLOR_CONTEXT_GET_TYPE: usize = 6;
const COLOR_CONTEXT_GET_PROFILE_BYTES: usize = 7;
// IWICBitmapEncoder
const ENCODER_INITIALIZE: usize = 3;
const ENCODER_CREATE_NEW_FRAME: usize = 10;
const ENCODER_COMMIT: usize = 11;
// IWICBitmapFrameEncode
const FRAME_ENC_INITIALIZE: usize = 3;
const FRAME_ENC_SET_SIZE: usize = 4;
const FRAME_ENC_SET_RESOLUTION: usize = 5;
const FRAME_ENC_SET_PIXEL_FORMAT: usize = 6;
const FRAME_ENC_WRITE_PIXELS: usize = 10;
const FRAME_ENC_COMMIT: usize = 12;
// IPropertyBag2
const PROPBAG_WRITE: usize = 4;

/// An owned COM interface pointer, released on drop.
struct Com(*mut c_void);

impl Com {
    /// The function in vtable slot `slot`, typed as `F`.
    unsafe fn method<F: Copy>(&self, slot: usize) -> F {
        unsafe {
            let vtbl = *(self.0 as *const *const *const c_void);
            std::mem::transmute_copy(&*vtbl.add(slot))
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let release: unsafe extern "system" fn(*mut c_void) -> u32 = self.method(2);
                release(self.0);
            }
        }
    }
}

fn same(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

fn check(hr: HRESULT, what: &str) -> Result<(), String> {
    if hr < 0 { Err(format!("{what} failed (0x{:08X}).", hr as u32)) } else { Ok(()) }
}

/// COM for the current thread, uninitialised on drop when this call initialised it.
struct ComInit(bool);

impl ComInit {
    fn new() -> ComInit {
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) };
        // S_OK/S_FALSE need a matching uninitialise; a thread already in another
        // apartment (RPC_E_CHANGED_MODE) can still use WIC's free-threaded factory.
        ComInit(hr >= 0 && hr != RPC_E_CHANGED_MODE)
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn factory() -> Result<Com, String> {
    let mut p = null_mut();
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_WIC_IMAGING_FACTORY,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IWIC_IMAGING_FACTORY,
            &mut p,
        )
    };
    check(hr, "Starting Windows Imaging Component")?;
    Ok(Com(p))
}

type CreateOut = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;

fn create(f: &Com, slot: usize, what: &str) -> Result<Com, String> {
    let mut p = null_mut();
    let hr = unsafe { f.method::<CreateOut>(slot)(f.0, &mut p) };
    check(hr, what)?;
    Ok(Com(p))
}

/// Decodes every frame (up to `MAX_FRAMES`) to RGBA, with the first frame's ICC profile.
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    if bytes.len() > u32::MAX as usize {
        return Err("The file is too large.".into());
    }
    let _com = ComInit::new();
    let factory = factory()?;
    let stream = create(&factory, FACTORY_CREATE_STREAM, "Creating a stream")?;
    unsafe {
        let init: unsafe extern "system" fn(*mut c_void, *const u8, u32) -> HRESULT =
            stream.method(STREAM_INIT_FROM_MEMORY);
        check(init(stream.0, bytes.as_ptr(), bytes.len() as u32), "Reading the image")?;
    }
    let decoder = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const GUID, i32, *mut *mut c_void) -> HRESULT =
            factory.method(FACTORY_CREATE_DECODER_FROM_STREAM);
        let mut d = null_mut();
        // WICDecodeMetadataCacheOnDemand = 0.
        check(
            f(factory.0, stream.0, std::ptr::null(), 0, &mut d),
            "No installed Windows codec can read this image; it",
        )?;
        Com(d)
    };
    let mut count = 0u32;
    unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT = decoder.method(DECODER_GET_FRAME_COUNT);
        check(f(decoder.0, &mut count), "Counting frames")?;
    }
    let mut frames = Vec::new();
    let mut info = ImageInfo { bits_per_channel: 8, color_model: "RGB".into(), ..Default::default() };
    let mut total = 0u64;
    for i in 0..count.min(MAX_FRAMES) {
        let frame = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
                decoder.method(DECODER_GET_FRAME);
            let mut p = null_mut();
            check(f(decoder.0, i, &mut p), "Reading a frame")?;
            Com(p)
        };
        let (mut w, mut h) = (0u32, 0u32);
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut u32, *mut u32) -> HRESULT =
                frame.method(SOURCE_GET_SIZE);
            check(f(frame.0, &mut w, &mut h), "Reading the frame size")?;
        }
        check_size(w, h)?;
        total += w as u64 * h as u64;
        if total > crate::images::decode::MAX_TOTAL_PIXELS {
            return Err("The image has too many pixels to open.".into());
        }
        if i == 0 {
            let (mut dx, mut dy) = (0f64, 0f64);
            unsafe {
                let f: unsafe extern "system" fn(*mut c_void, *mut f64, *mut f64) -> HRESULT =
                    frame.method(SOURCE_GET_RESOLUTION);
                if f(frame.0, &mut dx, &mut dy) >= 0 && dx > 1.0 && dy > 1.0 {
                    info.dpi = (dx.round() as f32, dy.round() as f32);
                }
            }
            info.icc = color_profile(&factory, &frame);
        }
        let conv = create(&factory, FACTORY_CREATE_FORMAT_CONVERTER, "Converting pixels")?;
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        unsafe {
            let init: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *const GUID,
                i32,
                *mut c_void,
                f64,
                i32,
            ) -> HRESULT = conv.method(CONVERTER_INITIALIZE);
            check(init(conv.0, frame.0, &PIXEL_RGBA32, 0, null_mut(), 0.0, 0), "Converting pixels")?;
            let copy: unsafe extern "system" fn(*mut c_void, *const c_void, u32, u32, *mut u8) -> HRESULT =
                conv.method(SOURCE_COPY_PIXELS);
            check(copy(conv.0, std::ptr::null(), w * 4, rgba.len() as u32, rgba.as_mut_ptr()), "Decoding pixels")?;
        }
        frames.push(Frame::new(w, h, rgba));
    }
    if frames.is_empty() {
        return Err("The image has no frames.".into());
    }
    Ok(Decoded { frames, info })
}

/// The frame's embedded ICC profile, if any.
fn color_profile(factory: &Com, frame: &Com) -> Option<Vec<u8>> {
    unsafe {
        let get: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut u32) -> HRESULT =
            frame.method(FRAME_GET_COLOR_CONTEXTS);
        let mut n = 0u32;
        if get(frame.0, 0, null_mut(), &mut n) < 0 || n == 0 {
            return None;
        }
        let contexts: Vec<Com> =
            (0..n.min(8)).filter_map(|_| create(factory, FACTORY_CREATE_COLOR_CONTEXT, "colour").ok()).collect();
        let mut raw: Vec<*mut c_void> = contexts.iter().map(|c| c.0).collect();
        let mut actual = 0u32;
        if get(frame.0, raw.len() as u32, raw.as_mut_ptr(), &mut actual) < 0 {
            return None;
        }
        for c in contexts.iter().take(actual as usize) {
            let ty: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT = c.method(COLOR_CONTEXT_GET_TYPE);
            let mut t = 0;
            // WICColorContextProfile = 1 (the other kind is an EXIF colour space).
            if ty(c.0, &mut t) < 0 || t != 1 {
                continue;
            }
            let bytes: unsafe extern "system" fn(*mut c_void, u32, *mut u8, *mut u32) -> HRESULT =
                c.method(COLOR_CONTEXT_GET_PROFILE_BYTES);
            let mut size = 0u32;
            if bytes(c.0, 0, null_mut(), &mut size) < 0 || size == 0 || size > 16 << 20 {
                continue;
            }
            let mut buf = vec![0u8; size as usize];
            if bytes(c.0, size, buf.as_mut_ptr(), &mut size) >= 0 {
                buf.truncate(size as usize);
                return Some(buf);
            }
        }
        None
    }
}

#[repr(C)]
struct PropBag2 {
    dw_type: u32,
    vt: u16,
    cf_type: u16,
    dw_hint: u32,
    name: *const u16,
    clsid: GUID,
}

#[repr(C)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: [u64; 2],
}

/// Encodes one frame as HEIC with the system HEIF encoder (needs the HEIF and HEVC
/// extensions from Microsoft).
pub fn encode_heic(f: &Frame, quality: u8) -> Result<Vec<u8>, String> {
    let _com = ComInit::new();
    let factory = factory()?;
    let encoder = unsafe {
        let ce: unsafe extern "system" fn(*mut c_void, *const GUID, *const GUID, *mut *mut c_void) -> HRESULT =
            factory.method(FACTORY_CREATE_ENCODER);
        let mut p = null_mut();
        if ce(factory.0, &CONTAINER_HEIF, std::ptr::null(), &mut p) < 0 {
            return Err(
                "HEIC can’t be written on this PC. Install the HEIF Image Extensions and HEVC Video Extensions from Microsoft, or choose another format."
                    .into(),
            );
        }
        Com(p)
    };
    let tmp = std::env::temp_dir().join(format!(
        "paper-mirror-{}-{}.heic",
        std::process::id(),
        crate::platform_windows::random::u64().unwrap_or(0)
    ));
    struct Remove(std::path::PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Remove(tmp.clone());
    {
        let stream = create(&factory, FACTORY_CREATE_STREAM, "Creating a stream")?;
        let wide = super::wide_path(&tmp);
        unsafe {
            let init: unsafe extern "system" fn(*mut c_void, *const u16, u32) -> HRESULT =
                stream.method(STREAM_INIT_FROM_FILENAME);
            check(init(stream.0, wide.as_ptr(), GENERIC_WRITE), "Creating the file")?;
            let ei: unsafe extern "system" fn(*mut c_void, *mut c_void, i32) -> HRESULT =
                encoder.method(ENCODER_INITIALIZE);
            // WICBitmapEncoderNoCache = 2.
            check(ei(encoder.0, stream.0, 2), "Starting the encoder")?;
            let nf: unsafe extern "system" fn(*mut c_void, *mut *mut c_void, *mut *mut c_void) -> HRESULT =
                encoder.method(ENCODER_CREATE_NEW_FRAME);
            let (mut fp, mut bag) = (null_mut(), null_mut());
            check(nf(encoder.0, &mut fp, &mut bag), "Creating a frame")?;
            let frame = Com(fp);
            let bag = Com(bag);
            if !bag.0.is_null() {
                let name = super::wide_str("ImageQuality");
                let prop = PropBag2 {
                    dw_type: 0,
                    vt: 4,
                    cf_type: 0,
                    dw_hint: 0,
                    name: name.as_ptr(),
                    clsid: GUID::from_u128(0),
                };
                let q = (quality.clamp(1, 100) as f32 / 100.0).to_bits() as u64;
                let var = Variant { vt: 4, reserved: [0; 3], value: [q, 0] };
                let write: unsafe extern "system" fn(*mut c_void, u32, *const PropBag2, *const Variant) -> HRESULT =
                    bag.method(PROPBAG_WRITE);
                let _ = write(bag.0, 1, &prop, &var);
            }
            let fi: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT = frame.method(FRAME_ENC_INITIALIZE);
            check(fi(frame.0, bag.0), "Starting the frame")?;
            let ss: unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT = frame.method(FRAME_ENC_SET_SIZE);
            check(ss(frame.0, f.width, f.height), "Setting the size")?;
            let sr: unsafe extern "system" fn(*mut c_void, f64, f64) -> HRESULT =
                frame.method(FRAME_ENC_SET_RESOLUTION);
            let _ = sr(frame.0, 72.0, 72.0);
            let mut fmt = PIXEL_BGRA32;
            let sp: unsafe extern "system" fn(*mut c_void, *mut GUID) -> HRESULT =
                frame.method(FRAME_ENC_SET_PIXEL_FORMAT);
            check(sp(frame.0, &mut fmt), "Setting the pixel format")?;
            let (stride, pixels) = if same(&fmt, &PIXEL_BGRA32) {
                (
                    f.width * 4,
                    f.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect::<Vec<u8>>(),
                )
            } else if same(&fmt, &PIXEL_BGR24) {
                let rgb = crate::images::encode::flatten_on_white(f);
                let row = (f.width * 3).div_ceil(4) * 4;
                let mut v = vec![0u8; row as usize * f.height as usize];
                for (y, line) in rgb.chunks_exact(f.width as usize * 3).enumerate() {
                    for (x, p) in line.as_chunks::<3>().0.iter().enumerate() {
                        let i = y * row as usize + x * 3;
                        v[i..i + 3].copy_from_slice(&[p[2], p[1], p[0]]);
                    }
                }
                (row, v)
            } else if same(&fmt, &PIXEL_BGR32) {
                let rgb = crate::images::encode::flatten_on_white(f);
                (f.width * 4, rgb.as_chunks::<3>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect::<Vec<u8>>())
            } else {
                return Err("The HEIC encoder wants an unsupported pixel format.".into());
            };
            let wp: unsafe extern "system" fn(*mut c_void, u32, u32, u32, *const u8) -> HRESULT =
                frame.method(FRAME_ENC_WRITE_PIXELS);
            check(wp(frame.0, f.height, stride, pixels.len() as u32, pixels.as_ptr()), "Writing pixels")?;
            let fc: unsafe extern "system" fn(*mut c_void) -> HRESULT = frame.method(FRAME_ENC_COMMIT);
            check(fc(frame.0), "Finishing the frame")?;
            let ec: unsafe extern "system" fn(*mut c_void) -> HRESULT = encoder.method(ENCODER_COMMIT);
            check(ec(encoder.0), "Finishing the file")?;
        }
    }
    drop(encoder);
    std::fs::read(&tmp).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_png_through_wic() {
        let img = image::RgbaImage::from_fn(5, 4, |x, y| image::Rgba([x as u8 * 50, y as u8 * 60, 9, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let d = decode(&png).unwrap();
        assert_eq!(d.frames.len(), 1);
        assert_eq!(d.frames[0].pixel(3, 2), [150, 120, 9, 255]);
        assert!(decode(b"definitely not an image").is_err());
    }

    #[test]
    fn heic_round_trip_when_codecs_are_installed() {
        let mut f = Frame::new(64, 48, vec![0; 64 * 48 * 4]);
        for (i, p) in f.rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            p.copy_from_slice(&[(i % 64) as u8 * 4, 128, 200, 255]);
        }
        let bytes = encode_heic(&f, 95);
        let Ok(bytes) = bytes else {
            eprintln!("HEIF encoder not installed; skipping: {}", bytes.unwrap_err());
            return;
        };
        assert_eq!(&bytes[4..8], b"ftyp");
        let d = decode(&bytes).unwrap();
        assert_eq!((d.frames[0].width, d.frames[0].height), (64, 48));
        let p = d.frames[0].pixel(32, 24);
        assert!(p[1].abs_diff(128) < 12 && p[2].abs_diff(200) < 12, "{p:?}");
    }
}
