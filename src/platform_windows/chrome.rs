//! Window chrome in Paper Mirror's own theme: the title bar and the native menu bar
//! take the toolbar's colours, so the top of the window reads as one surface (as in
//! Windows 11's own apps), in light and dark alike.
//!
//! - Title bar: DWM attributes (dark mode, caption and caption-text colours; Windows 11
//!   only, ignored by Windows 10).
//! - Menu bar: Windows asks a window to paint its menu bar through the undocumented
//!   `WM_UAHDRAWMENU` / `WM_UAHDRAWMENUITEM` messages. This subclass answers them with
//!   the theme's colours, and paints over the light line Windows draws under the bar.
//!   The technique and structures follow muda's `dark_menu_bar.rs` (MIT OR Apache-2.0),
//!   which itself ports Notepad++ and PPSSPP; muda only paints dark, and only for the
//!   system theme, not Paper Mirror's own Light/Dark setting.
//!
//! In high-contrast mode nothing is painted, so Windows' own colours apply.

#![allow(non_snake_case, clippy::upper_case_acronyms)]

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    CreateSolidBrush, DT_CENTER, DT_HIDEPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, FillRect,
    GetWindowDC, HDC, MapWindowPoints, OffsetRect, RDW_FRAME, RDW_INVALIDATE, RedrawWindow, ReleaseDC, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ODS_DISABLED, ODS_GRAYED, ODS_HOTLIGHT, ODS_NOACCEL, ODS_SELECTED,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DrawMenuBar, GetClientRect, GetMenuBarInfo, GetMenuItemInfoW, GetWindowRect, HMENU, MENUBARINFO, MENUITEMINFOW,
    MIIM_STRING, OBJID_MENU, SPI_GETHIGHCONTRAST, SystemParametersInfoW, WM_NCACTIVATE, WM_NCDESTROY, WM_NCPAINT,
    WM_SETTINGCHANGE,
};

const SUBCLASS_ID: usize = 0x504D_4348; // "PMCH"
const WM_UAHDRAWMENU: u32 = 0x0091;
const WM_UAHDRAWMENUITEM: u32 = 0x0092;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_CAPTION_COLOR: u32 = 35;
const DWMWA_TEXT_COLOR: u32 = 36;

/// Theme colours as COLORREFs (0x00BBGGRR).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colors {
    pub dark: bool,
    pub bar: u32,
    pub text: u32,
    pub disabled: u32,
    pub hot: u32,
}

/// COLORREF from 8-bit RGB.
pub fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

thread_local! {
    static COLORS: RefCell<HashMap<isize, Colors>> = RefCell::new(HashMap::new());
    static SETTINGS_HOOK: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Installs the menu-bar painting on `hwnd` (a top-level window of this thread).
pub fn install(hwnd: isize) {
    unsafe { SetWindowSubclass(hwnd as HWND, Some(proc_), SUBCLASS_ID, 0) };
}

/// Sets what runs when Windows reports a settings change (accent colour, theme)
/// to a themed window. It runs inside the window procedure, so it should defer
/// any UI work.
pub fn on_settings_changed(f: impl Fn() + 'static) {
    SETTINGS_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

fn user_dword(key: &str, name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key = super::wide_str(key);
    let name = super::wide_str(name);
    let (mut value, mut size) = (0u32, 4u32);
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    (r == 0).then_some(value)
}

/// The Windows accent colour (Settings > Personalisation > Colours) as RGB, if set.
pub fn system_accent() -> Option<(u8, u8, u8)> {
    // Stored as 0xAABBGGRR.
    let v = user_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")?;
    Some((v as u8, (v >> 8) as u8, (v >> 16) as u8))
}

/// Whether Windows is set to dark mode for apps (Settings > Personalisation > Colours).
pub fn system_dark() -> bool {
    user_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme") == Some(0)
}

/// Applies `c` to the title bar and menu bar of `hwnd`, repainting them.
pub fn apply(hwnd: isize, c: Colors) {
    COLORS.with(|m| m.borrow_mut().insert(hwnd, c));
    let set = |attr: u32, value: u32| unsafe {
        DwmSetWindowAttribute(hwnd as HWND, attr, (&value as *const u32).cast(), 4);
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, c.dark as u32);
    if high_contrast() {
        return;
    }
    set(DWMWA_CAPTION_COLOR, c.bar);
    set(DWMWA_TEXT_COLOR, c.text);
    unsafe {
        DrawMenuBar(hwnd as HWND);
        RedrawWindow(hwnd as HWND, std::ptr::null(), std::ptr::null_mut(), RDW_FRAME | RDW_INVALIDATE);
    }
}

fn high_contrast() -> bool {
    let mut hc = HIGHCONTRASTW { cbSize: size_of::<HIGHCONTRASTW>() as u32, ..unsafe { std::mem::zeroed() } };
    let ok =
        unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, (&mut hc as *mut HIGHCONTRASTW).cast(), 0) };
    ok != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
}

#[repr(C)]
struct UAHMENU {
    hmenu: HMENU,
    hdc: HDC,
    dwFlags: u32,
}

#[repr(C)]
struct UAHMENUITEMMETRICS {
    rgsizeBar: [[u32; 2]; 2],
    rgsizePopup: [[u32; 2]; 4],
}

#[repr(C)]
struct UAHMENUPOPUPMETRICS {
    rgcx: [u32; 4],
    fUpdateMaxWidths: u32,
}

