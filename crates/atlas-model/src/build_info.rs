//! Identity of the compiled artifact. Never queries a runtime checkout.
include!(concat!(env!("OUT_DIR"), "/build_identity.rs"));
pub const APP_NAME: &str = "FLDR";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub fn version_information() -> String {
    format!("{APP_NAME} {VERSION}\nRevision: {REVISION} ({TREE_STATE})\nSource: {SOURCE_ID}\nBuild: {BUILD_PROFILE} / {BUILD_TARGET}\nBuilt: {BUILD_DATE}\n© 2026 LW")
}
#[cfg(test)]
mod tests {
    #[test]
    fn identity_is_embedded_and_distinguishes_development_builds() {
        let info = super::version_information();
        assert!(info.starts_with("FLDR "));
        assert_eq!(super::SOURCE_ID.len(), 16);
        assert!(info.contains(super::BUILD_DATE));
        assert!(matches!(super::TREE_STATE, "clean" | "dirty" | "unknown"));
    }
}
