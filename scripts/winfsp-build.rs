// Resolve WinFsp only on first mount so Atlas still starts without its driver.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if std::env::var("CARGO_PKG_NAME").as_deref() == Ok("atlas") {
        embed_fileop_resources();
    }
    // The 1 MiB default main-thread stack overflows inside deep gpui layout
    // recursion (debug builds reliably, release builds on big trees). The
    // reservation is virtual; only touched pages are committed. This script is
    // shared with lib-only crates, where `-bins` link args are rejected.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
        && std::env::var("CARGO_PKG_NAME").as_deref() == Ok("atlas")
    {
        println!("cargo:rustc-link-arg-bins=/STACK:16777216");
        // Embed only the PDB's file name, not the build machine's path.
        println!("cargo:rustc-link-arg-bins=/PDBALTPATH:%_PDB%");
    }
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let dll = match arch.as_str() {
        "x86_64" => "winfsp-x64.dll",
        "aarch64" => "winfsp-a64.dll",
        "x86" => "winfsp-x86.dll",
        _ => return,
    };
    println!("cargo:rustc-link-lib=dylib=delayimp");
    println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
}

// Only the launcher embeds application resources. This build script is also
// used by library/sidecar crates, which must keep their existing link behavior.
fn embed_fileop_resources() {
    use std::{env, path::PathBuf, process::Command};
    for path in ["packaging/windows/fileop.rc", "FLDR-icon.ico"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"))
        .join("fileop.res");
    let status = Command::new("rc.exe")
        .current_dir(&root)
        .arg("/nologo")
        .arg("/fo")
        .arg(&output)
        .arg("packaging/windows/fileop.rc")
        .status()
        .expect("Windows SDK rc.exe is required; build in a VS Developer PowerShell");
    assert!(status.success(), "FLDR resource compilation failed");
    println!("cargo:rustc-link-arg-bin=FLDR={}", output.display());
}
