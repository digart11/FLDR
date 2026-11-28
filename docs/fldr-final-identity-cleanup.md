# FLDR final identity cleanup

Source validation: 2026-11-20. This report covers only this cleanup, measured against the working tree at entry, including its existing uncommitted and untracked work. It supersedes the configuration/data-path and Win+E namespace portions of `fldr-rename-handoff.md`; that earlier report remains historical evidence.

## Result and boundaries

The existing FLDR executable name, localized application name, application/window identity, About display, shortcuts and Windows product metadata were audited and retained. Remaining theme-picker, theme-editor, onboarding, command-palette, Free-edition, generated subtitle and optional helper display labels now use FLDR. Theme identifiers, palette values, artwork, crate/module names, file formats, service protocols and functional settings remain intact.

No FLDR launch, live Win+E registry read/write, real shortcut migration, installer execution, installation, release build, commit, push, publication, dependency download or destructive cleanup was performed. Cargo ran offline and locked, with four jobs, using the existing external `C:\Build\FileOP` cache. Test executables and disposable fixtures were used. Existing installer and MSIX build guards remain in place; no installer, association, hotkey application or startup process was added.

## Configuration and data

| Purpose | New Windows location / identity |
| --- | --- |
| Configuration and roaming application data | `%APPDATA%\FLDR` |
| Explicit diagnostic configuration override | `FLDR_CONFIG_DIR` |
| Folder-statistics cache | `%LOCALAPPDATA%\FLDR\Cache\folder-statistics-v1.json` |
| Crash reports | `%LOCALAPPDATA%\FLDR\Crashes`, `fldr-<pid>-<timestamp>.*` |
| Cleanup history | `%LOCALAPPDATA%\FLDR\cleanup`, or `<FLDR_CONFIG_DIR>\cleanup` for isolated diagnostics |
| Optional search-service data | `%ProgramData%\FLDR\Search` |
| Optional application tool probe | `FLDR/tools` under the existing Program Files / LocalAppData probe roots |
| Configuration writer mutex | `Local\FLDRConfigMutex` |

`ATLAS_CONFIG_DIR` is no longer an application override. Its only remaining active-source occurrence is the negative configuration test. Diagnostic examples and runners were updated together so they continue selecting isolated profiles. Old Atlas/FileOP data and preferences are neither read as a fallback nor moved, copied, merged or deleted. The old settings schema, including the Win+E setting and optional performance settings, is unchanged.

## Win+E ownership and restoration

New ownership metadata uses `HKCU\Software\FLDR\Integration\WinE` and `FLDRCommand`. The Windows target CLSID and the setting/CLI entry points remain unchanged.

- Both default command and `DelegateExecute` must exactly match the saved ownership certificate before an owned registration can be updated or disabled.
- First registration only admits an absent/empty per-user command with an absent/empty delegate or the recognized native Windows delegate. Other handlers, including unowned Explorer-like command strings, are refused.
- Original value types, raw bytes and absence are serialized separately from ownership. Both originals must validate before restoration starts. Backups are cleared only after both target values restore successfully.
- Legacy `Software\Atlas\Integration\WinE` / `AtlasCommand` records remain readable. An exactly owned legacy handler can be disabled directly or handed off to FLDR without replacing its original backups with the installed Atlas command. Automatic refresh requires the same executable directory; explicit registration supports a deliberate directory change.
- During a successful handoff, originals are copied to the FLDR namespace before the target changes; only the three recognized legacy metadata values are then retired. Unknown values and subkeys remain. No registry tree is deleted.
- Missing, malformed, stale or conflicting ownership records fail safely. Equal duplicate certificates are supported. Unknown handlers and conflicting backups are preserved.
- A write journal rolls back failed updates/restores. Target ownership is rechecked between writes and during rollback. An intervening takeover stops further mutation and retains recovery metadata. A failed rollback stops before discarding the remaining recovery metadata.

The policy is tested against an in-memory registry, including injected write failures and intervening takeovers. Live Windows integration and GUI acceptance were deliberately not performed; OS access failures can still prevent a registry operation and are reported rather than concealed.

## Remaining Atlas references

These were deliberately retained and are not represented as removed:

