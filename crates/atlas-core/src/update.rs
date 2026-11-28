use anyhow::Result;
use serde::Deserialize;



pub const APP_VERSION: &str = match option_env!("ATLAS_RELEASE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

pub const RELEASES_URL: &str = match option_env!("ATLAS_RELEASES_URL") {
    Some(url) => url,
    None => "https://github.com/digart11/FLDR/releases",
};

pub const SOURCE_URL: &str = match option_env!("ATLAS_SOURCE_URL") {
    Some(url) => url,
    None => "https://github.com/digart11/FLDR",
};


pub struct AppUpdate {
    pub version: String,
    pub url: String,
}

#[cfg(windows)]
#[derive(Debug, Deserialize)]
pub struct AppUpdateManifest {
    pub version: String,
    pub notes: String,
    pub windows: WindowsUpdate,
}

#[cfg(windows)]
#[derive(Debug, Deserialize)]
pub struct WindowsUpdate {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[cfg(not(windows))]
#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
}

fn is_newer(current: &str, candidate: &str) -> Result<bool> {
    let current = semver::Version::parse(current.trim_start_matches('v'))?;
    let candidate = semver::Version::parse(candidate.trim_start_matches('v'))?;
    Ok(candidate.cmp_precedence(&current).is_gt())
}

pub async fn check_for_update(_current_version: &str) -> Result<Option<AppUpdate>> {
    // FLDR is local-first: never contact legacy Atlas update servers.
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn version_precedence_prevents_downgrade_notices() {
        assert!(is_newer("0.9.0", "v0.10.0").unwrap());
        assert!(!is_newer("0.10.0", "0.9.0").unwrap());
        assert!(!is_newer("0.10.0", "0.10.0").unwrap());
        assert!(!is_newer("1.0.0+local", "1.0.0+other").unwrap());
        assert!(is_newer("1.0.0-rc.1", "1.0.0").unwrap());
        assert!(!is_newer("1.0.0", "1.0.0-rc.1").unwrap());
        assert!(is_newer("invalid", "1.0.0").is_err());
    }
}
