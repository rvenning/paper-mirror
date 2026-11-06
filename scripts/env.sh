# Source this from Git Bash: `. scripts/env.sh`. Keeps every toolchain on D:.
export CARGO_HOME='D:\rust\cargo'
export RUSTUP_HOME='D:\rust\rustup'
# llvm-mingw provides the linker (clang/lld) and llvm-dlltool for the gnullvm target.
export PATH="/d/rust/cargo/bin:/d/rust/llvm-mingw/bin:$PATH"
