# Builds a release into dist\:
#   PaperMirror-<version>-setup.exe      per-user installer (no administrator rights)
#   PaperMirror-<version>-portable.zip   unzip anywhere; settings stay next to the program
#   THIRD-PARTY-NOTICES.txt, SHA256SUMS.txt
# Usage (PowerShell, from anywhere):  .\scripts\package.ps1
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\env.ps1"
$root = Split-Path $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "Paper Mirror $version"

if (-not (Test-Path third_party\pdfium\bin\pdfium.dll)) { & "$PSScriptRoot\fetch-pdfium.ps1" }

cargo build --release --bins
if ($LASTEXITCODE) { throw 'release build failed' }
cargo run -q --example notices -- dist\THIRD-PARTY-NOTICES.txt
if ($LASTEXITCODE) { throw 'notices failed' }

$rel = Join-Path $root 'target\release'
$dist = Join-Path $root 'dist'
$files = @(
    @{ Name = 'paper-mirror.exe'; Path = "$rel\paper-mirror.exe" },
    @{ Name = 'pdfium.dll'; Path = "$rel\pdfium.dll" },
    @{ Name = 'THIRD-PARTY-NOTICES.txt'; Path = "$dist\THIRD-PARTY-NOTICES.txt" },
    @{ Name = 'README.md'; Path = "$root\README.md" }
)

# Portable zip
$stage = Join-Path $dist "PaperMirror-$version-portable"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory $stage | Out-Null
foreach ($f in $files) { Copy-Item $f.Path (Join-Path $stage $f.Name) }
Set-Content -Encoding utf8 (Join-Path $stage 'portable.txt') -Value @(
    'Portable mode: Paper Mirror keeps its settings and saved signatures in the "data"',
    'folder next to the program instead of in your user profile. Delete this file to use',
    'the profile instead.')
$zip = Join-Path $dist "PaperMirror-$version-portable.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Remove-Item -Recurse -Force $stage

# Installer: the setup stub with the files appended (see src/installer/mod.rs)
$setup = Join-Path $dist "PaperMirror-$version-setup.exe"
$packArgs = @('--pack', "`"$setup`"") + ($files | ForEach-Object { "`"$($_.Name)=$($_.Path)`"" })
$p = Start-Process -FilePath "$rel\paper-mirror-setup.exe" -ArgumentList $packArgs -Wait -PassThru -NoNewWindow
if ($p.ExitCode -ne 0) { throw "packing the installer failed ($($p.ExitCode))" }

# Checksums
$sums = Get-ChildItem $dist -File | Where-Object { $_.Extension -in '.exe', '.zip' } | ForEach-Object {
    "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.Name)"
}
Set-Content -Encoding ascii (Join-Path $dist 'SHA256SUMS.txt') -Value $sums
Get-ChildItem $dist -File | Format-Table Name, @{ n = 'MB'; e = { [math]::Round($_.Length / 1MB, 2) } } -AutoSize
