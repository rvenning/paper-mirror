//! The command registry: every user-invokable action with its menu, title and
//! keyboard shortcut. Menus live in `ui/main.slint`; a test keeps the two in sync, and
//! Help > Keyboard Shortcuts is generated from this table.
//!
//! Shortcuts follow macOS Preview's bindings translated to Windows conventions
//! (Command -> Ctrl, Option -> Alt).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub id: &'static str,
    pub menu: &'static str,
    pub title: &'static str,
    /// Display form of the shortcut, e.g. "Ctrl+Shift+A".
    pub shortcut: Option<&'static str>,
}

const fn c(id: &'static str, menu: &'static str, title: &'static str, shortcut: Option<&'static str>) -> CommandInfo {
    CommandInfo { id, menu, title, shortcut }
}

pub const COMMANDS: &[CommandInfo] = &[
    // File
    c("open", "File", "Open…", Some("Ctrl+O")),
    c("clear-recent", "File", "Clear Menu", None),
    c("close", "File", "Close Window", Some("Ctrl+W")),
    c("show-in-explorer", "File", "Show in File Explorer", None),
    // View
    c("toggle-sidebar", "View", "Show/Hide Sidebar", Some("Ctrl+Alt+S")),
    c("sidebar-thumbnails", "View", "Thumbnails", Some("Ctrl+Alt+2")),
    c("view-continuous", "View", "Continuous Scroll", Some("Ctrl+1")),
    c("view-single", "View", "Single Page", Some("Ctrl+2")),
    c("view-two-pages", "View", "Two Pages", Some("Ctrl+3")),
    c("zoom-actual", "View", "Actual Size", Some("Ctrl+0")),
    c("zoom-fit-page", "View", "Zoom to Fit", Some("Ctrl+9")),
    c("zoom-fit-width", "View", "Zoom to Width", None),
    c("zoom-in", "View", "Zoom In", Some("Ctrl+=")),
    c("zoom-out", "View", "Zoom Out", Some("Ctrl+-")),
    c("appearance-system", "View", "Appearance: Use System Setting", None),
    c("appearance-light", "View", "Appearance: Light", None),
    c("appearance-dark", "View", "Appearance: Dark", None),
    // Go
    c("page-prev", "Go", "Previous Page", Some("Ctrl+Alt+Up")),
    c("page-next", "Go", "Next Page", Some("Ctrl+Alt+Down")),
    c("page-first", "Go", "First Page", Some("Home")),
    c("page-last", "Go", "Last Page", Some("End")),
    c("go-to-page", "Go", "Go to Page…", Some("Ctrl+Alt+G")),
    // Window
    c("minimize", "Window", "Minimize", None),
    c("maximize", "Window", "Zoom", None),
    // Help
    c("shortcuts", "Help", "Keyboard Shortcuts", Some("F1")),
    c("about", "Help", "About Paper Mirror", None),
    c("show-notices", "Help", "Third-party Notices", None),
];

pub fn find(id: &str) -> Option<&'static CommandInfo> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// Rows for the keyboard-shortcut reference: (menu, title, shortcut).
pub fn shortcut_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    COMMANDS.iter().filter_map(|c| c.shortcut.map(|s| (c.menu, c.title, s))).collect()
}

/// Converts a Slint `@keys(...)` body such as `Control + Shift + "A"` into the display
/// form used here ("Ctrl+Shift+A").
pub fn slint_keys_to_display(body: &str) -> String {
    body.split('+')
        .map(|p| {
            let p = p.trim().trim_matches('"');
            match p {
                "Control" => "Ctrl".to_string(),
                "UpArrow" => "Up".to_string(),
                "DownArrow" => "Down".to_string(),
                "LeftArrow" => "Left".to_string(),
                "RightArrow" => "Right".to_string(),
                // Slint requires lower-case key literals; display them upper-case.
                other if other.chars().count() == 1 => other.to_uppercase(),
                other => other.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_and_shortcuts_are_unique() {
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        for c in COMMANDS {
            assert!(ids.insert(c.id), "duplicate command id {}", c.id);
            if let Some(k) = c.shortcut {
                assert!(keys.insert(k), "shortcut {k} bound twice");
            }
        }
    }

    /// Every `command("...")` in the Slint UI must be registered, and every menu
    /// shortcut must match the registry so the in-app reference is accurate.
    #[test]
    fn ui_menus_match_registry() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut used = HashSet::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "slint") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            for (i, _) in src.match_indices("command(\"") {
                let rest = &src[i + 9..];
                let id = &rest[..rest.find('"').unwrap()];
                assert!(find(id).is_some(), "{}: command {id:?} is not in the registry", path.display());
                used.insert(id.to_string());
            }
            // MenuItem { title: ...; shortcut: @keys(...); activated => { root.command("id"); } }
            for line in src.lines().filter(|l| l.contains("MenuItem") && l.contains("@keys(")) {
                let keys = &line[line.find("@keys(").unwrap() + 6..];
                let keys = &keys[..keys.find(')').unwrap()];
                let Some(ci) = line.find("command(\"") else { continue };
                let id = &line[ci + 9..];
                let id = &id[..id.find('"').unwrap()];
                let reg = find(id).unwrap();
                assert_eq!(reg.shortcut, Some(slint_keys_to_display(keys).as_str()), "shortcut mismatch for {id}");
            }
        }
        for c in COMMANDS {
            assert!(used.contains(c.id), "registered command {} is not reachable from the UI", c.id);
        }
    }

    #[test]
    fn keys_display_conversion() {
        assert_eq!(slint_keys_to_display(r#"Control + Shift + "a""#), "Ctrl+Shift+A");
        assert_eq!(slint_keys_to_display("Control + Alt + UpArrow"), "Ctrl+Alt+Up");
    }
}
