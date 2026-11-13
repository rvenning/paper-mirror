//! Windows integration: process metrics, dialogs and shell services. Everything that
//! calls Win32 directly lives under this module.

pub mod assoc;
pub mod clipboard;
pub mod dpapi;
pub mod memory;
pub mod print_dialog;
pub mod random;
pub mod share;
pub mod shell;
pub mod task_dialog;
pub mod touch;
pub mod wic;

/// A NUL-terminated UTF-16 string for Win32.
pub fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A NUL-terminated UTF-16 path for Win32.
pub fn wide_path(p: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}
