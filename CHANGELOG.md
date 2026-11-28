# Changelog

All notable changes to FLDR are documented here.

## [Unreleased]

## [1.0.0] - 2026-11-27

First FLDR 1.0 release.

### New features

- Independent windows and detachable tabs.
- Window, tab, and session restoration.
- Folder comparison with optional SHA-256 content verification.
- Compact, color-coded folder comparison labels.
- Folder-specific view and sorting preferences.
- Adaptive column layouts in split panes.
- Drag-and-drop navigation through tabs and breadcrumbs.
- Native Dropbox and Seafile folder icon recognition.
- Improved SMB server and shared-folder browsing.
- Optional Windows Win+E integration.
- Seven FLDR themes.
- Zebra-striped Details view and configurable row spacing.
- Folder-size display in Large Icons view.
- Control over file extension visibility.

### Performance

- Optimized WIC JPEG thumbnail decoding.
- Improved directory listing performance.
- Faster folder-size calculations.
- Fast Recycle for eligible local file operations.
- Deferred loading of inactive restored tabs.
- Removed unused startup navigation indexing.
- Reduced unnecessary background workers.
- Improved shell-menu worker lifecycle management.
- Reduced redundant configuration and theme writes.

### Windows integration

- Improved native Windows Shell file operations.
- Refined file-type and folder icons.
- Improved native context menu handling.
- Improved navigation between local and network locations.

### Interface

- Refined tabs, sidebar, and navigation controls.
- Improved split-pane layouts.
- Refined folder comparison indicators.
- Improved appearance consistency.

### Privacy and local-first operation

- Removed automatic commercial entitlement checks.
- Disabled automatic telemetry reporting.
- Disabled automatic cloud mounting and update checks.
- Removed unnecessary startup network discovery.
- Removed legacy and unused interface components.

### Distribution

- Standalone Windows executable.
- Portable distribution without an installer.
- No external copy manager required.

### Validation

- Fast Recycle tested with 20,000 files and hash-verified restoration.
- JPEG thumbnail and directory-listing improvements benchmarked.
- Multiple windows and restored tabs tested.
- Optimized release build verified.

---

## Upstream Atlas history

The following entries document releases of the original Atlas project.
Features described below may have been changed or removed in FLDR.

## [0.1.5] - 2026-11-05

### New

- Settings redesigned as a searchable palette, with French and more languages selectable.
- Atlas Ember theme, theme editor improvements and new interface icons.
- Local file conversion with a radial menu, including FLAC, H.264/AAC video and optional Opus.
- Quick actions ring for converting and acting on selected files.
- Atlas Free and Atlas Premium. Premium unlocks with a license key, or in-app through the Microsoft Store in regional-price markets.

### Fixed

- Crash on launch caused by some shell extensions, such as Google Drive for desktop (#1, #3).
- Opening sub-folders on SMB and other cloud connections (#2).
- Typing into the fields of the add-connection dialog.

### Stability

- Crash reports with symbolized stacks that can be sent from the app.
- Recovery from worker failures and poisoned locks instead of closing the app.

## [0.1.0] - 2026-10-11

First public alpha.

### Highlights

- Native, GPU-accelerated file manager for Windows built with Rust and GPUI.
- Grid, List, Details, Miller columns and Tree views; tabs and split panes.
- Command palette, fuzzy go-to, indexed search and previews.
- Local file conversion, archives browsed as folders, tags and profiles.
- Optional first-party extensions (icon themes, thumbnails, Pandoc, Opus, cloud drives, document previews).
- Experimental Linux packaging (Debian/Ubuntu, Fedora/openSUSE, Arch, portable); not yet a supported release.
