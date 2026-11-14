# Privacy and security

## What Paper Mirror does with your data
- **No network access.** Paper Mirror makes no network requests: no telemetry, update
  checks, crash reports or online maps (location is shown as coordinates). It contains
  no networking code (ADR 0005); `THIRD-PARTY-NOTICES.txt` lists every linked component.
- **No background activity.** It runs no services, scheduled tasks or start-up entries.
  The installer writes only to your user profile and your own registry hive.
- **Your files stay yours.**
  - Before Paper Mirror first saves over a file, it keeps the original
    (File > Revert To > Original).
  - Redaction removes the content from the saved file instead of covering it.
  - Saved signatures are encrypted for your Windows account with DPAPI.
- **Share** hands the file to the Windows Share sheet. Nothing leaves the PC unless you
  pick a target.

## Untrusted files
Every PDF, image and model is treated as hostile input:
- PDFs are parsed by PDFium (pinned build, checksum verified) on a separate engine
  thread.
- Images are decoded with limits of 40,000 pixels per side and 250 megapixels per
  frame.
- 3D models are read by size-checked parsers.
- Links open only for safe schemes (http, https, mailto), after you click.
- The installer verifies its own payload with SHA-256 and accepts only plain file
  names.

## Reporting a vulnerability
Please don't open a public issue for security problems. Use GitHub's
[private vulnerability reporting](https://github.com/rvenning/paper-mirror/security/advisories/new)
instead, with:
- the Paper Mirror version and Windows version;
- what an attacker could achieve, and a proof-of-concept file if you have one.

Reports are acknowledged as soon as possible. Fixes are released before details are
published.

## Supported versions
Only the latest release gets security fixes.

## Verifying downloads
Every release lists SHA-256 checksums in `SHA256SUMS.txt`. In PowerShell:

```powershell
Get-FileHash .\PaperMirror-0.1.0-setup.exe -Algorithm SHA256
```

Releases aren't code-signed yet (ADR 0010), so Windows may warn the first time the
setup runs.
