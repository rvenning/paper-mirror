# Builds a release into dist\:
#   PaperMirror-<version>-setup.exe      per-user installer (no administrator rights)
#   PaperMirror-<version>-portable.zip   unzip anywhere; settings stay next to the program
#   THIRD-PARTY-NOTICES.txt, SHA256SUMS.txt
#
# Usage (PowerShell, from anywhere):
#   .\scripts\package.ps1                                  unsigned build
#   .\scripts\package.ps1 -CertificateThumbprint <sha1>    sign with a code-signing
#                                                          certificate in Cert:\CurrentUser\My
#   .\scripts\package.ps1 -PfxPath cert.pfx                sign with a .pfx file (password from
#                                                          $env:PM_PFX_PASSWORD, or asked for)
#   .\scripts\package.ps1 -SignCommand '<command>'         sign by running a command with each
#                                                          file's quoted path appended (signtool
#                                                          with Azure Trusted Signing, a token...)
# Signing covers paper-mirror.exe, pdfium.dll, the uninstaller and the setup itself;
# see docs/decisions/0010-licence-and-code-signing.md. -TimestampServer '' skips the
# timestamp (the signature then expires with the certificate).
param(
    [string]$CertificateThumbprint,
    [string]$PfxPath,
    [string]$SignCommand,
    [string]$TimestampServer = 'http://timestamp.digicert.com'
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\env.ps1"
$root = Split-Path $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "Paper Mirror $version"

# The signing certificate, found before the (long) build so mistakes show early.
$cert = $null
if ($CertificateThumbprint) {
    $cert = Get-Item "Cert:\CurrentUser\My\$($CertificateThumbprint -replace '\s', '')"
} elseif ($PfxPath) {
    $pw = if ($env:PM_PFX_PASSWORD) { $env:PM_PFX_PASSWORD } else {
        [Runtime.InteropServices.Marshal]::PtrToStringBSTR(
            [Runtime.InteropServices.Marshal]::SecureStringToBSTR((Read-Host -AsSecureString "Password for $PfxPath")))
    }
    $cert = New-Object Security.Cryptography.X509Certificates.X509Certificate2((Resolve-Path $PfxPath).Path, $pw)
}
if ($cert -and -not $cert.HasPrivateKey) { throw "The certificate $($cert.Subject) has no private key." }
$signing = [bool]$cert -or [bool]$SignCommand

function Sign-File([string]$path) {
    if ($SignCommand) {
        cmd /c "$SignCommand `"$path`""
        if ($LASTEXITCODE) { throw "signing $path failed ($LASTEXITCODE)" }
    } else {
        $a = @{ FilePath = $path; Certificate = $cert; HashAlgorithm = 'SHA256' }
        if ($TimestampServer) { $a.TimestampServer = $TimestampServer }
        Set-AuthenticodeSignature @a | Out-Null
    }
    $s = Get-AuthenticodeSignature $path
    if (-not $s.SignerCertificate) { throw "$path was not signed" }
    Write-Host "signed $(Split-Path $path -Leaf): $($s.SignerCertificate.Subject) ($($s.Status))"
}

if (-not (Test-Path third_party\pdfium\bin\pdfium.dll)) { & "$PSScriptRoot\fetch-pdfium.ps1" }

cargo build --release --bins
if ($LASTEXITCODE) { throw 'release build failed' }
cargo run -q --example notices -- dist\THIRD-PARTY-NOTICES.txt
if ($LASTEXITCODE) { throw 'notices failed' }

$rel = Join-Path $root 'target\release'
$dist = Join-Path $root 'dist'

# Programs are copied out of target\ before signing, so cargo's outputs stay untouched.
$bin = Join-Path $root 'target\package'
if (Test-Path $bin) { Remove-Item -Recurse -Force $bin }
New-Item -ItemType Directory $bin | Out-Null
Copy-Item "$rel\paper-mirror.exe", "$rel\pdfium.dll" $bin
$files = @(
    @{ Name = 'paper-mirror.exe'; Path = "$bin\paper-mirror.exe" },
    @{ Name = 'pdfium.dll'; Path = "$bin\pdfium.dll" },
    @{ Name = 'THIRD-PARTY-NOTICES.txt'; Path = "$dist\THIRD-PARTY-NOTICES.txt" },
    @{ Name = 'LICENSE.txt'; Path = "$root\LICENSE" },
    @{ Name = 'README.md'; Path = "$root\README.md" }
)
$setupFiles = $files
if ($signing) {
    Sign-File "$bin\paper-mirror.exe"
    Sign-File "$bin\pdfium.dll"
    # The setup program becomes the uninstaller. The signed setup can't be reused for
    # that (cutting off its payload breaks the signature), so a separately signed copy
    # travels in the payload.
    Copy-Item "$rel\paper-mirror-setup.exe" "$bin\Uninstall Paper Mirror.exe"
    Sign-File "$bin\Uninstall Paper Mirror.exe"
    $setupFiles = $files + @(@{ Name = 'Uninstall Paper Mirror.exe'; Path = "$bin\Uninstall Paper Mirror.exe" })
} else {
    Write-Warning 'Not signed: Windows SmartScreen and Smart App Control will warn about or block these files.'
}

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

# Installer: the setup stub with the files appended (see src/installer/mod.rs), then
# signed as a whole; the signature goes after the payload, where the setup expects it.
$setup = Join-Path $dist "PaperMirror-$version-setup.exe"
$packArgs = @('--pack', "`"$setup`"") + ($setupFiles | ForEach-Object { "`"$($_.Name)=$($_.Path)`"" })
$p = Start-Process -FilePath "$rel\paper-mirror-setup.exe" -ArgumentList $packArgs -Wait -PassThru -NoNewWindow
if ($p.ExitCode -ne 0) { throw "packing the installer failed ($($p.ExitCode))" }
if ($signing) { Sign-File $setup }

# Checksums
$sums = Get-ChildItem $dist -File | Where-Object { $_.Extension -in '.exe', '.zip' } | ForEach-Object {
    "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.Name)"
}
Set-Content -Encoding ascii (Join-Path $dist 'SHA256SUMS.txt') -Value $sums
Get-ChildItem $dist -File | Format-Table Name, @{ n = 'MB'; e = { [math]::Round($_.Length / 1MB, 2) } } -AutoSize
