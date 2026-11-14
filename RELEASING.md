# Releasing Paper Mirror

Releases are built on the maintainer's PC with the same toolchain that runs the tests.
That is also where a code-signing certificate lives, if there is one. GitHub Actions
builds and tests every push, but publishes nothing.

## Checklist
1. Make sure `main` is green on [GitHub Actions](https://github.com/rvenning/paper-mirror/actions)
   and that `cargo test` passes locally.
2. Set the version in `Cargo.toml` and run `cargo build` so `Cargo.lock` follows.
3. In `CHANGELOG.md`:
   - move the `Unreleased` entries under a new `## [x.y.z] - YYYY-MM-DD` heading;
   - update the comparison links at the bottom.
4. Commit ("Release x.y.z") and push to `main`.
5. From PowerShell:

   ```powershell
   . .\scripts\env.ps1
   .\scripts\release.ps1                          # unsigned
   .\scripts\release.ps1 -CertificateThumbprint <sha1>   # signed (ADR 0010)
   ```

   The script:
   - checks the tree is clean and pushed, and the changelog has the version;
   - builds `dist\` with `scripts\package.ps1`;
   - tags `vx.y.z` and pushes the tag;
   - creates the GitHub release with the setup, the portable zip and `SHA256SUMS.txt`,
     using the changelog section as the notes.

   Add `-Draft` to review the release before it goes public.
6. Check the release page. Download the setup, compare its SHA-256 with
   `SHA256SUMS.txt`, install it on a clean account, and open a PDF.

## Signing
Without a certificate the release is unsigned. SmartScreen then warns on first run, and
Smart App Control blocks it. `docs/decisions/0010-licence-and-code-signing.md` describes
the options and how `package.ps1` signs every program and DLL.
