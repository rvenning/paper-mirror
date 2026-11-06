# 0001 — Rust toolchain and C toolchain

Date: 2026-11-06 · Status: accepted

## Context
The machine has no Visual Studio Build Tools or Windows SDK and only 3.9 GB free on C:.
The user requires every build tool to live on D:. Installing MSVC Build Tools would put
several GB on C: and requires accepting Microsoft's licence on the user's behalf.

## Decision
- Rust 1.99.1 via rustup with `CARGO_HOME=D:\rust\cargo`, `RUSTUP_HOME=D:\rust\rustup`;
  PATH is not modified globally — `scripts/env.ps1` / `scripts/env.sh` set it per shell.
- Host/target `x86_64-pc-windows-gnullvm`, paired with llvm-mingw 20261104 (UCRT) in
  `D:\rust\llvm-mingw` (clang, lld, llvm-dlltool). Pinned in `rust-toolchain.toml`.
- `+crt-static` links LLVM libunwind statically; the UCRT is the OS-provided one.

## Alternatives rejected
- `x86_64-pc-windows-msvc`: needs Build Tools + SDK on C:.
- `x86_64-pc-windows-gnu`: bundled dlltool cannot build `raw-dylib` imports without an
  assembler; swapping in llvm-dlltool with GNU ld produced crashing binaries.

## Consequences
The executable depends only on Windows system DLLs. Contributors need the same layout
(`scripts/setup-toolchain.ps1` installs it). PDFium is consumed as a prebuilt DLL, so no
C++ compilation is needed.
