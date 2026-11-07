//! The Inspector panel: file and document information.

use super::DocWindow;
use crate::pdf::engine::Priority;
use crate::pdf::outline;
use crate::ui::InfoRow;

fn row(key: &str, value: impl Into<String>) -> InfoRow {
    InfoRow { key: key.into(), value: value.into().into(), heading: false }
}

fn heading(key: &str) -> InfoRow {
    InfoRow { key: key.into(), value: Default::default(), heading: true }
}

/// Formats a byte count the way File Explorer does (KB = 1024 bytes).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {} ({bytes} bytes)", UNITS[u])
}

/// Page size in points as inches and millimetres, e.g. "8.50 × 11.00 in (215.9 × 279.4 mm)".
pub fn format_page_size(w: f32, h: f32) -> String {
    format!("{:.2} × {:.2} in ({:.1} × {:.1} mm)", w / 72.0, h / 72.0, w / 72.0 * 25.4, h / 72.0 * 25.4)
}

impl DocWindow {
    pub(crate) fn toggle_inspector(&self) {
        let on = !self.ui.get_inspector_visible();
        self.ui.set_inspector_visible(on);
        if on {
            self.refresh_inspector();
        }
    }

    pub(crate) fn refresh_inspector(&self) {
        let Some((doc_id, path, title)) =
            self.st.borrow().doc.as_ref().map(|d| (d.id, d.path.clone(), d.title.clone()))
        else {
            return;
        };
        let win = self.id;
        self.engine.submit(Priority::Interactive, None, move |s| {
            let Some(doc) = s.doc(doc_id) else { return };
            let info = outline::info(doc);
            let _ = slint::invoke_from_event_loop(move || {
                super::super::with_window(win, |w| {
                    let mut rows = vec![heading("General"), row("Name", title)];
                    if let Some(p) = &path {
                        if let Some(dir) = p.parent() {
                            rows.push(row("Where", dir.display().to_string()));
                        }
                        if let Ok(meta) = std::fs::metadata(p) {
                            rows.push(row("Size", format_size(meta.len())));
                            if meta.permissions().readonly() {
                                rows.push(row("Locked", "Yes (read-only)"));
                            }
                        }
                    }
                    let image_rows = w.image_info_rows();
                    if !image_rows.is_empty() {
                        for (k, v, h) in image_rows {
                            rows.push(if h { heading(&k) } else { row(&k, v) });
                        }
                        w.info_model.set_vec(rows);
                        return;
                    }
                    rows.push(row("Pages", info.pages.to_string()));
                    rows.push(row("Page size", format_page_size(info.first_page_size.0, info.first_page_size.1)));
                    rows.push(row("PDF version", info.version.clone()));
                    rows.push(heading("Document"));
                    for (k, v) in [
                        ("Title", &info.title),
                        ("Author", &info.author),
                        ("Subject", &info.subject),
                        ("Keywords", &info.keywords),
                        ("Created", &info.created),
                        ("Modified", &info.modified),
                        ("Creator", &info.creator),
                        ("Producer", &info.producer),
                    ] {
                        if !v.is_empty() {
                            rows.push(row(k, v.clone()));
                        }
                    }
                    rows.push(heading("Security"));
                    rows.push(row("Encryption", info.encryption.clone()));
                    for (k, allowed) in &info.permissions {
                        rows.push(row(k, if *allowed { "Allowed" } else { "Not allowed" }));
                    }
                    w.info_model.set_vec(rows);
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_format_like_explorer() {
        assert_eq!(format_size(512), "512 bytes");
        assert_eq!(format_size(2048), "2.0 KB (2048 bytes)");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB (5242880 bytes)");
        assert_eq!(format_page_size(612.0, 792.0), "8.50 × 11.00 in (215.9 × 279.4 mm)");
    }
}
