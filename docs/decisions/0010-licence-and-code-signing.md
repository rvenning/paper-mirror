# 0010 — Licence and code signing

Date: 2026-11-07 · Status: accepted

## Context
Paper Mirror's own code had no licence, and the release files were unsigned. Unsigned
programs downloaded from the internet make SmartScreen warn on first run. Smart App
Control (Windows 11) blocks them, including unsigned DLLs. The brief names neither a
licence nor a certificate.

## Decision
1. **Licence: MIT.** It is the simplest permissive licence and fits every dependency.
   - All the linked crates are permissive (MIT, Apache-2.0, BSD, Zlib, BSL-1.0 and
     similar; `THIRD-PARTY-NOTICES.txt` lists them), and PDFium is BSD-3-Clause.
   - Slint is used under its Royalty-free licence 2.0, not GPL-3.0. That licence allows
     desktop applications under any licence, provided they show the attribution:
     `AboutSlint` in Help > About Paper Mirror.
   - The licence is in `LICENSE` and `Cargo.toml`, the About window, the version
     resource (`LegalCopyright`), the top of the generated notices, and the release as
     `LICENSE.txt`.

   Until a build is shared, the choice is a one-line change if the owner prefers
   another licence. For example, `MIT OR Apache-2.0` adds an explicit patent grant.
2. **Signing is built into packaging; the certificate is the owner's.**
   `scripts/package.ps1` signs when it is given a certificate:
   - `-CertificateThumbprint` uses a code-signing certificate in
     `Cert:\CurrentUser\My`, which is where a USB token's certificate also appears;
   - `-PfxPath` uses a `.pfx` file, with its password from `PM_PFX_PASSWORD` or asked
     for;
   - `-SignCommand` runs any signing command with each file's path appended. Azure
     Trusted Signing (`signtool … /dlib Azure.CodeSigning.Dlib.dll`) or a vendor's
     tool goes here.

   Certificate signing uses PowerShell's built-in `Set-AuthenticodeSignature` (SHA-256),
   so no Windows SDK is needed. Signatures are timestamped (`-TimestampServer`, DigiCert
   by default), so they stay valid after the certificate expires. Packaging contacts the
   timestamp server only when signing; the program itself still makes no network
   requests (ADR 0005).
3. **What is signed:** `paper-mirror.exe`, `pdfium.dll` (Smart App Control checks DLLs
   too), the uninstaller, and the setup program.
4. **Installer layout.** Signing appends a certificate table after the setup's payload
   and records it in the PE security directory. The setup program finds its payload
   footer before that table, allowing for the up-to-7 bytes of padding signing adds.
   The uninstaller can't be the setup program with its payload cut off, because that
   would break the signature. So a signed release carries a separately signed
   `Uninstall Paper Mirror.exe` in the payload, and setup installs that copy instead.

## Verification
- `tests/installer.rs` reproduces what signing does to the file layout:
  - the payload of a "signed" setup still reads back;
  - the stub comes back unsigned;
  - a payload-supplied uninstaller is installed in place of the stub.
- A release was packaged with a throwaway self-signed certificate made in memory (no
  certificate store touched).
  - All four files were signed and timestamped by DigiCert. Windows reported only the
    expected untrusted-root status.
  - Flipping one payload byte turned the setup's status into `HashMismatch`.
  - The signed setup installed silently into a scratch folder, and the installed
    program, DLL and uninstaller each carried the signature.
  - The installed uninstaller then removed everything.

## Consequences
Until the owner signs with a trusted certificate, releases stay unsigned, and the
package script says so. Possible certificates:
- an OV code-signing certificate, which since 2023 lives on a hardware token or cloud
  HSM, so use `-CertificateThumbprint` or `-SignCommand`;
- Azure Trusted Signing, through `-SignCommand`.
Either way, SmartScreen reputation builds up over the first downloads.
