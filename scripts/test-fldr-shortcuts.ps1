$ErrorActionPreference = 'Stop'
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('FLDR-shortcuts-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $fixture)
$exe = Join-Path $fixture 'FLDR.exe'
[IO.File]::WriteAllText($exe, 'Disposable fixture bytes. Never execute.')
$shell = New-Object -ComObject WScript.Shell
$old = Join-Path $fixture 'FileOP.lnk'
$custom = Join-Path $fixture 'Custom.lnk'
$foreign = Join-Path $fixture 'Other.lnk'
foreach ($path in @($old, $custom, $foreign)) {
    $link = $shell.CreateShortcut($path)
    $link.TargetPath = if ($path -eq $foreign) { 'C:\Other\FileOP.exe' } else { Join-Path $fixture 'FileOP.exe' }
    $link.Arguments = '"C:\folder with spaces"'
    $link.WorkingDirectory = $fixture
    $link.IconLocation = "$($link.TargetPath),0"
    $link.Save()
}
$migrate = Join-Path $PSScriptRoot 'migrate-fldr-shortcuts.ps1'
$before = (Get-FileHash -LiteralPath $old).Hash
& $migrate -Executable $exe -ShortcutDirectory $fixture | Out-Null
if ((Get-FileHash -LiteralPath $old).Hash -ne $before) { throw 'Preview changed a shortcut.' }
& $migrate -Executable $exe -ShortcutDirectory $fixture -Apply -WhatIf | Out-Null
if (Test-Path -LiteralPath ($old + '.fldr-rename.bak')) { throw 'WhatIf wrote a backup.' }
& $migrate -Executable $exe -ShortcutDirectory $fixture -Apply -Confirm:$false | Out-Null
$renamed = Join-Path $fixture 'FLDR.lnk'
foreach ($path in @($renamed, $custom)) {
    $link = $shell.CreateShortcut($path)
    if ($link.TargetPath -ne $exe -or $link.Arguments -ne '"C:\folder with spaces"' -or $link.WorkingDirectory -ne $fixture) { throw 'Shortcut fields were not preserved.' }
    if ($link.IconLocation -ne "$exe,0") { throw 'Application icon was not updated.' }
    if ([FldrShortcutIdentity]::Read($path) -ne 'digART.FLDR') { throw 'Shortcut AppUserModelID was not updated.' }
}
if ((Get-FileHash -LiteralPath ($old + '.fldr-rename.bak')).Hash -ne $before) { throw 'Backup changed.' }
if ($shell.CreateShortcut($foreign).TargetPath -ne 'C:\Other\FileOP.exe') { throw 'Foreign shortcut changed.' }
$blocked = $false
& $migrate -Executable $exe -ShortcutDirectory $fixture -Apply -Confirm:$false | Out-Null
if ((Get-FileHash -LiteralPath ($old + '.fldr-rename.bak')).Hash -ne $before) { throw 'Repeat changed the backup.' }
[IO.File]::Copy($old + '.fldr-rename.bak', $old, $false)
try { & $migrate -Executable $exe -ShortcutDirectory $fixture -Apply -Confirm:$false | Out-Null } catch { $blocked = $true }
if (-not $blocked) { throw 'Existing backup was not protected.' }
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell)
Write-Output "Shortcut fixture passed: preview/WhatIf, name/target/icon migration, custom name/arguments/working directory, byte-identical backup, foreign shortcut and repeat protection. Retained at $fixture"
