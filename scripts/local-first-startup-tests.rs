//! Startup wiring guards. Run with rustc --test (no GPUI link required).
//! These complement the behavioral profile/path tests and runtime benchmark.
const STARTUP: &str = include_str!("../crates/atlas-ui/src/app/startup.rs");
const SIDEBAR: &str = include_str!("../crates/atlas-ui/src/ui/sidebar.rs");
const PATHS: &str = include_str!("../crates/atlas-core/src/fs/paths.rs");
const LIST: &str = include_str!("../crates/atlas-core/src/fs/list.rs");
const INDEX: &str = include_str!("../crates/atlas-core/src/fs/index.rs");

#[test]
fn no_automatic_wsl_discovery_or_process_launch() {
    for source in [STARTUP, SIDEBAR, PATHS, LIST, INDEX] {
        for forbidden in [
            "list_wsl_roots",
            "ensure_wsl_ready",
            "wsl.exe",
            "read_dir(WSL_LOCALHOST)",
        ] {
            assert!(
                !source.contains(forbidden),
                "automatic WSL path returned: {}",
                forbidden
            );
        }
    }
    assert!(!STARTUP.contains("ensure_linux_section"));
    assert!(SIDEBAR.contains("if section.kind == SectionKind::Linux {"));
    assert_eq!(SIDEBAR.matches("SectionKind::Linux => {}").count(), 2);
}

#[test]
fn startup_does_not_check_updates_or_launch_search_service() {
    for forbidden in [
        "check_for_update",
        "ensure_service_started",
        "find_search_service_exe",
        "Command::new",
    ] {
        assert!(
            !STARTUP.contains(forbidden),
            "startup action returned: {}",
            forbidden
        );
    }
    assert!(STARTUP.contains("build_index(goto_root.clone(), nucleo.injector())"));
    assert!(!STARTUP.contains("build_warm_navigation_index"));
}

#[test]
fn entitlement_and_usage_are_in_free_only_task() {
    let source = STARTUP.replace("\r\n", "\n");
    let task = source
        .split("#[cfg(feature = \"free\")]\n        rt.spawn(async {")
        .nth(1)
        .expect("entitlement task must be compile-time gated");
    let body = task.split("\n        });").next().unwrap();
    for call in [
        "atlas_core::usage::preload",
        "atlas_core::license::refresh().await",
    ] {
        assert!(body.contains(call));
        assert_eq!(source.matches(call).count(), 1);
    }
}

#[test]
fn saved_cloud_connections_are_loaded_without_mounting_or_warmup() {
    assert!(STARTUP.contains("load_cloud_remotes()"));
    assert!(STARTUP.contains("atlas_core::cloud::spawn"));
    for command in [
        "CloudCmd::Mount",
        "CloudCmd::Warmup",
        "spawn_list_remote",
        "navigate_remote",
    ] {
        assert!(!STARTUP.contains(command));
    }
    let cloud = include_str!("../crates/atlas-ui/src/app/cloud.rs");
    assert!(cloud.contains("CloudCmd::Mount { id }"));
    assert!(cloud.contains("fn navigate_remote"));
}

#[test]
fn startup_keeps_extension_archive_theme_and_shell_setup() {
    for required in [
        "Registry::load",
        "ensure_bundled",
        "fs::archive::open",
        "IconThemeIndex",
        "shell_menu::spawn",
        "setup_dnd",
        "load_profiles",
        "watch_path",
    ] {
        assert!(
            STARTUP.contains(required),
            "unrelated startup setup removed: {}",
            required
        );
    }
}
