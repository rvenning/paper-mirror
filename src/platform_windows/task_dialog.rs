//! Message boxes with labelled buttons ("Save", "Don’t Save", "Cancel"), as Windows'
//! own apps show them.
//!
//! `TaskDialogIndirect` exists only in Common Controls 6, which a program gets through
//! its manifest. Importing it directly (as rfd's `common-controls-v6` feature does)
//! stops every program without that manifest from starting, which includes the test
//! programs and examples. So it is looked up at run time, and callers fall back to a
//! plain Yes/No/Cancel box when it is missing.

use windows_sys::Win32::Foundation::{HWND, S_OK};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TD_WARNING_ICON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_POSITION_RELATIVE_TO_WINDOW,
};

use super::wide_str;

type TaskDialogIndirectFn = unsafe extern "system" fn(
    *const TASKDIALOGCONFIG,
    *mut i32,
    *mut i32,
    *mut windows_sys::core::BOOL,
) -> windows_sys::core::HRESULT;

const FIRST_ID: i32 = 100;
const IDCANCEL: i32 = 2;

fn task_dialog_indirect() -> Option<TaskDialogIndirectFn> {
    let dll = wide_str("comctl32.dll");
    unsafe {
        let module = LoadLibraryW(dll.as_ptr());
        if module.is_null() {
            return None;
        }
        let f = GetProcAddress(module, c"TaskDialogIndirect".as_ptr().cast())?;
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, TaskDialogIndirectFn>(f))
    }
}

/// Whether labelled-button dialogs are available in this process.
pub fn available() -> bool {
    task_dialog_indirect().is_some()
}

/// Shows a warning dialog over `parent` with `buttons` (the first is the default) and
/// returns the index of the one pressed. Escape or the close box picks `cancel`.
/// None when task dialogs aren't available here.
pub fn choose(parent: isize, title: &str, text: &str, buttons: &[&str], cancel: usize) -> Option<usize> {
    let show = task_dialog_indirect()?;
    let title = wide_str(title);
    let text = wide_str(text);
    let labels: Vec<Vec<u16>> = buttons.iter().map(|b| wide_str(b)).collect();
    let specs: Vec<TASKDIALOG_BUTTON> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| TASKDIALOG_BUTTON { nButtonID: FIRST_ID + i as i32, pszButtonText: l.as_ptr() })
        .collect();
    let mut config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: parent as HWND,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW,
        pszWindowTitle: title.as_ptr(),
        pszContent: text.as_ptr(),
        cButtons: specs.len() as u32,
        pButtons: specs.as_ptr(),
        nDefaultButton: FIRST_ID,
        ..Default::default()
    };
    config.Anonymous1.pszMainIcon = TD_WARNING_ICON;
    let mut pressed = 0;
    let hr = unsafe { show(&config, &mut pressed, std::ptr::null_mut(), std::ptr::null_mut()) };
    if hr != S_OK {
        return None;
    }
    Some(match pressed {
        IDCANCEL => cancel,
        id if (FIRST_ID..FIRST_ID + buttons.len() as i32).contains(&id) => (id - FIRST_ID) as usize,
        _ => cancel,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_without_the_common_controls_6_manifest() {
        // Test programs carry no manifest, so they load Common Controls 5, which has no
        // task dialogs; callers must then fall back (and the program must still start).
        assert!(!super::available());
    }
}
