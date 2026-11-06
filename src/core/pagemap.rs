//! Page operations as data, and how each one renumbers existing pages. The UI uses
//! the mapping to carry annotations, bookmarks and the current page across an edit.

use super::geometry::{Rect, Size};

#[derive(Clone, Debug, PartialEq)]
pub enum PageOp {
    /// Rotate pages clockwise by `quarter_turns` (negative: anticlockwise).
    Rotate {
        pages: Vec<usize>,
        quarter_turns: i32,
    },
    Delete {
        pages: Vec<usize>,
    },
    /// Move `pages` (in ascending order) so the block starts at `dest`, an index in
    /// the resulting document (PDFium's `FPDF_MovePages` semantics).
    Move {
        pages: Vec<usize>,
        dest: usize,
    },
    /// `count` new pages appear before old page `at` (or at the end).
    Insert {
        at: usize,
        count: usize,
        blank: Option<Size>,
    },
    /// Set the crop box of `page` to `rect` (page display space).
    Crop {
        page: usize,
        rect: Rect,
    },
}

fn sorted_unique(v: &[usize], n: usize) -> Vec<usize> {
    let mut v: Vec<usize> = v.iter().copied().filter(|p| *p < n).collect();
    v.sort_unstable();
    v.dedup();
    v
}

impl PageOp {
    /// Normalises page lists (sorted, unique, in range).
    pub fn normalised(&self, n: usize) -> PageOp {
        match self {
            PageOp::Rotate { pages, quarter_turns } => {
                PageOp::Rotate { pages: sorted_unique(pages, n), quarter_turns: *quarter_turns }
            }
            PageOp::Delete { pages } => PageOp::Delete { pages: sorted_unique(pages, n) },
            PageOp::Move { pages, dest } => {
                let pages = sorted_unique(pages, n);
                let dest = (*dest).min(n - pages.len().min(n));
                PageOp::Move { pages, dest }
            }
            PageOp::Insert { at, count, blank } => PageOp::Insert { at: (*at).min(n), count: *count, blank: *blank },
            other => other.clone(),
        }
    }

    /// For each old page, its index afterwards (`None` if deleted).
    pub fn remap(&self, n: usize) -> Vec<Option<usize>> {
        match self.normalised(n) {
            PageOp::Rotate { .. } | PageOp::Crop { .. } => (0..n).map(Some).collect(),
            PageOp::Delete { pages } => {
                let mut next = 0;
                (0..n)
                    .map(|i| {
                        if pages.binary_search(&i).is_ok() {
                            None
                        } else {
                            next += 1;
                            Some(next - 1)
                        }
                    })
                    .collect()
            }
            PageOp::Move { pages, dest } => {
                let rest: Vec<usize> = (0..n).filter(|i| pages.binary_search(i).is_err()).collect();
                let mut order: Vec<usize> = rest[..dest].to_vec();
                order.extend(&pages);
                order.extend(&rest[dest..]);
                let mut map = vec![None; n];
                for (new, old) in order.into_iter().enumerate() {
                    map[old] = Some(new);
                }
                map
            }
            PageOp::Insert { at, count, .. } => (0..n).map(|i| Some(if i >= at { i + count } else { i })).collect(),
        }
    }

    /// Page count afterwards.
    pub fn result_len(&self, n: usize) -> usize {
        match self.normalised(n) {
            PageOp::Delete { pages } => n - pages.len(),
            PageOp::Insert { count, .. } => n + count,
            _ => n,
        }
    }
}

/// Converts a drop position in the thumbnail list (a gap before old page `gap`,
/// `gap == n` meaning the end) into `FPDF_MovePages`' destination index.
pub fn drop_gap_to_dest(pages: &[usize], gap: usize) -> usize {
    gap - pages.iter().filter(|p| **p < gap).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_renumbers() {
        let op = PageOp::Delete { pages: vec![3, 1, 1, 9] };
        assert_eq!(op.remap(5), vec![Some(0), None, Some(1), None, Some(2)]);
        assert_eq!(op.result_len(5), 3);
    }

    #[test]
    fn move_block_to_front_and_end() {
        // Move pages 2 and 3 to the front.
        let op = PageOp::Move { pages: vec![3, 2], dest: 0 };
        assert_eq!(op.remap(5), vec![Some(2), Some(3), Some(0), Some(1), Some(4)]);
        // Move page 0 to the end.
        let op = PageOp::Move { pages: vec![0], dest: 4 };
        assert_eq!(op.remap(5), vec![Some(4), Some(0), Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn drop_gap_conversion() {
        // Dragging page 0 into the gap before page 3 lands it at index 2.
        assert_eq!(drop_gap_to_dest(&[0], 3), 2);
        let op = PageOp::Move { pages: vec![0], dest: drop_gap_to_dest(&[0], 3) };
        assert_eq!(op.remap(4), vec![Some(2), Some(0), Some(1), Some(3)]);
        // Dropping at the end.
        assert_eq!(drop_gap_to_dest(&[1, 2], 4), 2);
    }

    #[test]
    fn insert_shifts_following_pages() {
        let op = PageOp::Insert { at: 2, count: 3, blank: None };
        assert_eq!(op.remap(4), vec![Some(0), Some(1), Some(5), Some(6)]);
        assert_eq!(op.result_len(4), 7);
        let end = PageOp::Insert { at: 99, count: 1, blank: None };
        assert_eq!(end.remap(2), vec![Some(0), Some(1)]);
    }

    #[test]
    fn rotate_and_crop_keep_numbers() {
        assert_eq!(PageOp::Rotate { pages: vec![1], quarter_turns: 1 }.remap(3), vec![Some(0), Some(1), Some(2)]);
    }
}
