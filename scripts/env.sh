# Source this from Git Bash: `. scripts/env.sh`. On the development PC every toolchain
# lives on D: (ADR 0001); elsewhere (CI) the tools already on PATH are used.
if [ -d /d/rust/cargo/bin ]; then
    export CARGO_HOME='D:\rust\cargo'
    export RUSTUP_HOME='D:\rust\rustup'
    # llvm-mingw provides the linker (clang/lld) and llvm-dlltool for the gnullvm target.
    export PATH="/d/rust/cargo/bin:/d/rust/llvm-mingw/bin:$PATH"
fi
