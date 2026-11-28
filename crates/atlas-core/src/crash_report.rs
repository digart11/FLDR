//! FLDR does not upload crash reports.
//! Windows crash diagnostics remain local.

use std::path::Path;

/// Remote crash-report submission is intentionally disabled.
pub async fn send(
    _reports: &[impl AsRef<Path>],
    _version: &str,
    _summary: Option<&str>,
) -> Result<String, String> {
    Err("Remote crash reporting is disabled in FLDR.".to_string())
}