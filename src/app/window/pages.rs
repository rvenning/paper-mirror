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
        self.load_fields();
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

/// Document edits run on the engine thread; the UI follows up based on the kind.
#[derive(Clone, Debug)]
pub(crate) enum EditKind {
    /// A form field on `page` changed.
    Form { page: usize },
}

/// Above this size, edits are not snapshotted for undo (memory and time).
const SNAPSHOT_MAX_DOC: u64 = 64 * 1024 * 1024;

impl DocWindow {
    /// Runs `f` on the engine's document, keeping a snapshot for undo, then updates
    /// the UI according to `kind`.
    pub(crate) fn edit_document(&self, kind: EditKind, f: impl FnOnce(&PdfDoc) -> Result<(), String> + Send + 'static) {
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let small = self.doc_path().and_then(|p| std::fs::metadata(p).ok()).is_none_or(|m| m.len() <= SNAPSHOT_MAX_DOC);
        let win = self.id;
        self.engine.submit(Priority::Interactive, None, move |s| {
            let result = (|| {
                let doc = s.doc(doc_id).ok_or("The document is no longer open.")?;
                let before =
                    if small { doc.doc.save_to_vec(doc.doc.security_revision() != -1).map(Arc::new) } else { None };
                f(doc)?;
                let sizes: Vec<Size> = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
                Ok::<_, String>((before, sizes))
            })();
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_document_edited(doc_id, kind, result));
            });
        });
    }

    fn on_document_edited(
        &self,
        doc_id: DocId,
        kind: EditKind,
        result: Result<(Option<Arc<Vec<u8>>>, Vec<Size>), String>,
    ) {
        if self.st.borrow().doc.as_ref().is_none_or(|d| d.id != doc_id) {
            return;
        }
        let (before, _sizes) = match result {
            Ok(r) => r,
            Err(e) => {
                self.error("The change couldn’t be made.", &e);
                return;
            }
        };
        if let Some(pdf) = before {
            let annots = self.annotations();
            let mut st = self.st.borrow_mut();
            st.undo.push(Snapshot { annots, pdf: Some(pdf) });
            st.redo.clear();
        }
        self.trim_snapshots();
        self.sync_undo_ui();
        self.mark_dirty();
        match kind {
            EditKind::Form { page } => {
                self.invalidate_page(page);
                self.load_fields();
            }
        }
    }

    /// Re-renders one page (its content changed).
    pub(crate) fn invalidate_page(&self, page: usize) {
        {
            let mut st = self.st.borrow_mut();
            st.page_cache.as_mut().unwrap().remove(&page);
            st.thumb_cache.as_mut().unwrap().remove(&page);
            st.sheet_cache.as_mut().unwrap().remove(&page);
            st.pending_pages.retain(|(p, _)| *p != page);
            st.pending_thumbs.remove(&page);
            if st.tile.as_ref().is_some_and(|t| t.page == page) {
                st.tile = None;
            }
        }
        self.update_view();
        self.update_thumbs();
    }
}
