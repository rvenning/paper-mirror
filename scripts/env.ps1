# Dot-source from PowerShell: `. .\scripts\env.ps1`. Keeps every toolchain on D:.
$env:CARGO_HOME = 'D:\rust\cargo'
$env:RUSTUP_HOME = 'D:\rust\rustup'
# llvm-mingw provides the linker (clang/lld) and llvm-dlltool for the gnullvm target.
$env:PATH = "D:\rust\cargo\bin;D:\rust\llvm-mingw\bin;$env:PATH"
