//! Text selection and copy. Page text geometry is cached per page so selection
//! hit-testing runs synchronously on the UI thread.

use std::rc::Rc;

use slint::ComponentHandle;

use super::DocWindow;
use crate::core::geometry::Point;
use crate::core::textsel::{Caret, PageText, TextSelection};
use crate::pdf::document::RenderTarget;
use crate::pdf::engine::Priority;
use crate::pdf::text;
use crate::ui::EditBridge;

/// Pages of text geometry kept in memory.
const TEXT_CACHE_PAGES: usize = 48;
/// Resolution of copied image regions: 200 % of actual size (192 pixels per inch).
const COPY_IMAGE_SCALE: f32 = 192.0 / 72.0;

impl DocWindow {
    pub(crate) fn insert_page_text(&self, page: usize, t: PageText) {
        let mut st = self.st.borrow_mut();
        st.pending_text.remove(&page);
        st.page_speech.insert(page, speech(&t));
        st.page_text.insert(page, Rc::new(t));
        if st.page_text.len() > TEXT_CACHE_PAGES {
            // Evict the page farthest from the current one (never a selected page).
            let current = st.current_page;
            let sel = st.text_sel.map(|s| s.ordered());
            let victim = st
                .page_text
                .keys()
                .copied()
                .filter(|p| sel.is_none_or(|(a, b)| *p < a.page || *p > b.page))
                .max_by_key(|p| p.abs_diff(current));
            if let Some(v) = victim {
                st.page_text.remove(&v);
                st.page_speech.remove(&v);
            }
        }
    }

    /// Caret under a content-space point, if that page's text is loaded.
    pub(crate) fn caret_at_content(&self, p: Point) -> Option<Caret> {
        let st = self.st.borrow();
        let page = st.layout.page_at(p).map(|(i, _)| i).or_else(|| st.layout.nearest_page(p))?;
        let t = st.page_text.get(&page)?;
        let pp = st.layout.to_page(page, p);
        Some(Caret { page, index: t.caret_at(pp) })
    }

    pub(crate) fn set_text_selection(&self, sel: Option<TextSelection>) {
        let sel = sel.filter(|s| !s.is_empty());
        self.st.borrow_mut().text_sel = sel;
        self.ui.set_has_text_selection(sel.is_some());
        self.refresh_overlays();
    }

    pub fn text_selection(&self) -> Option<TextSelection> {
        self.st.borrow().text_sel
    }

    /// Double-click: select the word under the pointer.
    pub(crate) fn select_word_at(&self, p: Point) {
        let Some(c) = self.caret_at_content(p) else { return };
        let (a, b) = {
            let st = self.st.borrow();
            let Some(t) = st.page_text.get(&c.page) else { return };
            t.word_at(c.index)
        };
        self.set_text_selection(Some(TextSelection {
            anchor: Caret { page: c.page, index: a },
            focus: Caret { page: c.page, index: b },
        }));
    }

    /// Forwards an edit operation to the focused text field. Returns false when no
    /// field has focus, meaning the command applies to the document.
    pub(crate) fn forward_to_field(&self, op: &str) -> bool {
        let bridge = self.ui.global::<EditBridge>();
        if !bridge.get_field_focused() {
            return false;
        }
        bridge.set_op(op.into());
        bridge.set_serial(bridge.get_serial() + 1);
        true
    }

    pub(crate) fn select_all(&self) {
        if self.forward_to_field("select-all") {
            return;
        }
        let n = self.st.borrow().doc.as_ref().map(|d| d.sizes.len()).unwrap_or(0);
        if n == 0 {
            return;
        }
        self.set_text_selection(Some(TextSelection {
            anchor: Caret { page: 0, index: 0 },
            focus: Caret { page: n - 1, index: usize::MAX },
        }));
    }

    pub(crate) fn copy(&self) {
        if self.forward_to_field("copy") {
            return;
        }
        let (sel, rect, doc_id) = {
            let st = self.st.borrow();
            (st.text_sel, st.rect_selection, st.doc.as_ref().map(|d| d.id))
        };
        let Some(doc_id) = doc_id else { return };
        let win = self.id;
        if let Some(sel) = sel {
            let (a, b) = sel.ordered();
            // Copy synchronously when every page is cached; otherwise gather on the
            // engine thread (Select All on a long document).
            let cached: Option<Vec<Rc<PageText>>> = {
                let st = self.st.borrow();
                (a.page..=b.page).map(|p| st.page_text.get(&p).cloned()).collect()
            };
            match cached {
                Some(pages) => {
                    let s = join_selection(&sel, a.page, &pages);
                    self.put_text(s);
                }
                None => self.engine.submit(Priority::Interactive, None, move |s| {
                    let Some(doc) = s.doc(doc_id) else { return };
                    let pages: Vec<Rc<PageText>> =
                        (a.page..=b.page).map(|p| Rc::new(text::page_geometry(doc, p))).collect();
                    let s = join_selection(&sel, a.page, &pages);
                    let _ = slint::invoke_from_event_loop(move || {
                        super::super::with_window(win, |w| w.put_text(s));
                    });
                }),
            }
        } else if let Some((page, r)) = rect {
            let (w, h) =
                ((r.w * COPY_IMAGE_SCALE).round().max(1.0) as u32, (r.h * COPY_IMAGE_SCALE).round().max(1.0) as u32);
            self.engine.submit(Priority::Interactive, None, move |s| {
                let Some(doc) = s.doc(doc_id) else { return };
                let mut px = vec![0u8; (w * h * 4) as usize];
                let target = RenderTarget { pixels: &mut px, width: w, height: h };
                if doc.render(page, COPY_IMAGE_SCALE, r.x * COPY_IMAGE_SCALE, r.y * COPY_IMAGE_SCALE, target, true) {
                    let _ = slint::invoke_from_event_loop(move || {
                        super::super::with_window(win, |w_| w_.put_image(w, h, px));
                    });
                }
            });
        }
    }

    fn put_text(&self, s: String) {
        if super::super::context().winit
            && let Err(e) = crate::platform_windows::clipboard::set_text(&s)
        {
            self.error("Couldn’t copy.", &e);
        }
        *self.last_copied.borrow_mut() = Some(s);
    }

    fn put_image(&self, w: u32, h: u32, px: Vec<u8>) {
        if super::super::context().winit
            && let Err(e) = crate::platform_windows::clipboard::set_image(w, h, &px)
        {
            self.error("Couldn’t copy.", &e);
        }
        *self.last_copied.borrow_mut() = Some(format!("[image {w}x{h}]"));
    }
}

/// Concatenates the selected text of consecutive pages, one line break between pages.
fn join_selection(sel: &TextSelection, first_page: usize, pages: &[Rc<PageText>]) -> String {
    let mut out = String::new();
    for (i, t) in pages.iter().enumerate() {
        let page = first_page + i;
        if let Some((s, e)) = sel.range_on(page, t.len()) {
            if !out.is_empty() && !out.ends_with("\r\n") {
                out.push_str("\r\n");
            }
            out.push_str(&t.text(s, e));
        }
    }
    out
}

/// A page's text as a screen reader should hear it: one line per line of the page,
/// with runs of blank space collapsed.
fn speech(t: &PageText) -> slint::SharedString {
    let text = t.text(0, t.len());
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines.join("\n").into()
}
