//! Sidebar list modes: table of contents, bookmarks and search results, plus the
//! incremental search engine that feeds them.

use slint::{Model, SharedString};

use super::DocWindow;
use crate::core::layout::DisplayMode;
use crate::pdf::engine::{DocId, Engine, Generation, Priority};
use crate::pdf::outline;
use crate::pdf::text::{self, SearchHit, SearchOptions};
use crate::ui::ListRow;

/// Pages searched per engine job. Small enough that renders interleave smoothly.
const SEARCH_CHUNK: usize = 6;

impl DocWindow {
    pub(crate) fn set_sidebar_mode(&self, mode: &str) {
        self.ui.set_sidebar_visible(true);
        self.ui.set_sidebar_mode(mode.into());
        match mode {
            "thumbnails" => self.update_thumbs(),
            "contents" => self.refresh_outline_rows(),
            "bookmarks" => self.refresh_bookmark_rows(),
            _ => {}
        }
    }

    pub(crate) fn list_row_clicked(&self, mode: &str, id: i32) {
        let id = id.max(0) as usize;
        match mode {
            "contents" => {
                let page = self.st.borrow().outline.get(id).and_then(|o| o.page);
                if let Some(p) = page {
                    self.go_to_page(p);
                }
            }
            "bookmarks" => self.go_to_page(id),
            "search" => self.select_hit(id),
            _ => {}
        }
    }

    pub(crate) fn list_row_toggled(&self, mode: &str, id: i32) {
        if mode != "contents" {
            return;
        }
        {
            let mut st = self.st.borrow_mut();
            let id = id.max(0) as usize;
            if !st.outline_collapsed.remove(&id) {
                st.outline_collapsed.insert(id);
            }
        }
        self.refresh_outline_rows();
    }

    // ----- Outline -----------------------------------------------------------------

