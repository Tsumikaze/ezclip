//! Reading GitHub's "latest release" answer and comparing versions.
//! Pure logic, no Windows calls.

/// The GitHub repository, new name first. The old name ("ezclip") still works
/// before the repository is renamed, and GitHub redirects it afterwards.
pub const REPOS: [&str; 2] = ["Tsumikaze/shear", "Tsumikaze/ezclip"];
/// Release file names, preferred first ("ezclip.exe" for releases made before the rename).
pub const ASSETS: [&str; 2] = ["shear.exe", "ezclip.exe"];

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "0.5.0"
    pub version: String,
    /// Direct download of shear.exe.
    pub url: String,
    pub size: u64,
    /// Hex SHA-256 published by GitHub, if any.
    pub sha256: Option<String>,
    /// The release page (for "What's new").
    pub page: String,
}

/// Parse the JSON from `GET /repos/{REPO}/releases/latest`.
pub fn parse_latest(json: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let tag = v["tag_name"].as_str().ok_or("GitHub didn't say which version is the latest")?;
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let asset = ASSETS
        .iter()
        .find_map(|name| assets.iter().find(|a| a["name"].as_str() == Some(name)))
        .ok_or("the latest release has no shear.exe yet. Try again in a few minutes")?;
    let url = asset["browser_download_url"].as_str().ok_or("download link missing")?.to_string();
    // Only ever download from this project's releases on GitHub.
    if !REPOS.iter().any(|r| url.starts_with(&format!("https://github.com/{r}/releases/download/"))) {
        return Err("unexpected download location".into());
    }
    Ok(Release {
        version: tag.trim_start_matches('v').to_string(),
        url,
        size: asset["size"].as_u64().unwrap_or(0),
        sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|d| d.to_lowercase()),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
    })
}

fn parts(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect()
}

/// Is `latest` a newer version than `current`? ("0.10.0" > "0.9.3")
pub fn is_newer(latest: &str, current: &str) -> bool {
    parts(latest) > parts(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.5.0", "0.4.2"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.4.2", "0.4.2"));
        assert!(!is_newer("0.4.1", "0.4.2"));
    }

    #[test]
    fn parses_github_answer() {
        let json = r#"{"tag_name":"v0.6.0","html_url":"https://github.com/Tsumikaze/shear/releases/tag/v0.6.0",
            "assets":[{"name":"other.zip","browser_download_url":"https://x"},
                      {"name":"ezclip.exe","size":1,"browser_download_url":"https://github.com/Tsumikaze/shear/releases/download/v0.6.0/ezclip.exe"},
                      {"name":"shear.exe","size":4473856,"digest":"sha256:CC73EB",
                       "browser_download_url":"https://github.com/Tsumikaze/shear/releases/download/v0.6.0/shear.exe"}]}"#;
        let r = parse_latest(json).unwrap();
        assert_eq!(r.version, "0.6.0");
        assert_eq!(r.size, 4473856);
        assert_eq!(r.sha256.as_deref(), Some("cc73eb"));
        assert!(r.url.ends_with("/v0.6.0/shear.exe"), "prefers shear.exe");
        // releases from before the rename (old repo, ezclip.exe) are still accepted
        let old = r#"{"tag_name":"v0.5.0","assets":[{"name":"ezclip.exe","size":2,
            "browser_download_url":"https://github.com/Tsumikaze/ezclip/releases/download/v0.5.0/ezclip.exe"}]}"#;
        assert!(parse_latest(old).unwrap().url.ends_with("/ezclip.exe"));
        // a download link pointing anywhere else is refused
        let bad = json.replace("https://github.com/Tsumikaze/shear/releases/download/", "https://evil.example/");
        assert!(parse_latest(&bad).is_err());
        assert!(parse_latest(r#"{"tag_name":"v1.0.0","assets":[]}"#).is_err());
    }
}
