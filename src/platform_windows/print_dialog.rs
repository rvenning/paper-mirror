//! The Windows Print dialog (PrintDlgEx): printer, copies, collation and page range.
//! The driver handles copies and collation; Paper Mirror sends each page once.

use windows::Win32::Foundation::{GlobalFree, HWND};
use windows::Win32::UI::Controls::Dialogs::{
    PD_CURRENTPAGE, PD_NOSELECTION, PD_PAGENUMS, PD_RESULT_PRINT, PD_RETURNDC, PD_SELECTION,
    PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGEXW, PRINTPAGERANGE, PrintDlgExW, START_PAGE_GENERAL,
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
}

/// Shows the dialog; None if cancelled.
pub fn show(hwnd: isize, page_count: u32, has_selection: bool) -> Result<Option<Chosen>, String> {
    let n = page_count.max(1);
    let mut ranges = vec![PRINTPAGERANGE { nFromPage: 1, nToPage: n }; 64];
    let mut pd = PRINTDLGEXW {
        lStructSize: std::mem::size_of::<PRINTDLGEXW>() as u32,
        hwndOwner: HWND(hwnd as *mut _),
        Flags: PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE,
        nPageRanges: 0,
        nMaxPageRanges: ranges.len() as u32,
        lpPageRanges: ranges.as_mut_ptr(),
        nMinPage: 1,
        nMaxPage: n,
        nCopies: 1,
        nStartPage: START_PAGE_GENERAL,
        ..Default::default()
    };
    if !has_selection {
        pd.Flags |= PD_NOSELECTION;
    }
    unsafe { PrintDlgExW(&mut pd) }.map_err(|e| e.message())?;
    unsafe {
        if !pd.hDevMode.is_invalid() {
            let _ = GlobalFree(Some(pd.hDevMode));
        }
        if !pd.hDevNames.is_invalid() {
            let _ = GlobalFree(Some(pd.hDevNames));
        }
    }
    let dc = pd.hDC.0 as usize;
    if pd.dwResultAction != PD_RESULT_PRINT || dc == 0 {
        if dc != 0 {
            crate::pdf::print::delete_dc(dc);
        }
        return Ok(None);
    }
    let range = if pd.Flags.contains(PD_SELECTION) {
        Range::Selection
    } else if pd.Flags.contains(PD_CURRENTPAGE) {
        Range::Current
    } else if pd.Flags.contains(PD_PAGENUMS) {
        Range::Pages(ranges[..pd.nPageRanges as usize].iter().map(|r| (r.nFromPage, r.nToPage)).collect())
    } else {
        Range::All
    };
    Ok(Some(Chosen { dc, range }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_expand_to_page_indices() {
        assert_eq!(Range::All.pages(3, 0, &[]), vec![0, 1, 2]);
        assert_eq!(Range::Current.pages(3, 7, &[]), vec![2]);
        assert_eq!(Range::Selection.pages(5, 0, &[3, 1]), vec![1, 3]);
        assert_eq!(Range::Pages(vec![(2, 3), (1, 1), (3, 9)]).pages(5, 0, &[]), vec![0, 1, 2, 3, 4]);
        assert_eq!(Range::Pages(vec![(4, 2)]).pages(5, 0, &[]), vec![1, 2, 3]);
    }
}