| Surface | Remaining reference and reason |
| --- | --- |
| Executable legal metadata | `packaging/windows/fileop.rc`: `upstream (c) Atlas contributors` in LegalCopyright. The product, description and original filename already say FLDR. |
| About/upstream notice | `crates/atlas-ui/locales/en.yml` and `en-US.yml`: `Atlas contributors · AGPL-3.0-or-later`. |
| Extension authors | `extensions/{cloud,icon-manager,media-thumbs,runtime-tools}/extension.toml`: `Atlas Team`, retained attribution. |
| Legal documents and source availability | LICENSE, NOTICE, third-party notices, legal inventories and upstream source URLs retain their original authorship and addresses. |
| Optional companion filenames and service identity | `atlas-cloud-extension[.exe]`, `atlas-icon-extension[.exe]`, `atlas-search-service[.exe]`, `AtlasSearchService`. These remain compatibility identifiers used by loaders, packaging and the existing optional service. The service's human-readable display label now says FLDR. |
| Raw theme files and stored identifiers | `atlas-themes.json` and `Atlas ...` preset IDs remain unchanged. Settings, onboarding, theme editor and command-palette display labels say FLDR. |
| Internal formats and safety namespaces | Rust packages/modules, action/keybinding contexts, IPC identifiers, vault markers/cryptographic domains and index formats retain Atlas identifiers. Renaming these adds compatibility risk without changing the application display. |
| Upstream endpoints | Existing website/source/release, crash-report and licensing endpoints retain their Atlas addresses. No replacement endpoints were invented. |
| Historical or developer-only content | README, historical reports, upstream/Linux packaging, legacy artwork assets, diagnostic traces/tooling strings and existing build-script provenance messages still contain Atlas. README and artwork were explicitly protected. Existing disabled installer/MSIX templates retain upstream legal/source references and helper filenames. |

No non-legal Atlas app name remains in active locale display values. Existing FLDR icon/logo aliases were verified, and no artwork file was modified. Existing packaged-image assets remain untouched; the guarded MSIX script now references the supplied FLDR logo for future generation.

## Validation

| Check | Result |
| --- | --- |
| `cargo test -p atlas-win-space --lib integration::policy::tests --offline --locked -j 4` | 14 passed; in-memory registry only, including failure at every planned write in fresh enable, legacy handoff, current disable and legacy disable. |
| `cargo test -p atlas-core --test fldr_identity --test navigation_chrome --offline --locked -j 4` | 12 + 36 passed. Configuration cases run in separate test processes: default, empty override, explicit isolated override, and ignored legacy override; old fixture bytes remain untouched. |
| `cargo test -p atlas-core --lib --features free convert::edition_tests --offline --locked -j 4` | 3 passed; conversion limits and single-item behavior retained. |
| `cargo test -p atlas-core --lib --features free convert::subtitles::tests --offline --locked -j 4` | 5 passed; subtitle round trips remain valid with FLDR SAMI labels. |
| `cargo check -p atlas --bin FLDR --offline --locked -j 4` | Passed, default features. |
| Same FLDR check with `--no-default-features` | Passed. Existing unrelated compiler warnings remain. |
| `scripts/test-fldr-identity.ps1` | 62 checks passed; all six ICO frames match the compiled resource byte for byte. |
| `scripts/test-fldr-shortcuts.ps1` | Passed: preview/WhatIf, same-directory retargeting, AppUserModelID, icon, arguments, working directory, custom names, exact backups, foreign-target and repeat protection. Only disposable `.lnk` fixtures changed; no executable was launched. |
| Changed PowerShell scripts / MSIX manifest | Four scripts parse successfully; XML parses successfully. |
| Incremental whitespace checks | All edits pass against entry copies. Whole-tree `git diff --check` still reports two preexisting issues in untouched `.cargo/config.toml` and `packaging/windows/store-listings/de/description.md`; those files were preserved. |
| Optional publisher compile | Blocked offline by uncached `anstream v1.0.0`; no download attempted. Only display/description strings changed in its source and manifest. |
| Optional search-service compile | Blocked offline by uncached `windows-service v0.8.1`; no download attempted. Its scoped display/data-path changes therefore lack a fresh standalone compile result. |

All 70 executed Rust test cases passed. A passing source check and resource inspection do not establish live UI, integration or release acceptance.

## Preservation and evidence

The entry snapshot covers 2,191 existing tracked/untracked files. This task modified 60 of those files; all other 2,131 match their entry SHA-256 hashes, and no existing file is missing. In particular, every README, LICENSE, NOTICE, third-party license, artwork file, conflicted copy, Cargo.lock and `.cargo/config.toml` remains unchanged from entry. Folder-scanner, thumbnail, fast-recycle, transfer, rendering and other performance/UI implementations were preserved; the statistics cache edit changes its directory component only.

Original bytes for each edited existing file, the entry hash manifest and final test logs are retained at:

