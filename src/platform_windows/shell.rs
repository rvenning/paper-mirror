//! Shell integration: Explorer, bundled documents.

use std::os::windows::process::CommandExt;
use std::path::Path;

/// Opens File Explorer with `path` selected.
pub fn reveal_in_explorer(path: &Path) {
    // explorer.exe needs `/select,"<path>"` as one raw argument.
    let arg = format!("/select,\"{}\"", path.display());
    let _ = std::process::Command::new("explorer.exe").raw_arg(arg).spawn();
}

/// Shows THIRD-PARTY-NOTICES.txt from the installation folder in Notepad.
pub fn show_notices() {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let path = exe_dir.map(|d| d.join("THIRD-PARTY-NOTICES.txt"));
    match path.filter(|p| p.is_file()) {
        Some(p) => {
            let _ = std::process::Command::new("notepad.exe").arg(p).spawn();
        }
        None => {
            rfd::MessageDialog::new()
                .set_title("Third-party notices")
                .set_description("THIRD-PARTY-NOTICES.txt is missing from the installation folder.")
                .show();
        }
    }
}
