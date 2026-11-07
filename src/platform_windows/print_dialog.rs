//! The Windows Print dialog (PrintDlg): printer, copies, collation and page range.
//! The driver handles copies and collation; Paper Mirror sends each page once.

use windows::Win32::Foundation::{GlobalFree, HWND};
use windows::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, PD_ALLPAGES, PD_NOSELECTION, PD_PAGENUMS, PD_PRINTTOFILE, PD_RETURNDC, PD_SELECTION,
    PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW, PrintDlgW,
};

/// The pages the user chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Range {
    All,
    Current,
    Selection,
    /// 1-based inclusive ranges as typed.
    Pages(Vec<(u32, u32)>),
}

impl Range {
    /// Zero-based page indices for a document of `n` pages.
    pub fn pages(&self, n: usize, current: usize, selection: &[usize]) -> Vec<usize> {
        let mut v: Vec<usize> = match self {
            Range::All => (0..n).collect(),
            Range::Current => vec![current.min(n.saturating_sub(1))],
            Range::Selection => selection.to_vec(),
            Range::Pages(r) => r
                .iter()
                .flat_map(|&(a, b)| {
                    let (a, b) = (a.min(b).max(1) as usize, a.max(b) as usize);
                    (a - 1)..b.min(n)
                })
                .collect(),
        };
        // Ranges may overlap ("1-3, 2"); each page prints once, in order.
        v.retain(|p| *p < n);
        v.sort_unstable();
        v.dedup();
        v
    }
}

pub struct Chosen {
    /// The printer device context (delete it with `pdf::print::delete_dc`).
    pub dc: usize,
    pub range: Range,
    /// The printer's name, e.g. "Microsoft Print to PDF".
    pub device: String,
    /// The printer writes a file and would ask for its name (port PORTPROMPT: or FILE:,
    /// or "Print to file" ticked). Paper Mirror asks on the UI thread instead, because
    /// the driver's own prompt can't appear from the print thread.
    pub needs_file: bool,
}

/// Reads the device and port names from a DEVNAMES block.
fn devnames(h: windows::Win32::Foundation::HGLOBAL) -> (String, String) {
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    if h.is_invalid() {
        return Default::default();
    }
    unsafe {
        let p = GlobalLock(h.0) as *const u16;
        if p.is_null() {
            return Default::default();
        }
        let read = |offset: u16| {
            let s = p.add(offset as usize);
            let len = (0..512).take_while(|i| *s.add(*i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(s, len))
        };
        // DEVNAMES: wDriverOffset, wDeviceOffset, wOutputOffset, wDefault (in WCHARs).
        let names = (read(*p.add(1)), read(*p.add(2)));
        GlobalUnlock(h.0);
        names
    }
}

/// Whether a printer port means "ask for a file name".
pub fn port_needs_file(port: &str) -> bool {
    let p = port.trim().to_ascii_uppercase();
    p == "PORTPROMPT:" || p == "FILE:"
}

/// The file type a printing-to-file driver produces.
pub fn output_extension(device: &str) -> &'static str {
    let d = device.to_ascii_lowercase();
    if d.contains("pdf") {
        "pdf"
    } else if d.contains("xps") {
        "oxps"
    } else {
        "prn"
    }
}

/// Shows the classic Print dialog (`PrintDlg`); None if cancelled.
///
/// `PrintDlgEx` (the newer dialog with a printer list view) was tried first, but on a
/// PC with an offline network (WSD) printer it never appears and freezes the window;
/// the classic dialog opens at once with the same printer, copies and range choices.
pub fn show(hwnd: isize, page_count: u32, has_selection: bool) -> Result<Option<Chosen>, String> {
    let n = page_count.clamp(1, u16::MAX as u32) as u16;
    let mut pd = PRINTDLGW {
        lStructSize: std::mem::size_of::<PRINTDLGW>() as u32,
        hwndOwner: HWND(hwnd as *mut _),
        Flags: PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE | PD_ALLPAGES,
        nFromPage: 1,
        nToPage: n,
        nMinPage: 1,
        nMaxPage: n,
        nCopies: 1,
        ..Default::default()
    };
    if !has_selection {
        pd.Flags |= PD_NOSELECTION;
    }
    let ok = unsafe { PrintDlgW(&mut pd) }.as_bool();
    let (device, port) = devnames(pd.hDevNames);
    unsafe {
        if !pd.hDevMode.is_invalid() {
            let _ = GlobalFree(Some(pd.hDevMode));
        }
        if !pd.hDevNames.is_invalid() {
            let _ = GlobalFree(Some(pd.hDevNames));
        }
    }
    let dc = pd.hDC.0 as usize;
    if !ok || dc == 0 {
        if dc != 0 {
            crate::pdf::print::delete_dc(dc);
        }
        // Zero means cancelled; anything else is a real failure.
        let code = unsafe { CommDlgExtendedError() };
        return if code.0 == 0 { Ok(None) } else { Err(format!("The Print dialog failed (code {:#x}).", code.0)) };
    }
    let range = if pd.Flags.contains(PD_SELECTION) {
        Range::Selection
    } else if pd.Flags.contains(PD_PAGENUMS) {
        Range::Pages(vec![(pd.nFromPage as u32, pd.nToPage as u32)])
    } else {
        Range::All
    };
    let needs_file = port_needs_file(&port) || pd.Flags.contains(PD_PRINTTOFILE);
    Ok(Some(Chosen { dc, range, device, needs_file }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_ports_and_extensions() {
        assert!(port_needs_file("PORTPROMPT:"));
        assert!(port_needs_file("file:"));
        assert!(!port_needs_file("WSD-871246f0"));
        assert!(!port_needs_file("nul:"));
        assert_eq!(output_extension("Microsoft Print to PDF"), "pdf");
        assert_eq!(output_extension("Microsoft XPS Document Writer"), "oxps");
        assert_eq!(output_extension("Brother MFC"), "prn");
    }

    #[test]
    fn ranges_expand_to_page_indices() {
        assert_eq!(Range::All.pages(3, 0, &[]), vec![0, 1, 2]);
        assert_eq!(Range::Current.pages(3, 7, &[]), vec![2]);
        assert_eq!(Range::Selection.pages(5, 0, &[3, 1]), vec![1, 3]);
        assert_eq!(Range::Pages(vec![(2, 3), (1, 1), (3, 9)]).pages(5, 0, &[]), vec![0, 1, 2, 3, 4]);
        assert_eq!(Range::Pages(vec![(4, 2)]).pages(5, 0, &[]), vec![1, 2, 3]);
    }
}
