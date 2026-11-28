[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Executable,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,79}$')]
    [string]$ReleaseLabel,
    # Supply only after you have personally confirmed runtime testing.
    [switch]$RuntimeTested
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not $RuntimeTested) {
    throw 'Runtime testing must be explicitly confirmed with -RuntimeTested before copying.'
}
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "Source executable does not exist: $Executable"
}
$source = Get-Item -LiteralPath $Executable
if ($source.Extension -ine '.exe' -or $source.Length -eq 0) {
    throw 'Source must be a nonempty .exe file.'
}
$repository = Split-Path -Parent $PSScriptRoot
$releases = Join-Path $repository 'Releases'
$destination = Join-Path $releases "FLDR-$ReleaseLabel.exe"
$sourceHash = (Get-FileHash -LiteralPath $source.FullName -Algorithm SHA256).Hash
$sourceSize = $source.Length

if (Test-Path -LiteralPath $destination) {
    $existing = Get-Item -LiteralPath $destination
    if ($existing.PSIsContainer -or $existing.Length -ne $sourceSize -or
        (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ne $sourceHash) {
        throw "A different release already exists; choose another label: $destination"
    }
} else {
    [void][System.IO.Directory]::CreateDirectory($releases)
    # No overwrite, including if another publisher creates the path concurrently.
    [System.IO.File]::Copy($source.FullName, $destination, $false)
}

$copied = Get-Item -LiteralPath $destination
$copiedHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
$currentSource = Get-Item -LiteralPath $source.FullName
if ($copied.Length -ne $sourceSize -or $copiedHash -ne $sourceHash -or
    $currentSource.Length -ne $sourceSize -or
    (Get-FileHash -LiteralPath $source.FullName -Algorithm SHA256).Hash -ne $sourceHash) {
    throw "Verification failed. File retained for inspection; do not use as a release: $destination"
}

[pscustomobject]@{
    ReleasePath = $copied.FullName
    Size = $copied.Length
    SHA256 = $copiedHash
    RuntimeTesting = 'Confirmed by caller; not independently verified by this script'
}
