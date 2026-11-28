# FLDR Packaging

FLDR is distributed as a standalone Windows x64 application.

## Build

Requirements:
- Rust 1.97.1
- Microsoft C++ Build Tools
- Windows SDK

From a configured developer PowerShell:

    cargo +1.97.1 build --release --locked -p atlas --bin FLDR

The executable is FLDR.exe in the configured Cargo release directory.

## Distribution

- Portable ZIP distribution.
- No installer or Microsoft Store package.
- No automatic file associations.
- Optional Win+E integration through application settings.
- No external copy manager required.

Publish the tested executable with applicable license notices,
matching corresponding source and SHA-256 checksums.

Official releases:
https://github.com/digart11/FLDR/releases

Source:
https://github.com/digart11/FLDR

## Windows resources

The Windows build uses packaging/windows/fileop.rc and FLDR-icon.ico.
These are required build inputs despite the historical resource filename.

See LICENSE, NOTICE and legal/ for licensing information.
