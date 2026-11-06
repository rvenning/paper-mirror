//! Page organisation from the UI: thumbnail selection and drag-reordering, rotate,
//! delete, insert, combine and crop, with annotations, bookmarks and the current page
//! carried across each change and a document snapshot for undo.

use std::path::PathBuf;
use std::sync::Arc;

use super::{DocWindow, Snapshot};
use crate::core::geometry::{Affine, Size};
use crate::core::pagemap::{self, PageOp};
use crate::pdf::document::PdfDoc;
use crate::pdf::engine::{DocId, Priority};
use crate::pdf::ffi::Source;
use crate::pdf::{annot_io, pageops};

/// Thumbnail drag in progress: pages being moved and the current drop gap.
#[derive(Clone, Debug, Default)]
pub(crate) struct ThumbDrag {
    pub pages: Vec<usize>,
    pub start_y: f32,
    pub active: bool,
    pub gap: Option<usize>,
}

type OpResult = Result<(Arc<Vec<u8>>, Vec<Size>, Vec<Affine>, Vec<Affine>), String>;

impl DocWindow {
    /// Pages the next page command acts on: the thumbnail selection, else the current page.
    pub fn selected_pages(&self) -> Vec<usize> {
        let st = self.st.borrow();
        let mut v = st.selected.clone();
        if v.is_empty() {
            v.push(st.current_page);
        }
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Click on a thumbnail: plain selects one page, Ctrl toggles, Shift extends.
    pub fn thumb_clicked(&self, page: usize, ctrl: bool, shift: bool) {
        {
            let mut st = self.st.borrow_mut();
            if ctrl {
                if let Some(i) = st.selected.iter().position(|p| *p == page) {
                    if st.selected.len() > 1 {
                        st.selected.remove(i);
                    }
                } else {
                    st.selected.push(page);
                }
                st.page_anchor = page;
            } else if shift {
                let (a, b) = (st.page_anchor.min(page), st.page_anchor.max(page));
                st.selected = (a..=b).collect();
            } else {
                st.selected = vec![page];
                st.page_anchor = page;
            }
        }
        if ctrl || shift {
            self.update_thumbs();
        } else {
            self.go_to_page(page);
        }
    }

    /// Thumbnail drag in content coordinates of the thumbnail list.
    pub fn thumb_drag(&self, page: usize, y: f32) {
        let gap = {
            let st = self.st.borrow();
            // The gap before the first thumbnail whose centre lies below the pointer.
            st.thumb_rects.iter().position(|r| y < r.center().y).unwrap_or(st.thumb_rects.len())
        };
        let (active, marker) = {
            let mut st = self.st.borrow_mut();
            if st.thumb_drag.is_none() {
                let pages = if st.selected.contains(&page) { st.selected.clone() } else { vec![page] };
                st.thumb_drag = Some(ThumbDrag { pages, start_y: y, active: false, gap: None });
            }
            let rects = st.thumb_rects.clone();
            let d = st.thumb_drag.as_mut().unwrap();
            if !d.active && (y - d.start_y).abs() > 6.0 {
                d.active = true;
            }
            d.gap = Some(gap);
            let marker = if gap < rects.len() {
                rects[gap].y - 4.0
            } else {
                rects.last().map(|r| r.bottom() + 24.0).unwrap_or(0.0)
            };
            (d.active, marker)
        };
        self.ui.set_thumb_drop_visible(active);
        self.ui.set_thumb_drop_y(marker);
    }

    pub fn thumb_drop(&self) {
        let drag = self.st.borrow_mut().thumb_drag.take();
        self.ui.set_thumb_drop_visible(false);
        let Some(d) = drag else { return };
        let Some(gap) = d.gap.filter(|_| d.active) else { return };
        let mut pages = d.pages;
        pages.sort_unstable();
        let dest = pagemap::drop_gap_to_dest(&pages, gap);
        let n = self.page_count();
        let op = PageOp::Move { pages: pages.clone(), dest };
        // Skip drops that leave the order unchanged.
        if op.remap(n).iter().enumerate().all(|(i, m)| *m == Some(i)) {
            return;
        }
        self.apply_page_op(op, None);
    }

    /// Current page sizes (points, as displayed).
    pub fn page_sizes(&self) -> Vec<Size> {
        self.st.borrow().doc.as_ref().map(|d| d.sizes.clone()).unwrap_or_default()
    }

    /// Thumbnail-list y positions to drag `page` from, and past the last page.
    pub fn thumb_drag_points(&self, page: usize) -> (f32, f32) {
        let st = self.st.borrow();
        let r = &st.thumb_rects;
        (r.get(page).map(|r| r.center().y).unwrap_or(0.0), r.last().map(|r| r.bottom() + 10.0).unwrap_or(0.0))
    }

    pub fn page_count(&self) -> usize {
        self.st.borrow().doc.as_ref().map(|d| d.sizes.len()).unwrap_or(0)
    }

    pub fn rotate_pages(&self, quarter_turns: i32) {
        let pages = self.selected_pages();
        self.apply_page_op(PageOp::Rotate { pages, quarter_turns }, None);
    }

    pub fn delete_pages(&self) {
        let pages = self.selected_pages();
        if pages.len() >= self.page_count() {
            self.error("These pages can’t be deleted.", "A document must keep at least one page.");
            return;
        }
        self.apply_page_op(PageOp::Delete { pages }, None);
    }

    pub fn insert_blank_page(&self) {
        let (at, size) = {
            let st = self.st.borrow();
            let Some(doc) = st.doc.as_ref() else { return };
            let page = st.current_page;
            (page + 1, doc.sizes.get(page).copied().unwrap_or(Size::new(612.0, 792.0)))
        };
        self.apply_page_op(PageOp::Insert { at, count: 1, blank: Some(size) }, None);
    }

    /// Inserts all pages of another PDF after the current page (Edit > Insert >
    /// Page from File; also how documents are combined).
    pub fn insert_from_file(&self, path: Option<PathBuf>) {
        let path = match path {
            Some(p) => p,
            None => {
                if !super::super::context().winit {
                    return;
                }
                use slint::ComponentHandle;
                let Some(p) = rfd::FileDialog::new()
                    .set_title("Insert Pages from File")
                    .add_filter("PDF documents", &["pdf"])
                    .set_parent(&self.ui.window().window_handle())
                    .pick_file()
                else {
                    return;
                };
                p
            }
        };
        let at = self.current_page() + 1;
        self.apply_page_op(PageOp::Insert { at, count: usize::MAX, blank: None }, Some(path));
    }

    /// Crops the page to the rectangular selection (Tools > Crop).
    pub fn crop_to_selection(&self) {
        let Some((page, rect)) = self.rect_selection() else {
            self.error("Nothing to crop.", "Choose Tools > Rectangular Selection and drag over the area to keep.");
            return;
        };
        self.clear_selection();
        self.apply_page_op(PageOp::Crop { page, rect }, None);
    }

    /// Runs a page operation on the engine thread with an undo snapshot.
    pub fn apply_page_op(&self, op: PageOp, source: Option<PathBuf>) {
        self.finish_pending_edit();
        let Some(doc_id) = self.st.borrow().doc.as_ref().map(|d| d.id) else { return };
        let win = self.id;
        let op_engine = op.clone();
        self.ui.set_busy_text("Updating pages…".into());
        self.engine.submit(Priority::Interactive, None, move |s| {
            let result: OpResult = (|| {
                let src = match &source {
                    Some(p) => Some(PdfDoc::open(Source::File(p.clone()), None).map_err(|e| e.to_string())?),
                    None => None,
                };
                let mut op = op_engine;
                if let (PageOp::Insert { count, blank: None, .. }, Some(src)) = (&mut op, &src) {
                    *count = src.page_count();
                }
                let doc = s.doc_mut(doc_id).ok_or("The document is no longer open.")?;
                let before = doc
                    .doc
                    .save_to_vec(doc.doc.security_revision() != -1)
                    .ok_or("The document could not be copied for undo.")?;
                let old_t = annot_io::transforms(doc);
                pageops::apply(doc, &op, src.as_ref())?;
                let new_t = annot_io::transforms(doc);
                let sizes = doc.page_sizes().iter().map(|(w, h)| Size::new(*w, *h)).collect();
                Ok((Arc::new(before), sizes, old_t, new_t))
            })();
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| w.on_page_op(doc_id, op, result));
            });
        });
    }

    fn on_page_op(&self, doc_id: DocId, op: PageOp, result: OpResult) {
        self.ui.set_busy_text(Default::default());
        if self.st.borrow().doc.as_ref().is_none_or(|d| d.id != doc_id) {
            return;
        }
        let (before, sizes, old_t, new_t) = match result {
            Ok(r) => r,
            Err(e) => {
                self.error("The pages couldn’t be changed.", &e);
                return;
            }
        };
        let n_old = old_t.len();
        // An insert from a file only learns its page count on the engine thread.
        let op = match op {
            PageOp::Insert { at, blank: None, .. } => PageOp::Insert { at, count: sizes.len() - n_old, blank: None },
            other => other,
        };
        let map = op.remap(n_old);
        {
            let mut st = self.st.borrow_mut();
            let annots = std::mem::take(&mut st.annots);
            st.undo.push(Snapshot { annots: annots.clone(), pdf: Some(before) });
            st.redo.clear();
            st.annots = annots
                .into_iter()
                .filter_map(|a| {
                    let new_page = map.get(a.page).copied().flatten()?;
                    let to_pdf = old_t.get(a.page)?;
                    let to_display = new_t.get(new_page)?.inverse()?;
                    let mut moved = a.transformed(to_pdf).transformed(&to_display);
                    moved.page = new_page;
                    Some(moved)
                })
                .collect();
            st.selected_annot = None;
            st.bookmarks = st.bookmarks.iter().filter_map(|b| map.get(*b).copied().flatten()).collect();
            st.bookmarks.sort_unstable();
            let current = st.current_page;
            st.current_page = map.get(current).copied().flatten().unwrap_or(current.min(sizes.len().saturating_sub(1)));
            st.selected = st.selected.iter().filter_map(|p| map.get(*p).copied().flatten()).collect();
            if let PageOp::Insert { at, count, .. } = op {
                st.selected = (at.min(n_old)..at.min(n_old) + count).collect();
            }
        }
        if let Some(path) = self.doc_path() {
            let marks = self.bookmarks();
            super::super::update_settings(|s| s.set_bookmarks(&path, marks));
        }
        self.trim_snapshots();
        self.apply_new_sizes(sizes);
        let page = self.current_page();
        self.go_to_page(page);
        self.mark_dirty();
        self.sync_undo_ui();
    }
}
