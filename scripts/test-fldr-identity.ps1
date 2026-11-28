[CmdletBinding()]
param([string]$Resource)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$checks = 0
function Assert-Identity($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:checks++
}
$manifest = [IO.File]::ReadAllText((Join-Path $root 'Cargo.toml'))
Assert-Identity ($manifest -match 'default-run\s*=\s*"FLDR"') 'Incorrect default executable.'
Assert-Identity ($manifest -match '(?s)\[\[bin\]\]\s*name\s*=\s*"FLDR"') 'Incorrect binary target.'
foreach ($locale in Get-ChildItem -LiteralPath (Join-Path $root 'crates\atlas-ui\locales') -Filter '*.yml') {
    if ($locale.Name -match 'conflicted') { continue }
    $text = [IO.File]::ReadAllText($locale.FullName)
    Assert-Identity ($text -match '(?m)^"?app_name"?:\s*"?FLDR"?\s*$') "Wrong app name: $($locale.Name)"
    Assert-Identity ($text -cnotmatch 'FileOP') "Old public name: $($locale.Name)"
}
if (-not $Resource) {
    $Resource = Get-ChildItem 'C:\Build\FileOP\debug\build\atlas-*\out\fileop.res' |
        Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1 -ExpandProperty FullName
}
Assert-Identity ([bool]$Resource) 'Compile the FLDR target first to generate its Windows resources.'
$res = [IO.File]::ReadAllBytes($Resource)
$icons = @{}
$group = $null
$version = $null
for ($position = 0; $position -lt $res.Length;) {
    $size = [BitConverter]::ToUInt32($res, $position)
    $header = [BitConverter]::ToUInt32($res, $position + 4)
    if ($header -lt 8 -or $position + $header + $size -gt $res.Length) { throw 'Invalid RES record.' }
    $type = [BitConverter]::ToUInt16($res, $position + 10)
    $id = [BitConverter]::ToUInt16($res, $position + 14)
    $data = New-Object byte[] $size
    [Array]::Copy($res, $position + $header, $data, 0, $size)
    if ($type -eq 3) { $icons[$id] = $data }
    if ($type -eq 14 -and $id -eq 1) { $group = $data }
    if ($type -eq 16) { $version = [Text.Encoding]::Unicode.GetString($data) }
    $position = ($position + $header + $size + 3) -band -4
}
Assert-Identity ($null -ne $group) 'Missing application icon group ID 1.'
$ico = [IO.File]::ReadAllBytes((Join-Path $root 'FLDR-icon.ico'))
$count = [BitConverter]::ToUInt16($ico, 4)
Assert-Identity ($count -eq [BitConverter]::ToUInt16($group, 4)) 'ICO frame count changed.'
for ($i = 0; $i -lt $count; $i++) {
    $source = 6 + 16 * $i
    $embedded = 6 + 14 * $i
    Assert-Identity ([Convert]::ToBase64String($ico, $source, 12) -eq [Convert]::ToBase64String($group, $embedded, 12)) "ICO frame $i dimensions/format changed."
    $length = [BitConverter]::ToUInt32($ico, $source + 8)
    $offset = [BitConverter]::ToUInt32($ico, $source + 12)
    $id = [BitConverter]::ToUInt16($group, $embedded + 12)
    Assert-Identity ([Convert]::ToBase64String($ico, $offset, $length) -eq [Convert]::ToBase64String($icons[$id])) "ICO frame $i bytes/transparency changed."
}
Assert-Identity ($version -match 'FLDR.exe' -and $version -match 'digART') 'Wrong executable version metadata.'
Write-Output "$checks identity/resource checks passed; $count ICO frames preserved byte for byte. Resource: $Resource"
