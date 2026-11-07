//! M8: the per-user installer. Packs a setup file, verifies its payload, installs
//! silently into a temporary folder with a scratch registry key, then uninstalls.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use paper_mirror::installer::{self, APP_EXE, UNINSTALLER};
use paper_mirror::platform_windows::assoc;

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn pack_install_and_uninstall() {
    let setup = PathBuf::from(env!("CARGO_BIN_EXE_paper-mirror-setup"));
    let root = std::env::temp_dir().join(format!("pm-installer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let reg_base = format!(r"Software\PaperMirrorTest{}", std::process::id());

    // Payload files (a stand-in program; it is never run).
    let app = root.join("app.bin");
    std::fs::write(&app, b"MZ fake program").unwrap();
    let notices = root.join("notices.txt");
    std::fs::write(&notices, "Third-party notices\n".repeat(100)).unwrap();
    let packed = root.join("PaperMirror-setup.exe");
    let status = Command::new(&setup)
        .arg("--pack")
        .arg(&packed)
        .arg(format!("{APP_EXE}={}", app.display()))
        .arg(format!("THIRD-PARTY-NOTICES.txt={}", notices.display()))
        .status()
        .unwrap();
    assert!(status.success());

    // The payload reads back and is checksummed.
    let entries = installer::read_payload(&packed).unwrap().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, APP_EXE);
    assert_eq!(entries[0].data, b"MZ fake program");
    assert_eq!(installer::read_payload(&setup).unwrap(), None, "the plain stub has no payload");
    assert_eq!(installer::stub_bytes(&packed).unwrap(), std::fs::read(&setup).unwrap());
    let mut damaged = std::fs::read(&packed).unwrap();
    let i = installer::stub_bytes(&packed).unwrap().len() + 10;
    damaged[i] ^= 0xFF;
    let damaged_path = root.join("damaged.exe");
    std::fs::write(&damaged_path, damaged).unwrap();
    assert!(installer::read_payload(&damaged_path).unwrap_err().contains("damaged"));

    // Silent install with file types.
    let dir = root.join("Programs").join("Paper Mirror");
    let start = root.join("Start Menu");
    let common =
        [format!("--reg-base={reg_base}"), format!("--shortcut-dir={}", start.display()), "--silent".to_string()];
    let status = Command::new(&packed)
        .args(&common)
        .arg(format!("--dir={}", dir.display()))
        .arg("--associate")
        .status()
        .unwrap();
    assert!(status.success(), "install exited with {status}");
    assert_eq!(std::fs::read(dir.join(APP_EXE)).unwrap(), b"MZ fake program");
    assert!(dir.join("THIRD-PARTY-NOTICES.txt").is_file());
    assert!(dir.join(UNINSTALLER).is_file());
    assert!(start.join("Paper Mirror.lnk").is_file());
    assert!(installer::is_installed(&reg_base));
    assert_eq!(installer::install_location(&reg_base).as_deref(), Some(dir.as_path()));
    assert!(assoc::is_registered(&reg_base));
    let open = assoc::get_string(&format!(r"{reg_base}\Classes\PaperMirror.Image\shell\open\command"), None).unwrap();
    assert!(open.contains(&dir.join(APP_EXE).display().to_string()), "{open}");

    // Installing again (an update) replaces the files in place.
    std::fs::write(&app, b"MZ newer program").unwrap();
    let packed2 = root.join("PaperMirror-setup-2.exe");
    assert!(
        Command::new(&setup)
            .arg("--pack")
            .arg(&packed2)
            .arg(format!("{APP_EXE}={}", app.display()))
            .status()
            .unwrap()
            .success()
    );
    assert!(Command::new(&packed2).args(&common).arg(format!("--dir={}", dir.display())).status().unwrap().success());
    assert_eq!(std::fs::read(dir.join(APP_EXE)).unwrap(), b"MZ newer program");

    // Uninstall from the installed uninstaller (it removes even itself).
    let status = Command::new(dir.join(UNINSTALLER)).arg("--uninstall").args(&common).status().unwrap();
    assert!(status.success(), "uninstall exited with {status}");
    assert!(!installer::is_installed(&reg_base));
    assert!(!assoc::is_registered(&reg_base));
    assert!(!start.join("Paper Mirror.lnk").exists());
    assert!(!dir.join(APP_EXE).exists());
    wait_for("the install folder to disappear", || !dir.exists());
    assoc::delete_tree(&reg_base);
    let _ = std::fs::remove_dir_all(&root);
    let _: &Path = &root;
}
