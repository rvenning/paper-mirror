//! Touch and pen input for a document window.
//!
//! Windows delivers touch and pen as `WM_POINTER*` messages first. winit handles
//! those itself and turns both into touch events, and Slint lets a touch drag pan a
//! ScrollView. That suits a finger but not a pen, which should draw and select text
//! as a mouse does (the Windows Ink convention: the pen writes, the finger scrolls).
//! winit also places those points through the device's HIMETRIC rectangle, which
//! drops input from devices that report none.
//!
//! So the window is subclassed, and touch and pen pointer messages skip winit and go
//! to `DefWindowProcW`, which "promotes" them to the legacy messages winit also
//! handles:
//! - pen becomes mouse messages, so it draws, selects and clicks like a mouse, and
//!   press-and-hold becomes a right-click, as elsewhere in Windows. (Windows also
//!   makes WM_TOUCH from the pen for touch-registered windows; that is dropped.)
//! - touch becomes `WM_TOUCH` (the window is registered for it), which winit turns
//!   into touch events from plain pixel positions. Windows also makes mouse messages
//!   from touch; those are dropped, or each touch would act twice.
//!
//! Pressure is lost, and nothing in Paper Mirror uses it.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::Pointer::GetPointerType;
use windows_sys::Win32::UI::Input::Touch::{
    CloseTouchInputHandle, GetTouchInputInfo, HTOUCHINPUT, RegisterTouchWindow, TOUCHEVENTF_PEN, TOUCHINPUT,
    TWF_WANTPALM,
};
use windows_sys::Win32::UI::Input::{GetCurrentInputMessageSource, IMDT_TOUCH, INPUT_MESSAGE_SOURCE};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, PT_PEN, PT_TOUCH, WM_MOUSEFIRST, WM_MOUSELAST, WM_NCDESTROY, WM_POINTERDOWN, WM_POINTERUP,
    WM_POINTERUPDATE, WM_TOUCH,
};

const SUBCLASS_ID: usize = 0x504D_5443; // "PMTC"

/// Installs the touch and pen handling on `hwnd` (a top-level window of this thread).
pub fn install(hwnd: isize) {
    unsafe {
        RegisterTouchWindow(hwnd as HWND, TWF_WANTPALM);
        SetWindowSubclass(hwnd as HWND, Some(proc_), SUBCLASS_ID, 0);
    }
}

/// Whether a pointer message with this `wparam` comes from a pen or a touch screen.
fn is_pen_or_touch(wparam: WPARAM) -> bool {
    let id = (wparam & 0xFFFF) as u32;
    let mut kind = 0;
    unsafe { GetPointerType(id, &mut kind) != 0 && (kind == PT_PEN || kind == PT_TOUCH) }
}

/// Whether the mouse message being handled was made from touch input. (The extra-info
/// signature 0xFF5157xx with bit 0x80 is documented for this, but synthetic pens set
/// that bit too, so the input source is asked instead.)
fn mouse_from_touch() -> bool {
    let mut src = INPUT_MESSAGE_SOURCE { deviceType: 0, originId: 0 };
    unsafe { GetCurrentInputMessageSource(&mut src) != 0 && src.deviceType & IMDT_TOUCH != 0 }
}

/// Whether every contact in a WM_TOUCH message comes from a pen.
fn touch_is_all_pen(wparam: WPARAM, lparam: LPARAM) -> bool {
    let n = wparam & 0xFFFF;
    let mut inputs: Vec<TOUCHINPUT> = Vec::with_capacity(n);
    let size = size_of::<TOUCHINPUT>() as i32;
    if n == 0 || unsafe { GetTouchInputInfo(lparam as HTOUCHINPUT, n as u32, inputs.as_mut_ptr(), size) } == 0 {
        return false;
    }
    unsafe { inputs.set_len(n) };
    inputs.iter().all(|t| t.dwFlags & TOUCHEVENTF_PEN != 0)
}

unsafe extern "system" fn proc_(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP if is_pen_or_touch(wparam) => unsafe {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        // Touch already arrives as WM_TOUCH; its mouse copies would act a second time.
        WM_MOUSEFIRST..=WM_MOUSELAST if mouse_from_touch() => 0,
        // Registered for touch, the window also gets WM_TOUCH made from the pen; the pen
        // already arrives as the mouse.
        WM_TOUCH if touch_is_all_pen(wparam, lparam) => unsafe {
            CloseTouchInputHandle(lparam as HTOUCHINPUT);
            0
        },
        WM_NCDESTROY => unsafe {
            RemoveWindowSubclass(hwnd, Some(proc_), SUBCLASS_ID);
            DefSubclassProc(hwnd, msg, wparam, lparam)
        },
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_pointer_ids_are_neither_pen_nor_touch() {
        // Ids with no pointer behind them (here, no pen is down) are neither.
        assert!(!super::is_pen_or_touch(1));
        assert!(!super::is_pen_or_touch(0xFFFF));
    }
}