`C:\Users\Lucas\AppData\Local\Temp\fldr-final-identity-fa9fb7b167d146ed807dfccc3a09e9e8`

This is a development rollback/evidence snapshot, not a published artifact. No rollback or cleanup was executed. The disposable shortcut fixture is retained at:

`C:\Users\Lucas\AppData\Local\Temp\FLDR-shortcuts-6a6687a29d6d4467910f5b0c9a0f0a47`

## Every file changed by this task

The inventory includes files that were already dirty/untracked at entry. It records this task's incremental edits, not all preexisting working-tree changes. There are 60 modified existing files plus two new files, including this report.

- `crates/atlas-core/src/cleanup/execution.rs`
- `crates/atlas-core/src/convert/edition_tests.rs`
- `crates/atlas-core/src/convert/subtitles.rs`
- `crates/atlas-core/src/convert/tools.rs`
- `crates/atlas-core/src/fs/stats_cache.rs`
- `crates/atlas-core/src/license/mod.rs`
- `crates/atlas-core/src/search/volume_index.rs`
- `crates/atlas-core/src/util/config.rs`
- `crates/atlas-core/src/util/display.rs`
- `crates/atlas-core/src/vault/mount.rs`
- `crates/atlas-core/tests/fldr_identity.rs`
- `crates/atlas-model/src/edition.rs`
- `crates/atlas-publisher/Cargo.toml`
- `crates/atlas-publisher/src/main.rs`
- `crates/atlas-search-service/src/main.rs`
- `crates/atlas-search-service/src/mapped_index.rs`
- `crates/atlas-ui/examples/resize_diagnostic.rs`
- `crates/atlas-ui/src/app/vault/diagnostic.rs`
- `crates/atlas-ui/src/ui/cleanup/diagnostic.rs`
- `crates/atlas-ui/src/ui/overlays/getting_started.rs`
- `crates/atlas-ui/src/ui/palette/specs.rs`
- `crates/atlas-ui/src/ui/preview_lab.rs`
- `crates/atlas-ui/src/ui/settings_palette/mod.rs`
- `crates/atlas-ui/src/ui/shell_context_menu/diagnostic.rs`
- `crates/atlas-ui/src/ui/theme_editor.rs`
- `crates/atlas-ui/src/ui/views/chrome_diagnostic.rs`
- `crates/atlas-ui/src/ui/views/cloud_diagnostic.rs`
- `crates/atlas-ui/src/ui/views/selection_diagnostic.rs`
- `crates/atlas-ui/src/ui/views/transition/diagnostic.rs`
- `crates/atlas-win-space/src/crash.rs`
- `crates/atlas-win-space/src/crash/session.rs`
- `crates/atlas-win-space/src/integration.rs`
- `crates/atlas-win-space/src/integration_policy.rs`
- `crates/atlas-win-space/src/persistence.rs`
- `docs/fldr-final-identity-cleanup.md`
- `examples/miller_conversion_diagnostic.rs`
- `examples/preview_compat_diagnostic.rs`
- `examples/zip_docx_diagnostic.rs`
- `packaging/windows/atlas.iss`
- `packaging/windows/msix/Package.appxmanifest`
- `packaging/windows/store-listings/de/release_notes.txt`
- `packaging/windows/store-listings/en-US/release_notes.txt`
- `packaging/windows/store-listings/es-419/release_notes.txt`
- `packaging/windows/store-listings/fr/release_notes.txt`
- `packaging/windows/store-listings/it/release_notes.txt`
- `packaging/windows/store-listings/ja/release_notes.txt`
- `packaging/windows/store-listings/ko/release_notes.txt`
- `packaging/windows/store-listings/nl/release_notes.txt`
- `packaging/windows/store-listings/pl/release_notes.txt`
- `packaging/windows/store-listings/pt-BR/release_notes.txt`
- `packaging/windows/store-listings/ru/release_notes.txt`
- `packaging/windows/store-listings/sv/release_notes.txt`
- `packaging/windows/store-listings/tr/release_notes.txt`
- `packaging/windows/store-listings/uk/release_notes.txt`
- `packaging/windows/store-listings/vi/release_notes.txt`
- `packaging/windows/store-listings/zh-Hant/release_notes.txt`
- `scripts/build-windows-msix.ps1`
- `scripts/linux-smoke.sh`
- `scripts/measure-local-first-startup.ps1`
- `scripts/parity-idle-smoke.py`
- `scripts/run-pane-stress.ps1`
- `scripts/run-release-perf.ps1`
