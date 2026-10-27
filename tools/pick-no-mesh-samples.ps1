<#
  ModelCraft - pick representative "no_mesh" files from a swcraft-scan CSV and COPY them
  (the originals are only read, never modified or moved) into a folder + a zip to send back.

  Usage (PowerShell 5.1 or 7):
    powershell -ExecutionPolicy Bypass -File pick-no-mesh-samples.ps1 `
        -Csv "$env:USERPROFILE\ModelCraft\scan\scan.csv" `
        -Dest "$env:USERPROFILE\ModelCraft\samples4" -Count 15 -MaxMiB 100
  Add -DryRun to only print the selection.
#>
param(
    [string]$Csv = "$env:USERPROFILE\ModelCraft\scan\scan.csv",
    [string]$Dest = "$env:USERPROFILE\ModelCraft\samples4",
    [int]$Count = 15,
    [int]$MaxMiB = 100,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Csv)) { throw "CSV not found: $Csv" }
$rows = Import-Csv -LiteralPath $Csv -Delimiter ';' -Encoding UTF8 |
    Where-Object { $_.status -eq 'no_mesh' -and [int64]$_.size_bytes -le ([int64]$MaxMiB * 1MB) -and (Test-Path -LiteralPath $_.path) }
Write-Host ("{0} no_mesh files <= {1} MiB still present" -f @($rows).Count, $MaxMiB)

function Get-Category($r) {
    $m = "$($r.message)"
    if ($m -match '__ZLB') { return 'bad_zlb' }
    if ($m -match 'DisplayLists pr') { return 'dl_undecoded' }
    if ($r.kind -eq 'assembly' -and [int]$r.missing -gt 0) { return 'asm_missing' }
    if ($m -match 'Pas de flux DisplayLists') { if ($r.kind -eq 'assembly') { return 'nodl_assembly' } else { return 'nodl_part' } }
    return 'other'
}
# quota per category (sums to 15 by default, scaled to -Count); unused slots go to the others
$quota = [ordered]@{ dl_undecoded = 5; bad_zlb = 3; nodl_part = 3; nodl_assembly = 2; asm_missing = 1; other = 1 }
$groups = @{}
foreach ($r in $rows) { $c = Get-Category $r; if (-not $groups[$c]) { $groups[$c] = New-Object System.Collections.ArrayList }; [void]$groups[$c].Add($r) }
foreach ($k in $quota.Keys) { Write-Host ("  {0,-14} {1}" -f $k, @($groups[$k]).Count) }

# spread the picks over sizes and folders: sort by size, take evenly spaced items, skip a folder already used
function Select-Spread($list, $n, $usedDirs) {
    $sorted = @($list | Sort-Object { [int64]$_.size_bytes })
    $out = @()
    if ($sorted.Count -eq 0 -or $n -le 0) { return $out }
    $step = [math]::Max(1, [math]::Floor($sorted.Count / $n))
    for ($pass = 0; $pass -lt 2 -and $out.Count -lt $n; $pass++) {
        for ($i = [math]::Floor($step / 2); $i -lt $sorted.Count -and $out.Count -lt $n; $i += $step) {
            $r = $sorted[$i]; $d = Split-Path -Parent $r.path
            if ($out -contains $r) { continue }
            if ($pass -eq 0 -and $usedDirs.Contains($d)) { continue }
            $out += $r; [void]$usedDirs.Add($d)
        }
        $step = 1
    }
    return $out
}
$scale = $Count / 15.0
$used = New-Object 'System.Collections.Generic.HashSet[string]'
$picked = @(); $left = 0
foreach ($k in $quota.Keys) {
    $want = [math]::Round($quota[$k] * $scale) + $left
    $sel = @(Select-Spread $groups[$k] $want $used)
    $left = $want - $sel.Count
    foreach ($s in $sel) { $picked += [pscustomobject]@{ category = $k; row = $s } }
}
if ($left -gt 0) {
    $rest = @($rows | Where-Object { $p = $_.path; -not ($picked | Where-Object { $_.row.path -eq $p }) })
    foreach ($s in @(Select-Spread $rest $left $used)) { $picked += [pscustomobject]@{ category = (Get-Category $s); row = $s } }
}

$i = 0
$manifest = foreach ($p in $picked) {
    $i++
    $src = $p.row.path
    $name = '{0:D2}_{1}_{2}' -f $i, $p.category, (Split-Path -Leaf $src)
    [pscustomobject]@{ file = $name; category = $p.category; kind = $p.row.kind; size_mib = [math]::Round([int64]$p.row.size_bytes / 1MB, 2); source = $src; message = $p.row.message }
}
$manifest | Format-Table file, category, kind, size_mib -AutoSize
if ($DryRun) { return }

New-Item -ItemType Directory -Force -Path $Dest | Out-Null
foreach ($m in $manifest) {
    # read-only on the source: a plain copy, then the copy (not the original) is made writable
    Copy-Item -LiteralPath $m.source -Destination (Join-Path $Dest $m.file)
    (Get-Item -LiteralPath (Join-Path $Dest $m.file)).IsReadOnly = $false
}
$manifest | Export-Csv -LiteralPath (Join-Path $Dest 'manifest.csv') -Delimiter ';' -NoTypeInformation -Encoding UTF8
$zip = "$Dest.zip"
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
Compress-Archive -Path (Join-Path $Dest '*') -DestinationPath $zip
$total = (Get-ChildItem -LiteralPath $Dest | Measure-Object Length -Sum).Sum / 1MB
Write-Host ("Copied {0} files ({1:N1} MiB) to {2}" -f @($manifest).Count, $total, $Dest)
Write-Host "Archive: $zip"
