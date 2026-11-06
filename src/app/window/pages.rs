//! Document-level undo: page operations snapshot the whole PDF (bytes) before they
//! run; undo swaps the engine's document for the snapshot and keeps the current state
//! for redo.

use std::sync::Arc;

use super::{DocWindow, Snapshot};
use crate::core::geometry::Size;
use crate::pdf::document::PdfDoc;
use crate::pdf::engine::{DocId, Priority};
use crate::pdf::ffi::Source;

/// Snapshots kept for undo are capped in total size; older ones are dropped first.
pub(crate) const SNAPSHOT_BUDGET: usize = 256 * 1024 * 1024;

impl DocWindow {
    /// Replaces the engine document with `snap.pdf` and the annotations with
    /// `snap.annots`, pushing the current state onto the opposite stack.
    pub(crate) fn restore_pdf_snapshot(&self, snap: Snapshot, undo: bool) {
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let Some(bytes) = snap.pdf.clone() else { return };
        let current_annots = self.annotations();
        let win = self.id;
        self.engine.submit(Priority::Interactive, None, move |s| {
            let result = swap_document(s, doc_id, bytes);
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| {
                    let Ok((previous, sizes)) = result else {
                        w.error("Undo failed.", "The previous version of the document could not be restored.");
                        return;
                    };
                    {
                        let mut st = w.st.borrow_mut();
                        let other = Snapshot { annots: current_annots, pdf: Some(previous) };
                        if undo {
                            st.redo.push(other);
                        } else {
                            st.undo.push(other);
                        }
                        st.annots = snap.annots;
                        st.selected_annot = None;
                    }
                    w.apply_new_sizes(sizes);
                    w.mark_dirty();
                    w.sync_undo_ui();
                });
            });
        });
    }

    /// Adopts new page sizes after the page structure changed.
    pub(crate) fn apply_new_sizes(&self, sizes: Vec<Size>) {
        {
            let mut st = self.st.borrow_mut();
            let n = sizes.len();
            if let Some(d) = st.doc.as_mut() {
                d.sizes = sizes;
            }
            st.current_page = st.current_page.min(n.saturating_sub(1));
            st.selected.retain(|p| *p < n);
            st.bookmarks.retain(|p| *p < n);
            st.layout = Default::default();
            st.search = Default::default();
            st.text_sel = None;
            st.rect_selection = None;
            st.outline.clear();
        }
        let n = self.st.borrow().doc.as_ref().map(|d| d.sizes.len()).unwrap_or(0);
        self.ui.set_multi_page(n > 1);
        self.search_model.set_vec(Vec::new());
        self.ui.set_has_search(false);
        self.invalidate_renders();
        self.relayout(None);
        if let Some(id) = self.st.borrow().doc.as_ref().map(|d| d.id) {
            self.load_outline(id);
        }
        self.refresh_notes_list();
        self.refresh_bookmark_rows();
    }

    /// Trims undo snapshots so the PDF copies stay within budget.
    pub(crate) fn trim_snapshots(&self) {
        let mut st = self.st.borrow_mut();
        loop {
            let total: usize =
                st.undo.iter().chain(st.redo.iter()).filter_map(|s| s.pdf.as_ref().map(|p| p.len())).sum();
            if total <= SNAPSHOT_BUDGET {
                break;
            }
            match st.undo.iter().position(|s| s.pdf.is_some()) {
                Some(i) => {
                    st.undo.remove(i);
                }
                None => break,
            }
        }
    }
}

/// Engine side of a snapshot swap: returns the bytes of the document being replaced
/// and the new page sizes.
pub(crate) fn swap_document(
    s: &mut crate::pdf::engine::EngineState,
    doc_id: DocId,
    bytes: Arc<Vec<u8>>,
) -> Result<(Arc<Vec<u8>>, Vec<Size>), String> {
    let old = s.doc(doc_id).ok_or("document closed")?;
    let previous = Arc::new(old.doc.save_to_vec(old.doc.security_revision() != -1).ok_or("save failed")?);
    let path = old.path.clone();
    let password = old.password.clone();
    let mut new = PdfDoc::open(Source::Memory(bytes), None).map_err(|e| e.to_string())?;
    new.path = path;
    new.password = password;
    let sizes = new.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
    s.docs.insert(doc_id, new);
    Ok((previous, sizes))
}
