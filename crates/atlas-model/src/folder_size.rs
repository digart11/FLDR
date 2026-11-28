//! Read-only recursive-size feedback, independent of UI and traversal.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SizeReport {
    pub bytes: u64,
    pub files: u64,
    /// Recursive subdirectories, excluding each explicitly scanned root.
    pub folders: u64,
    pub skipped: u64,
    pub errors: u64,
    pub first_error: Option<String>,
    pub cancelled: bool,
    pub done: bool,
}
