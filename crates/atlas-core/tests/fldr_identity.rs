//! Pure identity, scrolling and tab geometry checks without linking a GUI.
#![allow(dead_code)]
#[path = "../../../scripts/build-identity.rs"]
mod build_identity;
#[path = "../../atlas-ui/src/ui/settings_palette/category_scroll.rs"]
mod category_scroll;
#[path = "../../atlas-ui/src/ui/tab_shape.rs"]
mod tab_shape;

#[test]
fn source_identity_is_stable_without_git_and_changes_with_source() {
    assert!(!atlas_core::update::APP_VERSION.is_empty());
    let fixture = tempfile::tempdir().unwrap();
    std::fs::create_dir(fixture.path().join("src")).unwrap();
    let file = fixture.path().join("src/main.rs");
    std::fs::write(&file, "fn main() {}").unwrap();
    let before = build_identity::fingerprint(fixture.path());
    assert_eq!(before, build_identity::fingerprint(fixture.path()));
    std::fs::write(&file, "fn main() { println!(\"FLDR\"); }").unwrap();
    assert_ne!(before, build_identity::fingerprint(fixture.path()));
    let generated = build_identity::generate(fixture.path());
    assert!(generated.contains("no-git"));
    assert!(generated.contains("unknown"));
}

// Each environment case runs in a separate test process. No shared process
// environment mutation, GUI initialization, or real user preference writes.
#[test]
fn configuration_paths_use_fldr_and_ignore_legacy_override() {
    let fixture = tempfile::tempdir().unwrap();
    let legacy = fixture.path().join("atlas");
    std::fs::create_dir(&legacy).unwrap();
    let sentinel = legacy.join("settings.json");
    std::fs::write(&sentinel, b"legacy preferences must remain untouched").unwrap();
    for case in ["default", "empty", "override"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", "configuration_environment_child", "--nocapture"])
            .env("FLDR_IDENTITY_TEST_CASE", case)
            .env("ATLAS_CONFIG_DIR", &legacy)
            .env_remove("FLDR_CONFIG_DIR");
        if case == "empty" { child.env("FLDR_CONFIG_DIR", ""); }
        if case == "override" { child.env("FLDR_CONFIG_DIR", fixture.path().join("FLDR isolated profile")); }
        let output = child.output().unwrap();
        assert!(output.status.success(), "{case}: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"legacy preferences must remain untouched");
    assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 1);
}

#[test]
fn configuration_environment_child() {
    let Ok(case) = std::env::var("FLDR_IDENTITY_TEST_CASE") else { return; };
    use atlas_core::util::config;
    let expected = if case == "override" {
        std::path::PathBuf::from(std::env::var_os("FLDR_CONFIG_DIR").unwrap())
    } else {
        dirs::config_dir().unwrap_or_else(|| ".".into()).join("FLDR")
    };
    assert_eq!(config::config_dir(), expected);
    assert_eq!(config::themes_dir(), expected.join("themes"));
    assert_eq!(config::data_dir(), dirs::data_dir().unwrap_or_else(|| ".".into()).join("FLDR"));
    assert_eq!(atlas_core::fs::stats_cache::path(), dirs::data_local_dir()
        .map(|base| base.join("FLDR/Cache/folder-statistics-v1.json")));
}
