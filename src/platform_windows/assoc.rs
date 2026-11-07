//! Opt-in, per-user file-type registration (no administrator rights). Paper Mirror
//! adds itself to "Open with" and to Settings > Default apps; Windows lets only the
//! user choose the default, so nothing is taken over. Everything lives under
//! HKEY_CURRENT_USER\<base> (base = "Software"; tests use a scratch key).

use std::path::Path;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_NONE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_ANY, RegCloseKey,
    RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};

pub const BASE: &str = "Software";
pub const APP_NAME: &str = "Paper Mirror";
const PROGID_PDF: &str = "PaperMirror.PDF";
const PROGID_IMAGE: &str = "PaperMirror.Image";
const EXE_NAME: &str = "paper-mirror.exe";

pub const PDF_EXTS: &[&str] = &["pdf"];
pub const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "gif", "tif", "tiff", "bmp", "webp", "ico", "heic", "heif", "jp2", "j2k", "jpf",
    "jpx", "exr",
];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

fn create(path: &str) -> Result<Key, String> {
    let mut h: HKEY = std::ptr::null_mut();
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut h,
            std::ptr::null_mut(),
        )
    };
    if r == ERROR_SUCCESS { Ok(Key(h)) } else { Err(format!("Could not write the registry key {path} (error {r}).")) }
}

/// Sets a string value (`name` None = the key's default value).
pub fn set_string(path: &str, name: Option<&str>, value: &str) -> Result<(), String> {
    let key = create(path)?;
    let data = wide(value);
    let name_w = name.map(wide);
    let r = unsafe {
        RegSetValueExW(
            key.0,
            name_w.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    if r == ERROR_SUCCESS { Ok(()) } else { Err(format!("Could not write {path} (error {r}).")) }
}

/// Sets a DWORD value.
pub fn set_dword(path: &str, name: &str, value: u32) -> Result<(), String> {
    let key = create(path)?;
    let r = unsafe {
        RegSetValueExW(
            key.0,
            wide(name).as_ptr(),
            0,
            windows_sys::Win32::System::Registry::REG_DWORD,
            (&value as *const u32).cast(),
            4,
        )
    };
    if r == ERROR_SUCCESS { Ok(()) } else { Err(format!("Could not write {path} (error {r}).")) }
}

fn set_empty(path: &str, name: &str) -> Result<(), String> {
    let key = create(path)?;
    let r = unsafe { RegSetValueExW(key.0, wide(name).as_ptr(), 0, REG_NONE, std::ptr::null(), 0) };
    if r == ERROR_SUCCESS { Ok(()) } else { Err(format!("Could not write {path} (error {r}).")) }
}

/// Reads a string value, if present.
pub fn get_string(path: &str, name: Option<&str>) -> Option<String> {
    let name_w = name.map(wide);
    let name_ptr = name_w.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let mut size = 0u32;
    let p = wide(path);
    unsafe {
        if RegGetValueW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            name_ptr,
            RRF_RT_ANY,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        if RegGetValueW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            name_ptr,
            RRF_RT_ANY,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

pub fn key_exists(path: &str) -> bool {
    let mut h: HKEY = std::ptr::null_mut();
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, KEY_READ, &mut h) };
    if r == ERROR_SUCCESS {
        drop(Key(h));
        true
    } else {
        false
    }
}

/// Deletes a key and everything under it (missing keys are fine).
pub fn delete_tree(path: &str) {
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(path).as_ptr()) };
    unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, wide(path).as_ptr()) };
}

fn delete_value(path: &str, name: &str) {
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(path).as_ptr(), wide(name).as_ptr()) };
}

fn capabilities(base: &str) -> String {
    format!(r"{base}\PaperMirror\Capabilities")
}

