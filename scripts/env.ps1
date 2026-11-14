# Dot-source from PowerShell: `. .\scripts\env.ps1`. On the development PC every
# toolchain lives on D: (ADR 0001); elsewhere (CI) the tools already on PATH are used.
if (Test-Path 'D:\rust\cargo\bin') {
    $env:CARGO_HOME = 'D:\rust\cargo'
    $env:RUSTUP_HOME = 'D:\rust\rustup'
    # llvm-mingw provides the linker (clang/lld) and llvm-dlltool for the gnullvm target.
    $env:PATH = "D:\rust\cargo\bin;D:\rust\llvm-mingw\bin;$env:PATH"
}
