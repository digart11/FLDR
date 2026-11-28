[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Text fixtures only: never execute an application or write into real Releases.
$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('FileOP release test ' + [guid]::NewGuid())
$fixtureScripts = Join-Path $fixtureRoot 'scripts'
[void][IO.Directory]::CreateDirectory($fixtureScripts)
$publisher = Join-Path $fixtureScripts 'save-tested-release.ps1'
[IO.File]::Copy((Join-Path $PSScriptRoot 'save-tested-release.ps1'), $publisher)
$inputFile = Join-Path $fixtureRoot 'fixture [1].exe'
[IO.File]::WriteAllText($inputFile, 'Not executable: isolated publication test fixture.')
$script:checks = 0
function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:checks++
}
function Assert-Rejected([scriptblock]$Action) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    Assert-True $rejected 'Expected publisher rejection.'
}

Assert-Rejected { & $publisher -Executable $inputFile -ReleaseLabel 'fixture' }
Assert-True (-not (Test-Path -LiteralPath (Join-Path $fixtureRoot 'Releases'))) 'Unconfirmed fixture was copied.'
Assert-Rejected { & $publisher -Executable (Join-Path $fixtureRoot 'missing.exe') -ReleaseLabel 'fixture' -RuntimeTested }
Assert-Rejected { & $publisher -Executable $inputFile -ReleaseLabel '../escape' -RuntimeTested }
Assert-Rejected { & $publisher -Executable $fixtureRoot -ReleaseLabel 'fixture' -RuntimeTested }
$empty = Join-Path $fixtureRoot 'empty.exe'
[IO.File]::WriteAllBytes($empty, [byte[]]@())
Assert-Rejected { & $publisher -Executable $empty -ReleaseLabel 'fixture' -RuntimeTested }
$text = Join-Path $fixtureRoot 'input.txt'
[IO.File]::WriteAllText($text, 'text')
Assert-Rejected { & $publisher -Executable $text -ReleaseLabel 'fixture' -RuntimeTested }

# RuntimeTested here exercises the gate on fake bytes, not a claim about FileOP.
$result = & $publisher -Executable $inputFile -ReleaseLabel 'fixture' -RuntimeTested
$expected = Join-Path $fixtureRoot 'Releases\FLDR-fixture.exe'
Assert-True ($result.ReleasePath -eq $expected) 'Repository-relative release destination is wrong.'
Assert-True ($result.Size -eq (Get-Item -LiteralPath $inputFile).Length) 'Size mismatch.'
Assert-True ($result.SHA256 -eq (Get-FileHash -LiteralPath $inputFile -Algorithm SHA256).Hash) 'Hash mismatch.'
$before = (Get-Item -LiteralPath $expected).LastWriteTimeUtc
$again = & $publisher -Executable $inputFile -ReleaseLabel 'fixture' -RuntimeTested
Assert-True ($again.SHA256 -eq $result.SHA256 -and (Get-Item -LiteralPath $expected).LastWriteTimeUtc -eq $before) 'Identical release was overwritten.'
[IO.File]::WriteAllText($inputFile, 'Different fake bytes.')
Assert-Rejected { & $publisher -Executable $inputFile -ReleaseLabel 'fixture' -RuntimeTested }
Assert-True ((Get-FileHash -LiteralPath $expected -Algorithm SHA256).Hash -eq $result.SHA256) 'Previous release was changed.'
$second = & $publisher -Executable $inputFile -ReleaseLabel 'fixture-2' -RuntimeTested
Assert-True ((Test-Path -LiteralPath $expected) -and (Test-Path -LiteralPath $second.ReleasePath)) 'Previous release was lost.'
Assert-True (Test-Path -LiteralPath $inputFile) 'Source file was removed.'
Write-Output "$script:checks checks passed. Fake fixtures retained at: $fixtureRoot"
