use std::path::PathBuf;

fn main() {
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .expect("Slint UI failed to compile");

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
