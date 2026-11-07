// Paper Mirror's per-user setup program. See `paper_mirror::installer`.
//
//   paper-mirror-setup.exe                      install, asking first
//   paper-mirror-setup.exe --silent [--associate] [--dir=<folder>] [--no-shortcut]
//   "Uninstall Paper Mirror.exe" --uninstall [--silent]
//   paper-mirror-setup.exe --pack <out.exe> <name>=<file>...   (release packaging)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use paper_mirror::installer::{self, Options};
use paper_mirror::platform_windows::{assoc, shell};

const TITLE: &str = "Paper Mirror Setup";

fn ask(silent: bool, default: bool, text: &str) -> bool {
    if silent {
        return default;
    }
    rfd::MessageDialog::new().set_title(TITLE).set_description(text).set_buttons(rfd::MessageButtons::YesNo).show()
        == rfd::MessageDialogResult::Yes
}

fn tell(silent: bool, error: bool, text: &str) {
    if silent {
        eprintln!("{text}");
        return;
    }
    rfd::MessageDialog::new()
        .set_title(TITLE)
        .set_level(if error { rfd::MessageLevel::Error } else { rfd::MessageLevel::Info })
        .set_description(text)
        .show();
}

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(args: Vec<String>) -> i32 {
    if args.first().map(String::as_str) == Some("--pack") {
        return pack(&args[1..]);
    }
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().find_map(|a| a.strip_prefix(&format!("{f}=")).map(str::to_string));
    let silent = flag("--silent");
    let mut opts = Options::defaults();
    if let Some(base) = value("--reg-base") {
        opts.reg_base = base;
    }
    if let Some(d) = value("--shortcut-dir") {
        opts.shortcut_dir = Some(PathBuf::from(d));
    }
    if flag("--no-shortcut") {
        opts.shortcut_dir = None;
    }
    let exe = std::env::current_exe().unwrap_or_default();

    if flag("--uninstall") {
        opts.dir = value("--dir").map(PathBuf::from).or_else(|| exe.parent().map(PathBuf::from)).unwrap_or(opts.dir);
        if !ask(silent, true, "Remove Paper Mirror from this PC?\n\nYour documents are not affected.") {
            return 2;
        }
        if let Err(e) = installer::uninstall(&opts) {
            tell(silent, true, &e);
            return 1;
        }
        let data = paper_mirror::storage::data_dir();
        if data.is_dir()
            && ask(
                silent,
                false,
                &format!("Also remove your Paper Mirror settings and saved signatures?\n\n{}", data.display()),
            )
        {
            let _ = std::fs::remove_dir_all(&data);
        }
        tell(silent, false, "Paper Mirror has been removed.");
        return 0;
    }

    if let Some(d) = value("--dir") {
        opts.dir = PathBuf::from(d);
    }
    let entries = match installer::read_payload(&exe) {
        Ok(Some(e)) => e,
        Ok(None) => {
            tell(silent, true, "This setup program contains no files. Use the setup file from a Paper Mirror release.");
            return 1;
        }
        Err(e) => {
            tell(silent, true, &e);
            return 1;
        }
    };
    let upgrading = installer::is_installed(&opts.reg_base);
    if !ask(
        silent,
        true,
        &format!(
            "{} Paper Mirror {} for your account?\n\nFolder: {}\n\nNo administrator rights are needed, and Paper Mirror never connects to the internet.",
            if upgrading { "Update" } else { "Install" },
            paper_mirror::VERSION,
            opts.dir.display()
        ),
    ) {
        return 2;
    }
    opts.associate = flag("--associate")
        || (!silent
            && ask(
                false,
                false,
                "Add Paper Mirror to “Open with” for PDF and image files?\n\nWindows Settings then opens, where you can choose Paper Mirror as the default for the file types you want.",
            ));
    let stub = match installer::stub_bytes(&exe) {
        Ok(s) => s,
        Err(e) => {
            tell(silent, true, &e);
            return 1;
        }
    };
    match installer::install(&entries, &stub, &opts) {
        Ok(app) => {
            if opts.associate && !silent {
                shell::open_default_apps(assoc::APP_NAME);
            }
            if ask(silent, false, "Paper Mirror is installed. Open it now?") {
                let _ = std::process::Command::new(app).spawn();
            }
            0
        }
        Err(e) => {
            tell(silent, true, &format!("Paper Mirror couldn’t be installed.\n\n{e}"));
            1
        }
    }
}

fn pack(args: &[String]) -> i32 {
    let Some(out) = args.first() else {
        eprintln!("usage: --pack <out.exe> <name>=<file>...");
        return 1;
    };
    let files: Vec<(String, PathBuf)> =
        args[1..].iter().filter_map(|a| a.split_once('=').map(|(n, p)| (n.to_string(), PathBuf::from(p)))).collect();
    let me = std::env::current_exe().unwrap_or_default();
    match installer::pack(&me, &files, std::path::Path::new(out)) {
        Ok(()) => {
            println!("wrote {out} ({} files)", files.len());
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
