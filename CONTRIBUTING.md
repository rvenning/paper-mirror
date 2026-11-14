# Contributing to Paper Mirror

Thanks for helping. Bug reports, accessibility findings, test files that break Paper
Mirror, and pull requests are all welcome.

## Reporting a bug
Open an [issue](https://github.com/rvenning/paper-mirror/issues/new/choose) and include:
- the Paper Mirror version (Help > About) and your Windows version;
- what you did, what you expected, and what happened;
- if a particular file triggers it, the file, or a way to make one. Don't attach
  documents that contain personal information.

Security problems should not go in a public issue: see [SECURITY.md](SECURITY.md).

## Building
Paper Mirror is Rust with [Slint](https://slint.dev) for the interface and
[PDFium](https://pdfium.googlesource.com/pdfium/) for PDFs. It builds with the
`x86_64-pc-windows-gnullvm` toolchain and llvm-mingw, so no Visual Studio is needed.
See the README's *Build from source* section and `docs/decisions/0001-toolchain.md`.

```powershell
.\scripts\fetch-pdfium.ps1   # pinned PDFium build, checksum verified
cargo run -- some.pdf        # run
cargo test                   # unit, backend and headless UI tests
```

## Before opening a pull request
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must be clean (CI checks
  both).
- `cargo test` must pass. Add a test with the change: a unit test next to the code, or
  one of the end-to-end tests in `tests/`.
- Keep the project's rules (`docs/discovery.md` and the decision records):
  - no network access, telemetry, background services or auto-start;
  - never overwrite a user's file without the existing safeguards;
  - treat every file as untrusted input.
- Any new dependency needs a reason, and a note in the relevant decision record under
  `docs/decisions/`. Dependencies are pinned to exact versions.
- If a change alters behaviour that `docs/feature-checklist.md` or
  `docs/known-limitations.md` describes, update them.
- User-visible changes go under `Unreleased` in [CHANGELOG.md](CHANGELOG.md).

## Code of conduct
Taking part means following the [Code of Conduct](CODE_OF_CONDUCT.md).

## Licence
Contributions are accepted under the project's [MIT licence](LICENSE).
