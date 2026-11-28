param([string]$TargetDir = 'C:/Build/FileOP-native-tests')
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$sourceRoot = $projectRoot.Replace('\', '/')
$fixtureDir = Join-Path ([System.IO.Path]::GetTempPath()) ('FileOP-native-drag-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureDir | Out-Null

# Test only the production COM/bitmap modules. No Atlas executable or GPUI link.
# The two model/normalization stubs are unused by these local C:\ fixture paths.
@"
#![allow(dead_code)]
extern crate self as atlas_model;
extern crate self as atlas_core;
pub mod app_state { pub enum BgResult { ClipboardPerformed { effect:u32, paths:Vec<std::path::PathBuf> } } }
pub mod fs { pub mod paths { pub fn normalize_wsl_unc_paths(paths:Vec<std::path::PathBuf>)->Vec<std::path::PathBuf> { paths } } }
mod dnd { pub mod windows {
#[path = "$sourceRoot/crates/atlas-ui/src/dnd/windows/clipboard_formats.rs"] pub mod clipboard_formats;
#[path = "$sourceRoot/crates/atlas-ui/src/dnd/windows/data_object.rs"] pub mod data_object;
#[path = "$sourceRoot/crates/atlas-ui/src/dnd/windows/drag_image.rs"] pub mod drag_image;
} }
"@ | Set-Content -LiteralPath (Join-Path $fixtureDir 'lib.rs') -Encoding utf8
@'
[package]
name = "fileop-native-drag-tests"
version = "0.0.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
windows = { version = "=0.61.3", features = ["Win32_Foundation", "Win32_Security", "Win32_Graphics_Gdi", "Win32_System_Com", "Win32_System_Com_StructuredStorage", "Win32_System_Com_Urlmon", "Win32_System_Memory", "Win32_System_DataExchange", "Win32_System_Ole", "Win32_UI_Shell", "Win32_UI_Shell_Common", "Win32_UI_WindowsAndMessaging", "Win32_UI_HiDpi"] }
windows-core = "=0.61.2"
flume = "0.11"
tracing = "0.1"
'@ | Set-Content -LiteralPath (Join-Path $fixtureDir 'Cargo.toml') -Encoding utf8
# Retain the project's locked transitive versions, including already-locked yanked releases.
Copy-Item -LiteralPath (Join-Path $projectRoot 'Cargo.lock') -Destination (Join-Path $fixtureDir 'Cargo.lock')
Write-Output "Native-only fixture: $fixtureDir"
& cargo test --offline --manifest-path (Join-Path $fixtureDir 'Cargo.toml') --target-dir $TargetDir --lib -- --nocapture
exit $LASTEXITCODE
