# Downloads the pinned PDFium build into third_party/pdfium and verifies its checksum.
# Source: https://github.com/bblanchon/pdfium-binaries (MIT scripts, PDFium BSD-3-Clause).
$ErrorActionPreference = 'Stop'
$Tag = 'chromium/8138'
$Asset = 'pdfium-win-x64.tgz'
$Sha256 = '8d48973e3eb472dda0f5f2435e595ac043f9f2219dd71883392f549445afa8a5'

$Root = Split-Path -Parent $PSScriptRoot
$Dest = Join-Path $Root 'third_party\pdfium'
$Archive = Join-Path $Dest $Asset
New-Item -ItemType Directory -Force $Dest | Out-Null

if (Test-Path (Join-Path $Dest 'bin\pdfium.dll')) {
    Write-Host "PDFium already present in $Dest"
    exit 0
}

$Url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$([uri]::EscapeDataString($Tag))/$Asset"
Write-Host "Downloading $Url"
Invoke-WebRequest -Uri $Url -OutFile $Archive -UseBasicParsing
$Actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLower()
if ($Actual -ne $Sha256) {
    Remove-Item $Archive
    throw "Checksum mismatch for $Asset (expected $Sha256, got $Actual)"
}
tar -xzf $Archive -C $Dest
Remove-Item $Archive
Write-Host "PDFium $Tag extracted to $Dest"
