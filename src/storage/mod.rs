//! Local persistence: settings, recent documents, per-document bookmarks and the
//! protected signature store. Everything stays on this device.

pub mod settings;

use std::io::Write;
use std::path::{Path, PathBuf};

/// Directory for Paper Mirror's own data. Portable mode (a `portable.txt` file next to
/// the executable) keeps data in `<exe dir>\data`; otherwise `%APPDATA%\Paper Mirror`.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PAPER_MIRROR_DATA_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && dir.join("portable.txt").is_file()
    {
        return dir.join("data");
    }
    match std::env::var_os("APPDATA") {
        Some(appdata) => PathBuf::from(appdata).join("Paper Mirror"),
        None => std::env::temp_dir().join("Paper Mirror"),
    }
}

/// Writes `bytes` to `path` atomically: the data goes to a sibling temporary file
/// which then replaces the target in one rename, so a crash or full disk can never
/// leave a half-written file in place of the original.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // std::fs::rename uses MoveFileExW(MOVEFILE_REPLACE_EXISTING) on Windows.
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("pm-atomic-{}", std::process::id()));
        let p = dir.join("x.bin");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
