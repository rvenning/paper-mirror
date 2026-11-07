# Measures the release build: start-up, time to first rendered page and memory for a
# set of documents, each run several times; prints the median of every measure point.
# Usage:  .\scripts\measure.ps1 [-Runs 5] [-Extra 'C:\some\big.pdf', ...]
# Build first: cargo build --release
param([int]$Runs = 5, [string[]]$Extra = @())
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$exe = Join-Path $root 'target\release\paper-mirror.exe'
if (-not (Test-Path $exe)) { throw 'Build the release first: cargo build --release' }
$tmp = Join-Path $env:TEMP "pm-measure-$PID"
New-Item -ItemType Directory $tmp | Out-Null
# Settings and recents go to a scratch folder, not the user's profile.
$env:PAPER_MIRROR_DATA_DIR = Join-Path $tmp 'data'

$scenarios = @(
    @{ Name = 'Start, no document'; Args = @('--measure', '--exit-after-idle') },
    @{ Name = 'PDF, 3 pages (text-3p.pdf)'; Args = @("$root\tests\fixtures\text-3p.pdf", '--measure', '--exit-after-render') },
    @{ Name = 'PDF, 120 pages (pages-120.pdf)'; Args = @("$root\tests\fixtures\pages-120.pdf", '--measure', '--exit-after-render') }
)
foreach ($f in $Extra) {
    $scenarios += @{ Name = "$(Split-Path $f -Leaf) ($([math]::Round((Get-Item $f).Length / 1MB, 1)) MB)"; Args = @($f, '--measure', '--exit-after-render') }
}

function Median([double[]]$v) {
    $s = $v | Sort-Object
    if ($s.Count % 2) { $s[[int][math]::Floor($s.Count / 2)] } else { ($s[$s.Count / 2 - 1] + $s[$s.Count / 2]) / 2 }
}

$rows = @()
foreach ($sc in $scenarios) {
    $points = @{}
    for ($i = 0; $i -lt $Runs; $i++) {
        $err = Join-Path $tmp 'err.txt'
        $argList = $sc.Args | ForEach-Object { if ($_ -match ' ') { "`"$_`"" } else { $_ } }
        $p = Start-Process $exe -ArgumentList $argList -Wait -PassThru -NoNewWindow -RedirectStandardError $err
        foreach ($line in Get-Content $err) {
            if ($line -match '^\[measure\] (?<label>[^:]+): t=(?<t>[\d.]+) ms working_set=(?<ws>[\d.]+) MiB private=(?<pv>[\d.]+) MiB peak_ws=(?<pk>[\d.]+) MiB') {
                $k = $Matches.label
                if (-not $points.ContainsKey($k)) { $points[$k] = @{ t = @(); ws = @(); pk = @() } }
                $points[$k].t += [double]$Matches.t
                $points[$k].ws += [double]$Matches.ws
                $points[$k].pk += [double]$Matches.pk
            }
        }
    }
    foreach ($k in $points.Keys) {
        $rows += [pscustomobject]@{
            Scenario = $sc.Name
            Point = $k
            'Time ms' = [math]::Round((Median $points[$k].t), 0)
            'Working set MiB' = [math]::Round((Median $points[$k].ws), 1)
            'Peak MiB' = [math]::Round((Median $points[$k].pk), 1)
        }
    }
}
Remove-Item -Recurse -Force $tmp
$rows | Sort-Object Scenario, 'Time ms' | Format-Table -AutoSize
