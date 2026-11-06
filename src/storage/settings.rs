//! User preferences and the recent-documents list.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MAX_RECENT: usize = 15;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "system", "light" or "dark".
    pub appearance: String,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    /// "continuous", "single" or "two-pages".
    pub display_mode: String,
    pub recent: Vec<PathBuf>,
    /// Per-document page bookmarks, keyed by absolute path.
    pub bookmarks: BTreeMap<String, Vec<usize>>,
    /// Show photo location metadata without asking each time.
    pub show_location: bool,
    pub markup_color: String,
    pub markup_line_width: f32,
    pub markup_font_size: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            appearance: "system".into(),
            sidebar_visible: true,
            sidebar_width: 170.0,
            display_mode: "continuous".into(),
            recent: Vec::new(),
            bookmarks: BTreeMap::new(),
            show_location: false,
            markup_color: "#e5484d".into(),
            markup_line_width: 2.0,
            markup_font_size: 14.0,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        super::data_dir().join("settings.json")
    }

    /// Loads settings; a missing or corrupt file yields defaults (a corrupt file is
    /// kept aside as `settings.json.bad` rather than silently discarded).
    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(mut s) => {
                    s.sanitise();
                    s
                }
                Err(_) => {
                    let _ = std::fs::rename(path, path.with_extension("json.bad"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        super::write_atomic(path, &bytes)
    }

    fn sanitise(&mut self) {
        if !["system", "light", "dark"].contains(&self.appearance.as_str()) {
            self.appearance = "system".into();
        }
        if !["continuous", "single", "two-pages"].contains(&self.display_mode.as_str()) {
            self.display_mode = "continuous".into();
        }
        if !self.sidebar_width.is_finite() {
            self.sidebar_width = 170.0;
        }
        self.sidebar_width = self.sidebar_width.clamp(120.0, 420.0);
        if !(0.25..=30.0).contains(&self.markup_line_width) {
            self.markup_line_width = 2.0;
        }
        if !(4.0..=144.0).contains(&self.markup_font_size) {
            self.markup_font_size = 14.0;
        }
        self.recent.truncate(MAX_RECENT);
    }

    /// Moves `path` to the top of the recent list.
    pub fn add_recent(&mut self, path: &Path) {
        let key = normalise(path);
        self.recent.retain(|p| normalise(p) != key);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, path: &Path) {
        let key = normalise(path);
        self.recent.retain(|p| normalise(p) != key);
    }

    pub fn bookmarks_for(&self, path: &Path) -> Vec<usize> {
        self.bookmarks.get(&normalise(path)).cloned().unwrap_or_default()
    }

    pub fn set_bookmarks(&mut self, path: &Path, pages: Vec<usize>) {
        let key = normalise(path);
        if pages.is_empty() {
            self.bookmarks.remove(&key);
        } else {
            self.bookmarks.insert(key, pages);
        }
    }
}

/// Case-insensitive comparison key for Windows paths.
pub fn normalise(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    abs.to_string_lossy().replace('/', "\\").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pm-settings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("settings.json")
    }

    #[test]
    fn round_trip() {
        let p = tmp("rt");
        let mut s = Settings { appearance: "dark".into(), ..Default::default() };
        s.add_recent(Path::new("C:/docs/a.pdf"));
        s.set_bookmarks(Path::new("C:/docs/a.pdf"), vec![3, 7]);
        s.save_to(&p).unwrap();
        let back = Settings::load_from(&p);
        assert_eq!(back, s);
        assert_eq!(back.bookmarks_for(Path::new("c:\\DOCS\\a.pdf")), vec![3, 7]);
    }

    #[test]
    fn corrupt_file_falls_back_and_is_kept_aside() {
        let p = tmp("bad");
        std::fs::write(&p, b"{ not json").unwrap();
        assert_eq!(Settings::load_from(&p), Settings::default());
        assert!(p.with_extension("json.bad").exists());
    }

    #[test]
    fn invalid_values_are_sanitised() {
        let p = tmp("sanitise");
        std::fs::write(&p, br#"{"appearance":"purple","sidebar_width":99999,"display_mode":"x"}"#).unwrap();
        let s = Settings::load_from(&p);
        assert_eq!(s.appearance, "system");
        assert_eq!(s.sidebar_width, 420.0);
        assert_eq!(s.display_mode, "continuous");
    }

    #[test]
    fn recent_is_deduplicated_case_insensitively_and_bounded() {
        let mut s = Settings::default();
        for i in 0..20 {
            s.add_recent(Path::new(&format!("C:/d/{i}.pdf")));
        }
        assert_eq!(s.recent.len(), MAX_RECENT);
        s.add_recent(Path::new("c:\\D\\5.PDF"));
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], PathBuf::from("c:\\D\\5.PDF"));
        assert_eq!(s.recent.iter().filter(|p| normalise(p) == normalise(Path::new("C:/d/5.pdf"))).count(), 1);
        s.remove_recent(Path::new("C:/d/5.pdf"));
        assert!(s.recent.iter().all(|p| normalise(p) != normalise(Path::new("C:/d/5.pdf"))));
    }
}
