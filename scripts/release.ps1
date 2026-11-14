# Publishes a GitHub release built on this PC (see RELEASING.md):
#   1. checks the working tree is clean, on main and pushed, and that CHANGELOG.md has
#      a section for the version in Cargo.toml;
#   2. builds the package with scripts/package.ps1 (signing options are passed through);
#   3. tags v<version>, pushes the tag, and creates the GitHub release with the setup,
#      the portable zip and SHA256SUMS.txt, using the changelog section as notes.
# Usage: .\scripts\release.ps1 [-CertificateThumbprint ... | -PfxPath ... | -SignCommand ...] [-Draft]
param(
    [string]$CertificateThumbprint,
    [string]$PfxPath,
    [string]$SignCommand,
    [switch]$Draft
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$tag = "v$version"

if (git status --porcelain) { throw 'The working tree has uncommitted changes.' }
if ((git rev-parse --abbrev-ref HEAD) -ne 'main') { throw 'Releases are made from main.' }
git fetch -q origin
if ((git rev-parse HEAD) -ne (git rev-parse origin/main)) { throw 'main is not pushed (or is behind origin/main).' }
if (git tag --list $tag) { throw "Tag $tag already exists." }

# Release notes: the CHANGELOG.md section for this version.
$lines = Get-Content CHANGELOG.md
$start = ($lines | Select-String -Pattern "^## \[$([regex]::Escape($version))\]").LineNumber
if (-not $start) { throw "CHANGELOG.md has no section for $version." }
$notes = @()
foreach ($l in $lines[$start..($lines.Count - 1)]) {
    if ($l -match '^## \[') { break }
    if ($l -match '^\[.+\]: ') { break }
    $notes += $l
}
$notesFile = Join-Path $env:TEMP "paper-mirror-$tag-notes.md"
Set-Content -Encoding utf8 $notesFile (($notes -join "`n").Trim())

$pkg = @{}
if ($CertificateThumbprint) { $pkg.CertificateThumbprint = $CertificateThumbprint }
if ($PfxPath) { $pkg.PfxPath = $PfxPath }
if ($SignCommand) { $pkg.SignCommand = $SignCommand }
& "$PSScriptRoot\package.ps1" @pkg

$assets = @(
    "dist\PaperMirror-$version-setup.exe",
    "dist\PaperMirror-$version-portable.zip",
    'dist\SHA256SUMS.txt'
)
foreach ($a in $assets) { if (-not (Test-Path $a)) { throw "Missing $a" } }

git tag -a $tag -m "Paper Mirror $version"
git push origin $tag
$ghArgs = @('release', 'create', $tag) + $assets + @('--title', "Paper Mirror $version", '--notes-file', $notesFile, '--verify-tag')
if ($Draft) { $ghArgs += '--draft' }
gh @ghArgs
if ($LASTEXITCODE) { throw 'gh release create failed' }
Write-Host "Released $tag"
