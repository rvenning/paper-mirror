//! The per-user installer (`paper-mirror-setup.exe`): no administrator rights, no
//! services, nothing outside the user's profile. The setup program carries its files
//! as a payload appended to its own executable:
//!
//! `[stub exe][entries][sha256 of entries: 32][entries offset: u64][count: u32]["PMSETUP2"]`
//!
//! where each entry is `[name length: u32][name: UTF-8][size: u64][compressed size: u64]`
//! followed by the file, raw-deflate compressed. A signed setup also ends with an
//! Authenticode certificate table after the footer (see [`signature`]). The payload may
//! carry its own (signed) uninstaller; otherwise the setup stub becomes the uninstaller.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::platform_windows::assoc;

pub const MAGIC: &[u8; 8] = b"PMSETUP2";
const FOOTER: usize = 32 + 8 + 4 + 8;
pub const APP_EXE: &str = "paper-mirror.exe";
pub const UNINSTALLER: &str = "Uninstall Paper Mirror.exe";
const MANIFEST: &str = "install-manifest.txt";
const SHORTCUT: &str = "Paper Mirror.lnk";
const UNINSTALL_KEY: &str = r"Microsoft\Windows\CurrentVersion\Uninstall\PaperMirror";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
    let mut out = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            std::ptr::null(),
            0,
            data.as_ptr(),
            data.len() as u32,
            out.as_mut_ptr(),
            32,
        )
    };
    if status == 0 { Ok(out) } else { Err(format!("SHA-256 failed (0x{status:08x})")) }
}

/// The Authenticode signature of a PE file, if it has one: the file offset of the
/// certificate table, and where in the headers its directory entry sits. Signing
/// appends the table at the end of the file, after the payload, and records it in the
/// optional header's security directory (data directory 4, which holds a file offset
/// rather than an address).
pub fn signature(bytes: &[u8]) -> Option<(usize, usize)> {
    let u16_at = |i: usize| Some(u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?) as usize);
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(0x3C)?;
    if bytes.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let opt = pe + 24;
    let (count_at, dirs) = match u16_at(opt)? {
        0x10b => (opt + 92, opt + 96),
        0x20b => (opt + 108, opt + 112),
        _ => return None,
    };
    if u32_at(count_at)? < 5 {
        return None;
    }
    let entry = dirs + 4 * 8;
    let (start, size) = (u32_at(entry)?, u32_at(entry + 4)?);
    (start != 0 && size != 0 && start <= bytes.len() && start + size <= bytes.len()).then_some((start, entry))
}

/// The file without its Authenticode signature: the certificate table is cut off
/// and its directory entry cleared, so the bytes are a valid unsigned program.
fn unsigned(mut bytes: Vec<u8>) -> Vec<u8> {
    if let Some((start, entry)) = signature(&bytes) {
        bytes.truncate(start);
        bytes[entry..entry + 8].fill(0);
    }
    bytes
}

/// The executable without any payload or signature (`exe` may carry either).
pub fn stub_bytes(exe: &Path) -> Result<Vec<u8>, String> {
    let bytes = unsigned(std::fs::read(exe).map_err(|e| e.to_string())?);
    let len = match footer(&bytes) {
        Some((offset, _, _, end)) if offset as usize <= end - FOOTER => offset as usize,
        _ => bytes.len(),
    };
    Ok(bytes[..len].to_vec())
}

/// Finds the payload footer: at the end of the file, or, in a signed setup, just
/// before the certificate table (signing pads the file to 8 bytes first). Returns
/// (payload offset, entry count, checksum, footer end).
fn footer(bytes: &[u8]) -> Option<(u64, u32, [u8; 32], usize)> {
    let end = signature(bytes).map_or(bytes.len(), |(start, _)| start);
    let end = (end.saturating_sub(7)..=end)
        .rev()
        .find(|&e| e >= FOOTER && &bytes[e - 8..e] == MAGIC && bytes[e..end].iter().all(|&b| b == 0))?;
    let f = &bytes[end - FOOTER..end];
    let hash: [u8; 32] = f[..32].try_into().ok()?;
    let offset = u64::from_le_bytes(f[32..40].try_into().ok()?);
    let count = u32::from_le_bytes(f[40..44].try_into().ok()?);
    Some((offset, count, hash, end))
}

