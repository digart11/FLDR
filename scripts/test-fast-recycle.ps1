# Isolated tests compile the production module without linking the GUI. All mutations
# are confined to newly created Temp fixtures; no test calls the live-bin executor.
param([string]$BuildRoot = 'C:\Build\FileOP', [switch]$Benchmark)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$harness = Join-Path $BuildRoot 'experiments\fast-recycle-unit-harness'
New-Item -ItemType Directory -Force -Path (Join-Path $harness 'src') | Out-Null
$manifest = @'
[package]
name = "fldr-fast-recycle-tests"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
windows = { version = "=0.61.3", features = ["Win32_Foundation", "Win32_Security", "Win32_Security_Authorization", "Win32_Storage_FileSystem", "Win32_System_Registry", "Win32_System_Threading", "Win32_System_Com", "Win32_System_Com_StructuredStorage", "Win32_UI_Shell", "Win32_UI_Shell_Common", "Win32_UI_WindowsAndMessaging"] }
'@
Set-Content -LiteralPath (Join-Path $harness 'Cargo.toml') -Value $manifest -Encoding UTF8
$source = (Join-Path $repo 'crates\atlas-ui\src\fast_recycle.rs').Replace('\','/')
Set-Content -LiteralPath (Join-Path $harness 'src\lib.rs') -Value ('#[allow(dead_code)] #[path = "' + $source + '"] mod fast_recycle;') -Encoding UTF8
$env:CARGO_TARGET_DIR = $BuildRoot
$env:CARGO_BUILD_JOBS = '4'
$testArgs = @('--nocapture')
if ($Benchmark) { $testArgs += '--ignored' }
$ErrorActionPreference = 'Continue' # Cargo diagnostics use stderr, including on success.
& cargo +1.97.1 test --manifest-path (Join-Path $harness 'Cargo.toml') --offline -j 4 -- @testArgs
$cargoExit = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($cargoExit -ne 0) { throw "Native recycle tests failed ($cargoExit)" }
