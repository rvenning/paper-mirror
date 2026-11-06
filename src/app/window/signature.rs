//! Signatures: placing a stored signature on the page. Creation and protected
//! storage are added with the signatures milestone.

use super::DocWindow;
use crate::core::geometry::Point;

impl DocWindow {
    pub(crate) fn place_signature(&self, _page: usize, _at: Point) {
        self.error("No signature yet.", "Create a signature first with Tools > Sign > Manage Signatures.");
    }
}