/// Writes `out` = stub of `setup_exe` + the files as a payload.
pub fn pack(setup_exe: &Path, files: &[(String, PathBuf)], out: &Path) -> Result<(), String> {
    let mut bytes = stub_bytes(setup_exe)?;
    let offset = bytes.len() as u64;
    let mut payload = Vec::new();
    for (name, path) in files {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        payload.extend_from_slice(&(name.len() as u32).to_le_bytes());
        payload.extend_from_slice(name.as_bytes());
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(&data).map_err(|e| e.to_string())?;
        let packed = z.finish().map_err(|e| e.to_string())?;
        payload.extend_from_slice(&(data.len() as u64).to_le_bytes());
        payload.extend_from_slice(&(packed.len() as u64).to_le_bytes());
        payload.extend_from_slice(&packed);
    }
    let hash = sha256(&payload)?;
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(&hash);
    bytes.extend_from_slice(&offset.to_le_bytes());
    bytes.extend_from_slice(&(files.len() as u32).to_le_bytes());
    bytes.extend_from_slice(MAGIC);
    std::fs::write(out, bytes).map_err(|e| e.to_string())
}

/// Reads and verifies the payload of `exe`. Ok(None) means there is none.
pub fn read_payload(exe: &Path) -> Result<Option<Vec<Entry>>, String> {
    let bytes = std::fs::read(exe).map_err(|e| e.to_string())?;
    let Some((offset, count, hash, end)) = footer(&bytes) else { return Ok(None) };
    if offset as usize > end - FOOTER {
        return Err("The setup file is damaged.".into());
    }
    let payload = &bytes[offset as usize..end - FOOTER];
    if sha256(payload)? != hash {
        return Err("The setup file is damaged (checksum mismatch). Download it again.".into());
    }
    let mut entries = Vec::new();
    let mut p = payload;
    for _ in 0..count {
        let take = |p: &mut &[u8], n: usize| -> Result<Vec<u8>, String> {
            if p.len() < n {
                return Err("The setup file is damaged.".into());
            }
            let (a, b) = p.split_at(n);
            *p = b;
            Ok(a.to_vec())
        };
        let n = u32::from_le_bytes(take(&mut p, 4)?.try_into().unwrap()) as usize;
        let name = String::from_utf8(take(&mut p, n)?).map_err(|_| "The setup file is damaged.")?;
        // Plain file names only: nothing may be written outside the install folder.
        if name.is_empty() || name.contains(['/', '\\', ':']) || name == ".." || name == "." {
            return Err("The setup file is damaged.".into());
        }
        let size = u64::from_le_bytes(take(&mut p, 8)?.try_into().unwrap()) as usize;
        let packed_size = u64::from_le_bytes(take(&mut p, 8)?.try_into().unwrap()) as usize;
        let packed = take(&mut p, packed_size)?;
        let mut data = Vec::with_capacity(size.min(1 << 30));
        flate2::read::DeflateDecoder::new(&packed[..])
            .take(size as u64)
            .read_to_end(&mut data)
            .map_err(|_| "The setup file is damaged.")?;
        if data.len() != size {
            return Err("The setup file is damaged.".into());
        }
        entries.push(Entry { name, data });
    }
    Ok(Some(entries))
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Install folder; default `%LOCALAPPDATA%\Programs\Paper Mirror`.
    pub dir: PathBuf,
    /// Folder for the Start menu shortcut (None: no shortcut).
    pub shortcut_dir: Option<PathBuf>,
    /// Register file types (opt-in).
    pub associate: bool,
    /// Registry base under HKEY_CURRENT_USER ("Software"; tests use a scratch key).
    pub reg_base: String,
}

impl Options {
    pub fn defaults() -> Options {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let start =
            std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs"));
        Options {
            dir: local.join("Programs").join("Paper Mirror"),
            shortcut_dir: start,
            associate: false,
            reg_base: assoc::BASE.into(),
        }
    }
}

fn uninstall_key(base: &str) -> String {
    format!(r"{base}\{UNINSTALL_KEY}")
}

