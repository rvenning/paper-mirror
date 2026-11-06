//! Copies of original files taken before Paper Mirror first overwrites them in a
//! session, so File > Revert To > Original can restore them. Bounded by count and size.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_ENTRIES: usize = 30;
const MAX_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Default, Serialize, Deserialize)]
struct Index {
    /// Normalised source path -> (backup file name, unix seconds).
    entries: BTreeMap<String, (String, u64)>,
}

fn dir() -> PathBuf {
    super::data_dir().join("Originals")
}

fn load_index(dir: &Path) -> Index {
    std::fs::read(dir.join("index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_index(dir: &Path, idx: &Index) -> std::io::Result<()> {
    super::write_atomic(&dir.join("index.json"), &serde_json::to_vec_pretty(idx).map_err(std::io::Error::other)?)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Copies `path` into the originals store unless a copy of it already exists.
pub fn backup(path: &Path) -> std::io::Result<PathBuf> {
    backup_in(&dir(), path)
}

pub fn backup_in(dir: &Path, path: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut idx = load_index(dir);
    let key = super::settings::normalise(path);
    if let Some((file, _)) = idx.entries.get(&key) {
        let p = dir.join(file);
        if p.is_file() {
            return Ok(p);
        }
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let file = format!("{}-{}", now(), name);
    let dest = dir.join(&file);
    std::fs::copy(path, &dest)?;
    idx.entries.insert(key, (file, now()));
    prune(dir, &mut idx);
    save_index(dir, &idx)?;
    Ok(dest)
}

/// The stored original for `path`, if any.
pub fn original_for(path: &Path) -> Option<PathBuf> {
    original_in(&dir(), path)
}

pub fn original_in(dir: &Path, path: &Path) -> Option<PathBuf> {
    let idx = load_index(dir);
    let (file, _) = idx.entries.get(&super::settings::normalise(path))?;
    Some(dir.join(file)).filter(|p| p.is_file())
}

fn prune(dir: &Path, idx: &mut Index) {
    loop {
        let total: u64 =
            idx.entries.values().filter_map(|(f, _)| std::fs::metadata(dir.join(f)).ok()).map(|m| m.len()).sum();
        if idx.entries.len() <= MAX_ENTRIES && total <= MAX_BYTES {
            break;
        }
        let Some(oldest) = idx.entries.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone()) else { break };
        if let Some((f, _)) = idx.entries.remove(&oldest) {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_up_once_and_finds_it() {
        let root = std::env::temp_dir().join(format!("pm-orig-{}", std::process::id()));
        let store = root.join("store");
        std::fs::create_dir_all(&root).unwrap();
        let doc = root.join("a.pdf");
        std::fs::write(&doc, b"v1").unwrap();
        let b1 = backup_in(&store, &doc).unwrap();
        std::fs::write(&doc, b"v2").unwrap();
        let b2 = backup_in(&store, &doc).unwrap();
        assert_eq!(b1, b2, "the first original is kept");
        assert_eq!(std::fs::read(original_in(&store, &doc).unwrap()).unwrap(), b"v1");
        assert!(original_in(&store, &root.join("other.pdf")).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