/// Registers Paper Mirror for PDFs and images. `exe` is the installed program.
pub fn register(base: &str, exe: &Path) -> Result<(), String> {
    let exe = exe.display().to_string();
    let open = format!("\"{exe}\" \"%1\"");
    let classes = format!(r"{base}\Classes");
    for (progid, name, icon) in [(PROGID_PDF, "PDF Document", 0), (PROGID_IMAGE, "Image", 0)] {
        set_string(&format!(r"{classes}\{progid}"), None, name)?;
        set_string(&format!(r"{classes}\{progid}"), Some("FriendlyTypeName"), name)?;
        set_string(&format!(r"{classes}\{progid}\DefaultIcon"), None, &format!("\"{exe}\",{icon}"))?;
        set_string(&format!(r"{classes}\{progid}\shell\open\command"), None, &open)?;
    }
    let app = format!(r"{classes}\Applications\{EXE_NAME}");
    set_string(&app, Some("FriendlyAppName"), APP_NAME)?;
    set_string(&format!(r"{app}\shell\open\command"), None, &open)?;
    let caps = capabilities(base);
    set_string(&caps, Some("ApplicationName"), APP_NAME)?;
    set_string(&caps, Some("ApplicationDescription"), "View and mark up PDF documents and images.")?;
    for (exts, progid) in [(PDF_EXTS, PROGID_PDF), (IMAGE_EXTS, PROGID_IMAGE)] {
        for ext in exts {
            set_empty(&format!(r"{classes}\.{ext}\OpenWithProgids"), progid)?;
            set_string(&format!(r"{app}\SupportedTypes"), Some(&format!(".{ext}")), "")?;
            set_string(&format!(r"{caps}\FileAssociations"), Some(&format!(".{ext}")), progid)?;
        }
    }
    set_string(&format!(r"{base}\RegisteredApplications"), Some(APP_NAME), &caps)?;
    notify();
    Ok(())
}

/// Removes everything `register` added; other programs' entries are untouched.
pub fn unregister(base: &str) {
    let classes = format!(r"{base}\Classes");
    for progid in [PROGID_PDF, PROGID_IMAGE] {
        delete_tree(&format!(r"{classes}\{progid}"));
    }
    for (exts, progid) in [(PDF_EXTS, PROGID_PDF), (IMAGE_EXTS, PROGID_IMAGE)] {
        for ext in exts {
            delete_value(&format!(r"{classes}\.{ext}\OpenWithProgids"), progid);
        }
    }
    delete_tree(&format!(r"{classes}\Applications\{EXE_NAME}"));
    delete_tree(&format!(r"{base}\PaperMirror"));
    delete_value(&format!(r"{base}\RegisteredApplications"), APP_NAME);
    notify();
}

pub fn is_registered(base: &str) -> bool {
    get_string(&format!(r"{base}\RegisteredApplications"), Some(APP_NAME)).is_some()
}

fn notify() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_unregister_under_a_scratch_key() {
        let base = format!(r"Software\PaperMirrorTest{}", std::process::id());
        let exe = Path::new(r"C:\Test Folder\paper-mirror.exe");
        register(&base, exe).unwrap();
        assert!(is_registered(&base));
        assert_eq!(
            get_string(&format!(r"{base}\Classes\PaperMirror.PDF\shell\open\command"), None).as_deref(),
            Some(r#""C:\Test Folder\paper-mirror.exe" "%1""#)
        );
        assert!(key_exists(&format!(r"{base}\Classes\.png\OpenWithProgids")));
        assert_eq!(
            get_string(&format!(r"{base}\PaperMirror\Capabilities\FileAssociations"), Some(".heic")).as_deref(),
            Some("PaperMirror.Image")
        );
        // Another program's Open-with entry survives unregistering.
        set_empty(&format!(r"{base}\Classes\.png\OpenWithProgids"), "Other.Viewer").unwrap();
        unregister(&base);
        assert!(!is_registered(&base));
        assert!(!key_exists(&format!(r"{base}\Classes\PaperMirror.PDF")));
        assert!(!key_exists(&format!(r"{base}\PaperMirror")));
        assert!(key_exists(&format!(r"{base}\Classes\.png\OpenWithProgids")));
        delete_tree(&base);
        assert!(!key_exists(&base));
    }
}
