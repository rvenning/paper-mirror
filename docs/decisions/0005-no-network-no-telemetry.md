# 0005 — Logging, telemetry, updates

Date: 2026-11-06 · Status: accepted

- No telemetry, analytics, crash upload, background services, scheduled tasks or
  auto-start entries.
- No automatic updates (no network access at all). New versions are installed manually.
- Diagnostics go to stderr only when `--measure` is passed; document text, passwords and
  file contents are never logged.
- Share uses the Windows Share sheet, which the user invokes explicitly.
