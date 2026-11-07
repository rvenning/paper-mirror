//! Shell integration: Explorer, bundled documents.

use std::os::windows::process::CommandExt;
use std::path::Path;

/// Opens File Explorer with `path` selected.
pub fn reveal_in_explorer(path: &Path) {
    // explorer.exe needs `/select,"<path>"` as one raw argument.
    let arg = format!("/select,\"{}\"", path.display());
    let _ = std::process::Command::new("explorer.exe").raw_arg(arg).spawn();
}

/// Shows THIRD-PARTY-NOTICES.txt from the installation folder in Notepad.
pub fn show_notices() {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let path = exe_dir.map(|d| d.join("THIRD-PARTY-NOTICES.txt"));
    match path.filter(|p| p.is_file()) {
        Some(p) => {
            let _ = std::process::Command::new("notepad.exe").arg(p).spawn();
        }
        None => {
            rfd::MessageDialog::new()
                .set_title("Third-party notices")
                .set_description("THIRD-PARTY-NOTICES.txt is missing from the installation folder.")
                .show();
        }
    }
}

/// Only web and e-mail links may be handed to the shell from document content;
/// `file:`, `javascript:` and custom protocol handlers are refused.
pub fn is_safe_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
        && !url.chars().any(|c| c.is_control())
}

/// Opens a URL with the user's default handler.
pub fn open_url(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !is_safe_url(url) {
        return;
    }
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
}

/// Opens Settings > Default apps at this app's page (Windows 11), or the general
/// Default apps page.
pub fn open_default_apps(app_name: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let encoded: String = app_name.chars().map(|c| if c == ' ' { "%20".to_string() } else { c.to_string() }).collect();
    let uri = format!("ms-settings:defaultapps?registeredAppUser={encoded}");
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let target: Vec<u16> = uri.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::is_safe_url;

    #[test]
    fn url_allow_list() {
        assert!(is_safe_url("https://example.com/a?b=c"));
        assert!(is_safe_url("HTTP://EXAMPLE.COM"));
        assert!(is_safe_url("mailto:someone@example.com"));
        assert!(!is_safe_url("file:///C:/Windows/system32/calc.exe"));
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("ms-settings:"));
        assert!(!is_safe_url(r"\\server\share\x.exe"));
        assert!(!is_safe_url("https://example.com/\u{0}x"));
    }
}
