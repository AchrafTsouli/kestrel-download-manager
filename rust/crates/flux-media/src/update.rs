//! Keeps a newer yt-dlp beside the user's data (sites change often; the bundled copy ages). The
//! bundled tools are never modified; a copy is used only after its SHA-256 matches the release's
//! published checksums and it runs and reports the version it claims. Same rules as 1.x.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

pub const REPOSITORY: &str = "https://github.com/yt-dlp/yt-dlp/releases";

/// A yt-dlp version ("2026.08.19" or "2026.08.19.1") as comparable numbers.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32, u32)> {
    let parts: Vec<&str> = text.trim().split('.').collect();
    if !(3..=4).contains(&parts.len()) || parts[0].len() != 4 || parts.iter().any(|p| p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit())) || parts[1].len() > 2 || parts[2].len() > 2 {
        return None;
    }
    let n = |i: usize| parts.get(i).map_or(Some(0), |p| p.parse().ok());
    Some((n(0)?, n(1)?, n(2)?, n(3)?))
}

/// The version in the tools folder's `sources.json` (the installer's manifest).
pub fn bundled_version(tools: &Path) -> Option<(u32, u32, u32, u32)> {
    let text = std::fs::read_to_string(tools.join("sources.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    parse_version(json["ytDlp"]["version"].as_str()?)
}

pub fn installed_version(update_dir: &Path) -> Option<(u32, u32, u32, u32)> {
    if !update_dir.join("yt-dlp.exe").exists() {
        return None;
    }
    parse_version(&std::fs::read_to_string(update_dir.join("yt-dlp.version")).ok()?)
}

/// The updated copy when it is newer than the bundled one.
pub fn preferred(tools: &Path, update_dir: &Path) -> Option<PathBuf> {
    let installed = installed_version(update_dir)?;
    bundled_version(tools).is_none_or(|bundled| installed > bundled).then(|| update_dir.join("yt-dlp.exe"))
}

async fn read(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("Could not reach GitHub: {e}"))?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return Err("The download is larger than expected.".into());
        }
    }
    Ok(body)
}

/// Installs a newer yt-dlp if one exists. Returns the new version, or `None` when already current.
pub async fn update(tools: &Path, update_dir: &Path) -> Result<Option<String>, String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(300)).user_agent("Flux").build().map_err(|e| e.to_string())?;
    // /releases/latest redirects to /releases/tag/<version>.
    let response = client.get(format!("{REPOSITORY}/latest")).send().await.map_err(|e| format!("Could not reach GitHub: {e}"))?;
    let tag = response.url().path().rsplit_once("/releases/tag/").map(|(_, t)| t.to_string()).ok_or("Could not read the latest yt-dlp release.")?;
    let latest = parse_version(&tag).ok_or("Could not read the latest yt-dlp release.")?;
    let current = [installed_version(update_dir), bundled_version(tools)].into_iter().flatten().max();
    if current.is_some_and(|c| latest <= c) {
        return Ok(None);
    }
    let sums = String::from_utf8_lossy(&read(&client, &format!("{REPOSITORY}/download/{tag}/SHA2-256SUMS"), 64 * 1024).await?).into_owned();
    let expected = sums
        .lines()
        .filter_map(|line| line.trim().split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == "yt-dlp.exe")
        .map(|(hash, _)| hash.to_ascii_lowercase())
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("The release has no valid checksum for yt-dlp.exe.")?;
    let executable = read(&client, &format!("{REPOSITORY}/download/{tag}/yt-dlp.exe"), 200 * 1024 * 1024).await?;
    let actual: String = Sha256::digest(&executable).iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err("The downloaded yt-dlp does not match its published checksum.".into());
    }
    std::fs::create_dir_all(update_dir).map_err(|e| e.to_string())?;
    let staging = update_dir.join(format!("yt-dlp.{}.tmp.exe", std::process::id()));
    std::fs::write(&staging, &executable).map_err(|e| e.to_string())?;
    // The copy must actually run and report the version it claims.
    let mut command = tokio::process::Command::new(&staging);
    command.arg("--version").kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output()).await;
    let verified = matches!(&output, Ok(Ok(o)) if o.status.success() && parse_version(&String::from_utf8_lossy(&o.stdout)) == Some(latest));
    if !verified {
        let _ = std::fs::remove_file(&staging);
        return Err("The updated yt-dlp did not report the expected version.".into());
    }
    std::fs::rename(&staging, update_dir.join("yt-dlp.exe")).map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        e.to_string()
    })?;
    std::fs::write(update_dir.join("yt-dlp.version"), &tag).map_err(|e| e.to_string())?;
    Ok(Some(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_dates() {
        assert_eq!(parse_version("2026.08.19\n"), Some((2026, 8, 19, 0)));
        assert!(parse_version("2026.08.19.1") > parse_version("2026.08.19"));
        assert!(parse_version("2026.10.01") > parse_version("2026.08.19.3"));
        for bad in ["", "1.2.3", "2026.08", "2026.08.19.1.2", "2026.x.19", "2026.123.1"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_newer_installed_copy_is_preferred() {
        let root = std::env::temp_dir().join(format!("flux-media-update-{}", std::process::id()));
        let (tools, updates) = (root.join("tools"), root.join("updates"));
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::create_dir_all(&updates).unwrap();
        std::fs::write(tools.join("sources.json"), r#"{"ytDlp":{"version":"2026.08.19"}}"#).unwrap();
        assert_eq!(preferred(&tools, &updates), None);
        std::fs::write(updates.join("yt-dlp.exe"), b"").unwrap();
        std::fs::write(updates.join("yt-dlp.version"), "2026.08.19").unwrap();
        assert_eq!(preferred(&tools, &updates), None);
        std::fs::write(updates.join("yt-dlp.version"), "2026.09.02").unwrap();
        assert_eq!(preferred(&tools, &updates), Some(updates.join("yt-dlp.exe")));
        let _ = std::fs::remove_dir_all(root);
    }
}