/// Installs (or upgrades in place). `stub` becomes the uninstaller.
pub fn install(entries: &[Entry], stub: &[u8], opts: &Options) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&opts.dir)
        .map_err(|e| format!("The folder {} couldn’t be created: {e}", opts.dir.display()))?;
    let mut names: Vec<String> = Vec::new();
    let mut files: Vec<(&str, &[u8])> = entries.iter().map(|e| (e.name.as_str(), e.data.as_slice())).collect();
    if !entries.iter().any(|e| e.name == UNINSTALLER) {
        files.push((UNINSTALLER, stub));
    }
    for (name, data) in files {
        let target = opts.dir.join(name);
        let tmp = opts.dir.join(format!("{name}.new"));
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(data).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        drop(f);
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Paper Mirror is running. Close every Paper Mirror window and try again.".into()
            } else {
                format!("{name} couldn’t be installed: {e}")
            });
        }
        names.push(name.to_string());
    }
    // An update removes files the previous version installed that this one doesn't ship.
    let previous = std::fs::read_to_string(opts.dir.join(MANIFEST)).unwrap_or_default();
    for old in previous.lines().map(str::trim).filter(|n| !n.is_empty() && !n.contains(['/', '\\', ':'])) {
        if !names.iter().any(|n| n == old) {
            let _ = std::fs::remove_file(opts.dir.join(old));
        }
    }
    std::fs::write(opts.dir.join(MANIFEST), names.join("\n")).map_err(|e| e.to_string())?;
    let exe = opts.dir.join(APP_EXE);
    if let Some(dir) = &opts.shortcut_dir {
        create_shortcut(&dir.join(SHORTCUT), &exe, &opts.dir)?;
    }
    let key = uninstall_key(&opts.reg_base);
    let uninstaller = opts.dir.join(UNINSTALLER);
    let size_kb: u64 = entries.iter().map(|e| e.data.len() as u64).sum::<u64>() / 1024;
    assoc::set_string(&key, Some("DisplayName"), "Paper Mirror")?;
    assoc::set_string(&key, Some("DisplayVersion"), crate::VERSION)?;
    assoc::set_string(&key, Some("Publisher"), "Paper Mirror")?;
    assoc::set_string(&key, Some("DisplayIcon"), &format!("\"{}\",0", exe.display()))?;
    assoc::set_string(&key, Some("InstallLocation"), &opts.dir.display().to_string())?;
    assoc::set_string(&key, Some("UninstallString"), &format!("\"{}\" --uninstall", uninstaller.display()))?;
    assoc::set_string(
        &key,
        Some("QuietUninstallString"),
        &format!("\"{}\" --uninstall --silent", uninstaller.display()),
    )?;
    assoc::set_dword(&key, "NoModify", 1)?;
    assoc::set_dword(&key, "NoRepair", 1)?;
    assoc::set_dword(&key, "EstimatedSize", size_kb.min(u32::MAX as u64) as u32)?;
    if opts.associate {
        assoc::register(&opts.reg_base, &exe)?;
    }
    Ok(exe)
}

/// Removes the installation: files listed in the manifest, the shortcut, the
/// Apps & features entry and any file-type registration. User data is kept.
pub fn uninstall(opts: &Options) -> Result<(), String> {
    assoc::unregister(&opts.reg_base);
    assoc::delete_tree(&uninstall_key(&opts.reg_base));
    if let Some(dir) = &opts.shortcut_dir {
        let _ = std::fs::remove_file(dir.join(SHORTCUT));
    }
    let manifest = std::fs::read_to_string(opts.dir.join(MANIFEST)).unwrap_or_default();
    let me = std::env::current_exe().ok().and_then(|p| std::fs::canonicalize(p).ok());
    let mut leftover = None;
    for name in manifest.lines().map(str::trim).filter(|n| !n.is_empty() && !n.contains(['/', '\\', ':'])) {
        let path = opts.dir.join(name);
        if me.is_some() && std::fs::canonicalize(&path).ok() == me {
            leftover = Some(path);
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Paper Mirror is running. Close every Paper Mirror window and try again.".into()
            } else {
                format!("{name} couldn’t be removed: {e}")
            });
        }
    }
    let _ = std::fs::remove_file(opts.dir.join(MANIFEST));
    match leftover {
        // A running program can't delete itself: a short-lived shell finishes the job.
        Some(path) => delete_after_exit(&path, &opts.dir),
        None => {
            let _ = std::fs::remove_dir(&opts.dir);
        }
    }
    Ok(())
}

fn delete_after_exit(file: &Path, dir: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Retried: a just-deleted file can stay "delete pending" (e.g. while antivirus
    // scans it), and the folder can't be removed until it is gone.
    let (f, d) = (file.display(), dir.display());
    let script = format!(
        "ping -n 3 127.0.0.1 >nul & for /l %i in (1,1,10) do @(del /f /q \"{f}\" 2>nul & rmdir \"{d}\" 2>nul & if exist \"{d}\" ping -n 2 127.0.0.1 >nul)"
    );
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(format!("/d /c {script}"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

fn create_shortcut(lnk: &Path, target: &Path, workdir: &Path) -> Result<(), String> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};
    let err = |e: windows::core::Error| format!("The Start menu shortcut couldn’t be created: {}", e.message());
    if let Some(dir) = lnk.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        link.SetPath(&HSTRING::from(target.as_os_str())).map_err(err)?;
        link.SetWorkingDirectory(&HSTRING::from(workdir.as_os_str())).map_err(err)?;
        link.SetDescription(&HSTRING::from("View and mark up PDF documents and images")).map_err(err)?;
        link.SetIconLocation(&HSTRING::from(target.as_os_str()), 0).map_err(err)?;
        let file: IPersistFile = link.cast().map_err(err)?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true).map_err(err)?;
    }
    Ok(())
}

pub fn is_installed(reg_base: &str) -> bool {
    assoc::get_string(&uninstall_key(reg_base), Some("DisplayName")).is_some()
}

pub fn install_location(reg_base: &str) -> Option<PathBuf> {
    assoc::get_string(&uninstall_key(reg_base), Some("InstallLocation")).map(PathBuf::from)
}
