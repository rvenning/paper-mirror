# 0008 — Printing, Share, file types, installer and notices

Date: 2026-11-07 · Status: accepted

## Context
M8 needs printing, the Windows Share sheet, opt-in file associations, a per-user
installer and a portable build, and third-party notices. The constraints are: no
administrator rights, no services or auto-start, no network access, nothing taken
over without the user's say, and no extra build tools beyond the D: toolchain.

## Decision
1. **Printing.** The Windows Print dialog (`PrintDlgEx`) picks the printer, copies,
   collation and range (all, current page, selection, page numbers); the driver handles
   copies. On the engine thread, markup is flattened into a copy of the document. PDFium's
   `FPDF_RenderPage` then draws each page straight into the printer device context, so
   text and vectors stay sharp. The function is looked up in the loaded `pdfium.dll`.
   Pages shrink to fit the printable area (images also enlarge), are centred, and turn
   to match the paper's orientation. Tests print to "Microsoft Print to PDF" with an
   output file and read the result back.
2. **Share.** `IDataTransferManagerInterop` opens the Share sheet for the window, and
   the file goes over as a read-only storage item. A document with unsaved changes is
   shared as a copy of what is on screen, written to `%TEMP%\Paper Mirror Share`.
   Nothing is sent unless the user picks a target.
3. **File types.** Help > Use Paper Mirror for PDFs and Images… writes per-user keys:
   ProgIDs, `OpenWithProgids`, `Applications\paper-mirror.exe` and `RegisteredApplications`
   with Capabilities. It then opens Settings > Default apps on Paper Mirror's page.
   Windows lets only the user set defaults, so nothing is taken over. Unregistering
   removes only Paper Mirror's own entries.
4. **Installer.** `paper-mirror-setup.exe` is a 0.5 MB Rust program. The release files
   are appended to it, deflate-compressed with a SHA-256 checksum, and entry names are
   restricted to plain file names. It installs to `%LOCALAPPDATA%\Programs\Paper Mirror`
   and adds a Start-menu shortcut and an Apps & features entry (HKCU). File types are
   optional and asked for. Updating installs over the top and removes files the new
   version no longer ships. The uninstaller removes everything it installed, keeps user
   data unless asked, and deletes itself through a short-lived `cmd` after it exits.
   Both programs carry a manifest with `asInvoker`, which stops Windows' installer
   detection from asking for elevation, plus PerMonitorV2 DPI awareness, Common Controls
   6 and UTF-8.
5. **Portable.** The same files plus `portable.txt`, which keeps settings in a `data`
   folder beside the program (`storage::data_dir`).
6. **Notices.** `cargo run --example notices` lists every crate linked into the
   executables. It takes the set from `cargo tree -e normal,no-proc-macro`, because
   `cargo metadata` alone also counts dev-dependency features and build-time
   proc-macros. Identical licence texts print once. PDFium's licence and its bundled
   libraries' licences follow, and standard texts cover crates that ship no licence file.
7. **Resources.** An original icon (`scripts/make_icon.py`, drawn with Pillow) and
   version information are compiled by llvm-mingw's `windres` from `build.rs`, per
   executable.

## Dependencies
`windows` 0.62.2 (MIT OR Apache-2.0) for the COM and WinRT interfaces `windows-sys`
lacks (PrintDlgEx, DataTransferManager, IShellLinkW), plus `windows-collections` 0.3.2,
`raw-window-handle` 0.6.2 and `flate2` 1.1.10. All four were already in the dependency
tree through Slint and `png`, so no new code is downloaded.

## Consequences
The executables are unsigned, so SmartScreen warns on first run until a code-signing
certificate is used. The Print dialog and Share sheet are system UI that automated tests can't drive, so
they need checking by hand. Automated tests cover print output, page ranges, share-copy preparation,
registration and install/update/uninstall, all under scratch registry keys and
temporary folders.
