pub mod index;
pub mod list;
pub mod scan_policy;
pub mod dir_stats;
pub mod mounts;
pub mod network;
#[cfg(windows)]
pub mod windows_places;
pub mod paths;
pub mod tag_db;
pub mod udisks;
pub mod desktop_apps;

pub mod compress;
pub mod ops;
pub mod transfer;
pub mod transfer_safety;

pub mod recycle_bin;
pub mod watcher;

pub mod archive;
pub mod preview;
pub mod properties;

pub mod folder_size;
pub mod folder_compare;

pub mod stats_cache;