#[repr(C)]
struct UAHMENUITEM {
    iPosition: u32,
    umim: UAHMENUITEMMETRICS,
    umpm: UAHMENUPOPUPMETRICS,
}

#[repr(C)]
struct UAHDRAWMENUITEM {
    dis: DRAWITEMSTRUCT,
    um: UAHMENU,
    umi: UAHMENUITEM,
}

fn fill(hdc: HDC, rc: &RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(hdc, rc, brush);
        DeleteObject(brush);
    }
}

/// The menu bar's rectangle in window coordinates.
fn bar_rect(hwnd: HWND) -> Option<RECT> {
    let mut mbi = MENUBARINFO { cbSize: size_of::<MENUBARINFO>() as u32, ..unsafe { std::mem::zeroed() } };
    let mut wr: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        if GetMenuBarInfo(hwnd, OBJID_MENU, 0, &mut mbi) == 0 || GetWindowRect(hwnd, &mut wr) == 0 {
            return None;
        }
        let mut rc = mbi.rcBar;
        OffsetRect(&mut rc, -wr.left, -wr.top);
        rc.top -= 1;
        Some(rc)
    }
}

/// The one-pixel line Windows draws between the menu bar and the client area.
fn paint_bar_line(hwnd: HWND, color: u32) {
    unsafe {
        let mut client: RECT = std::mem::zeroed();
        let mut wr: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        MapWindowPoints(hwnd, std::ptr::null_mut(), (&mut client as *mut RECT).cast(), 2);
        GetWindowRect(hwnd, &mut wr);
        OffsetRect(&mut client, -wr.left, -wr.top);
        let line = RECT { left: client.left, top: client.top - 1, right: client.right, bottom: client.top };
        let hdc = GetWindowDC(hwnd);
        fill(hdc, &line, color);
        ReleaseDC(hwnd, hdc);
    }
}

fn paint_item(item: &mut UAHDRAWMENUITEM, c: &Colors) {
    let mut info: MENUITEMINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MENUITEMINFOW>() as u32;
    info.fMask = MIIM_STRING;
    let (hmenu, pos) = (item.um.hmenu, item.umi.iPosition);
    if unsafe { GetMenuItemInfoW(hmenu, pos, 1, &mut info) } == 0 {
        return;
    }
    let mut label = vec![0u16; info.cch as usize + 1];
    info.cch += 1;
    info.dwTypeData = label.as_mut_ptr();
    if unsafe { GetMenuItemInfoW(hmenu, pos, 1, &mut info) } == 0 {
        return;
    }
    let state = item.dis.itemState;
    let hot = state & (ODS_HOTLIGHT | ODS_SELECTED) != 0;
    let disabled = state & (ODS_GRAYED | ODS_DISABLED) != 0;
    let mut flags = DT_CENTER | DT_SINGLELINE | DT_VCENTER;
    if state & ODS_NOACCEL != 0 {
        flags |= DT_HIDEPREFIX;
    }
    let rc = item.dis.rcItem;
    fill(item.um.hdc, &rc, c.bar);
    if hot && !disabled {
        // A slightly inset highlight, like Windows 11's own menu bar.
        let inner = RECT { left: rc.left + 1, top: rc.top + 2, right: rc.right - 1, bottom: rc.bottom - 2 };
        fill(item.um.hdc, &inner, c.hot);
    }
    unsafe {
        SetBkMode(item.um.hdc, TRANSPARENT as i32);
        SetTextColor(item.um.hdc, if disabled { c.disabled } else { c.text });
        // After the second call `cch` is the label's length without the terminator.
        DrawTextW(item.um.hdc, label.as_ptr(), info.cch as i32, &mut item.dis.rcItem, flags);
    }
}

unsafe extern "system" fn proc_(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let colors = COLORS.with(|m| m.borrow().get(&(hwnd as isize)).copied());
    if msg == WM_SETTINGCHANGE {
        // winit resets the title bar to the system theme here; Paper Mirror's own
        // Light/Dark setting may differ, so apply the stored colours again.
        let r = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        if let Some(c) = colors {
            apply(hwnd as isize, c);
        }
        SETTINGS_HOOK.with(|h| {
            if let Some(f) = h.borrow().as_ref() {
                f();
            }
        });
        return r;
    }
    let active = colors.filter(|_| matches!(msg, WM_UAHDRAWMENU | WM_UAHDRAWMENUITEM | WM_NCPAINT | WM_NCACTIVATE));
    let Some(c) = active.filter(|_| !high_contrast()) else {
        if msg == WM_NCDESTROY {
            COLORS.with(|m| m.borrow_mut().remove(&(hwnd as isize)));
            unsafe { RemoveWindowSubclass(hwnd, Some(proc_), SUBCLASS_ID) };
        }
        return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_UAHDRAWMENU => {
            let menu = unsafe { &*(lparam as *const UAHMENU) };
            if let Some(rc) = bar_rect(hwnd) {
                fill(menu.hdc, &rc, c.bar);
            }
            0
        }
        WM_UAHDRAWMENUITEM => {
            paint_item(unsafe { &mut *(lparam as *mut UAHDRAWMENUITEM) }, &c);
            0
        }
        _ => {
            // WM_NCPAINT / WM_NCACTIVATE: let Windows (and muda) paint, then cover the line.
            let r = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            paint_bar_line(hwnd, c.bar);
            r
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn colorref_is_bgr() {
        assert_eq!(super::rgb(0x12, 0x34, 0x56), 0x0056_3412);
    }
}
