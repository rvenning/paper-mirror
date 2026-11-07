use std::path::PathBuf;

fn main() {
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .expect("Slint UI failed to compile");
    compile_resources();

    // Copy the pinned PDFium DLL next to the built executables so `cargo run` and the
    // test binaries load it by absolute path from their own directory.
    let dll = PathBuf::from("third_party/pdfium/bin/pdfium.dll");
    println!("cargo:rerun-if-changed={}", dll.display());
    if !dll.is_file() {
        println!("cargo:warning=third_party/pdfium/bin/pdfium.dll is missing; run scripts/fetch-pdfium.ps1");
        return;
    }
    // OUT_DIR is target/<profile>/build/<pkg>/out; the profile directory is 3 levels up.
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    if let Some(profile_dir) = out.ancestors().nth(3) {
        for dir in [profile_dir.to_path_buf(), profile_dir.join("deps"), profile_dir.join("examples")] {
            let _ = std::fs::create_dir_all(&dir);
            let dest = dir.join("pdfium.dll");
            let stale = match (std::fs::metadata(&dll), std::fs::metadata(&dest)) {
                (Ok(a), Ok(b)) => a.len() != b.len() || a.modified().ok() > b.modified().ok(),
                _ => true,
            };
            if stale {
                let _ = std::fs::copy(&dll, &dest);
            }
        }
    }
}

/// Icon, version information and manifest for each executable, compiled with the
/// llvm-mingw resource compiler (on PATH after scripts/env.sh). Missing tools only warn.
fn compile_resources() {
    let assets = std::fs::canonicalize("assets").expect("assets folder");
    for f in ["paper-mirror.ico", "app.manifest"] {
        println!("cargo:rerun-if-changed=assets/{f}");
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split(['.', '-']).map(|p| p.parse::<u16>().unwrap_or(0));
    let (a, b, c) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
    // Forward slashes: windres treats backslashes in quoted strings as escapes.
    let path = |f: &str| assets.join(f).display().to_string().replace('\\', "/").trim_start_matches("//?/").to_string();
    for (bin, description) in [("paper-mirror", "Paper Mirror"), ("paper-mirror-setup", "Paper Mirror Setup")] {
        let rc = format!(
            r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Paper Mirror"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{bin}"
      VALUE "OriginalFilename", "{bin}.exe"
      VALUE "ProductName", "Paper Mirror"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
            icon = path("paper-mirror.ico"),
            manifest = path("app.manifest"),
        );
        let rc_path = out.join(format!("{bin}.rc"));
        let obj = out.join(format!("{bin}-res.o"));
        std::fs::write(&rc_path, rc).unwrap();
        let ok = std::process::Command::new(&windres)
            .args(["-i", rc_path.to_str().unwrap(), "-o", obj.to_str().unwrap(), "-O", "coff"])
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            println!("cargo:rustc-link-arg-bin={bin}={}", obj.display());
        } else {
            println!("cargo:warning={windres} failed or is missing: {bin}.exe gets no icon or manifest");
        }
    }
}
