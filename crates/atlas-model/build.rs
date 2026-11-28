#[path = "../../scripts/build-identity.rs"]
mod identity;
fn main() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    for path in ["src", "crates", "scripts", "packaging", "Cargo.toml", "Cargo.lock", "FLDR-icon.ico", "FLDR-logo.png", ".git/HEAD", ".git/index", ".git/refs"] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("build_identity.rs");
    std::fs::write(output, identity::generate(&root)).unwrap();
}