    /// Fetches the outline after a document opens. Preview shows the table of
    /// contents automatically when a document has one; so does Paper Mirror.
    pub(crate) fn load_outline(&self, doc_id: DocId) {
        let win = self.id;
        self.engine.submit(Priority::Interactive, None, move |s| {
            let Some(doc) = s.doc(doc_id) else { return };
            let items = outline::outline(doc);
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| {
                    {
                        let mut st = w.st.borrow_mut();
                        if st.doc.as_ref().is_none_or(|d| d.id != doc_id) {
                            return;
                        }
                        // Start with nested levels collapsed below depth 1.
                        st.outline_collapsed = items
                            .iter()
                            .enumerate()
                            .filter(|(_, o)| o.depth >= 1 && o.has_children)
                            .map(|(i, _)| i)
                            .collect();
                        st.outline = items;
                    }
                    w.refresh_outline_rows();
                });
            });
        });
    }

    pub(crate) fn refresh_outline_rows(&self) {
        let st = self.st.borrow();
        let current = st.current_page;
        let mut rows = Vec::new();
        let mut hidden_below: Option<usize> = None;
        // The deepest outline entry at or before the current page is highlighted.
        let active = st
            .outline
            .iter()
            .enumerate()
            .filter(|(_, o)| o.page.is_some_and(|p| p <= current))
            .max_by_key(|(_, o)| o.page)
            .map(|(i, _)| i);
        for (i, o) in st.outline.iter().enumerate() {
            if let Some(d) = hidden_below {
                if o.depth > d {
                    continue;
                }
                hidden_below = None;
            }
            let collapsed = st.outline_collapsed.contains(&i);
            if o.has_children && collapsed {
                hidden_below = Some(o.depth);
            }
            rows.push(ListRow {
                id: i as i32,
                depth: o.depth as i32,
                title: if o.title.is_empty() { "Untitled".into() } else { o.title.as_str().into() },
                detail: o.page.map(|p| format!("Page {}", p + 1)).unwrap_or_default().into(),
                selected: active == Some(i),
                expandable: o.has_children,
                expanded: o.has_children && !collapsed,
            });
        }
        drop(st);
        self.outline_model.set_vec(rows);
    }

    pub fn outline_len(&self) -> usize {
        self.st.borrow().outline.len()
    }

    // ----- Bookmarks ---------------------------------------------------------------

    pub(crate) fn load_bookmarks(&self) {
        let Some(path) = self.doc_path() else { return };
        let marks = super::super::context().settings.borrow().bookmarks_for(&path);
        let n = self.st.borrow().doc.as_ref().map(|d| d.sizes.len()).unwrap_or(0);
        self.st.borrow_mut().bookmarks = marks.into_iter().filter(|p| *p < n).collect();
        self.refresh_bookmark_rows();
    }

    pub(crate) fn toggle_bookmark(&self) {
        let Some(path) = self.doc_path() else { return };
        let marks = {
            let mut st = self.st.borrow_mut();
            let page = st.current_page;
            if let Some(i) = st.bookmarks.iter().position(|p| *p == page) {
                st.bookmarks.remove(i);
            } else {
                st.bookmarks.push(page);
                st.bookmarks.sort_unstable();
            }
            st.bookmarks.clone()
        };
        super::super::update_settings(|s| s.set_bookmarks(&path, marks));
        self.refresh_bookmark_rows();
        self.update_thumbs();
    }

    pub fn bookmarks(&self) -> Vec<usize> {
        self.st.borrow().bookmarks.clone()
    }

    pub(crate) fn refresh_bookmark_rows(&self) {
        let st = self.st.borrow();
        let rows: Vec<ListRow> = st
            .bookmarks
            .iter()
            .map(|p| ListRow {
                id: *p as i32,
                depth: 0,
                title: format!("Page {}", p + 1).into(),
                detail: SharedString::new(),
                selected: *p == st.current_page,
                expandable: false,
                expanded: false,
            })
            .collect();
        let bookmarked = st.bookmarks.contains(&st.current_page);
        drop(st);
        self.bookmarks_model.set_vec(rows);
        self.ui.set_page_bookmarked(bookmarked);
    }

    // ----- Search ------------------------------------------------------------------

    /// Starts (or clears, for an empty query) a whole-document search.
    pub fn start_search(&self, query: &str) {
        let query = query.trim().to_string();
        let generation = self.search_gen.bump();
        let doc = {
            let mut st = self.st.borrow_mut();
            st.search = super::SearchState { query: query.clone(), ..Default::default() };
            st.doc.as_ref().map(|d| (d.id, d.sizes.len()))
        };
        self.search_model.set_vec(Vec::new());
        if query.is_empty() || doc.is_none() {
            self.ui.set_has_search(false);
            self.ui.set_search_status(SharedString::new());
            if self.ui.get_sidebar_mode() == "search" {
                self.ui.set_sidebar_mode("thumbnails".into());
                self.update_thumbs();
            }
            self.update_view();
            return;
        }
        let (doc_id, pages) = doc.unwrap();
        self.ui.set_has_search(true);
        self.ui.set_sidebar_visible(true);
        self.ui.set_sidebar_mode("search".into());
        self.ui.set_search_status("Searching…".into());
        search_chunk(self.engine.clone(), self.id, doc_id, query, 0, pages, generation);
    }

    fn on_search_chunk(&self, doc_id: DocId, query: &str, done: usize, total: usize, hits: Vec<SearchHit>) {
        let first_hit;
        {
            let mut st = self.st.borrow_mut();
            if st.doc.as_ref().is_none_or(|d| d.id != doc_id) || st.search.query != query {
                return;
            }
            first_hit = st.search.hits.is_empty() && !hits.is_empty();
            let base = st.search.hits.len();
            for (i, h) in hits.iter().enumerate() {
                self.search_model.push(ListRow {
                    id: (base + i) as i32,
                    depth: 0,
                    title: h.context.as_str().into(),
                    detail: format!("Page {}", h.page + 1).into(),
                    selected: false,
                    expandable: false,
                    expanded: false,
                });
            }
            st.search.hits.extend(hits);
            st.search.pages_done = done;
            st.search.finished = done >= total;
        }
        let n = self.st.borrow().search.hits.len();
        let status = if done < total {
            format!("Searching… {n} found")
        } else {
            match n {
                0 => "No matches".to_string(),
                1 => "1 match".to_string(),
                n => format!("{n} matches"),
            }
        };
        self.ui.set_search_status(status.into());
        if first_hit {
            self.select_hit(0);
        } else {
            self.update_view();
        }
    }

    pub fn search_hits(&self) -> usize {
        self.st.borrow().search.hits.len()
    }

    pub fn search_finished(&self) -> bool {
        self.st.borrow().search.finished
    }

    pub fn current_hit(&self) -> Option<usize> {
        self.st.borrow().search.current
    }

    pub(crate) fn step_search(&self, delta: i64) {
        let (n, cur) = {
            let st = self.st.borrow();
            (st.search.hits.len(), st.search.current)
        };
        if n == 0 {
            return;
        }
        let next = match cur {
            None => 0,
            Some(c) => (c as i64 + delta).rem_euclid(n as i64) as usize,
        };
        self.select_hit(next);
    }

    /// Makes `index` the current match and scrolls it into view.
    pub(crate) fn select_hit(&self, index: usize) {
        let target = {
            let mut st = self.st.borrow_mut();
            let Some(hit) = st.search.hits.get(index).cloned() else { return };
            st.search.current = Some(index);
            st.current_page = hit.page;
            hit
        };
        for row in 0..self.search_model.row_count() {
            let mut r = self.search_model.row_data(row).unwrap();
            let sel = r.id as usize == index;
            if r.selected != sel {
                r.selected = sel;
                self.search_model.set_row_data(row, r);
            }
        }
        let single = self.st.borrow().mode == DisplayMode::SinglePage;
        if single {
            self.go_to_page(target.page);
        }
        // Scroll so the match sits a third of the way down the viewport.
        let (y, x) = {
            let st = self.st.borrow();
            let Some(first) = target.rects.first() else { return };
            let v = st.layout.rect_to_view(target.page, *first);
            (v.y, v.x)
        };
        let vp = self.viewport();
        let sx = self.scroll().x;
        let nx = if x < sx || x > sx + vp.w - 40.0 { (x - vp.w / 3.0).max(0.0) } else { sx };
        self.ui.invoke_scroll_canvas_to(nx, (y - vp.h / 3.0).max(0.0));
        self.update_view();
    }
}

/// Searches `SEARCH_CHUNK` pages from `from`, posts the hits, then queues the next
/// chunk. Stale generations (a newer search started) stop the chain.
fn search_chunk(
    engine: Engine,
    win: u64,
    doc_id: DocId,
    query: String,
    from: usize,
    total: usize,
    generation: Generation,
) {
    let next_engine = engine.clone();
    engine.submit(Priority::Thumbnail, Some(generation.clone()), move |s| {
        let Some(doc) = s.doc(doc_id) else { return };
        let to = (from + SEARCH_CHUNK).min(total);
        let mut hits = Vec::new();
        for page in from..to {
            hits.extend(text::search_page(doc, page, &query, SearchOptions::default()));
        }
        let q = query.clone();
        let _ = slint::invoke_from_event_loop(move || {
            super::super::with_window(win, |w| w.on_search_chunk(doc_id, &q, to, total, hits));
        });
        if to < total {
            search_chunk(next_engine, win, doc_id, query, to, total, generation);
        }
    });
}
