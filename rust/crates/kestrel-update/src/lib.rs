//! Kestrel's own updates: a small JSON feed (`version.json`) signed with the release key. The key, the
//! signed text and the rules are Flux 1.x's (`AppUpdates`), so 1.x installs 2.0 and 2.0 keeps
//! updating itself. Only a signed release is installed; an unsigned one can only be opened in the
//! browser.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use sha2::{Digest, Sha256};

/// ECDSA P-256 public key (SubjectPublicKeyInfo, Base64), from `SwiftFetch.Core/UpdateSigningKey.cs`.
pub const PUBLIC_KEY: &str = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEr4A5xVXDDg3DgMK352ttyolZka2KjRdAOzMP3Ng4S/gfjQ1lnLgg7LQWpg3HdgPFevqIJx7QHQpliiVZ3enskQ==";
/// The release key since Kestrel 2.1 (the first one was lost). Its signature is in the feed's
/// "signature2" field, which copies before 2.1 do not read: for them the release is unsigned, so
/// they offer its download page instead of refusing it.
pub const PUBLIC_KEY_2: &str = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE+PyReQ8uS5KEhNJ2bepnRe5OONiqc338y5c6WK3GE/5eN4WYEzFSHYbtXbO/njtBphytrWKEiHZFO7vHskj0zw==";
pub const DEFAULT_FEED: &str = "https://github.com/AchrafTsouli/kestrel-download-manager/releases/latest/download/version.json";

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: String,
    /// The signature verified against the built-in key: it may be installed automatically.
    pub signed: bool,
}

/// What the release key signs.
pub fn payload(version: &str, url: &str, sha256: &str) -> Vec<u8> {
    format!("apex-update\n{version}\n{url}\n{}", sha256.to_ascii_lowercase()).into_bytes()
}

/// A version as .NET reads it (2 to 4 numbers); a pre-release suffix ("-beta.1") is ignored.
pub fn parse_version(text: &str) -> Option<[u32; 4]> {
    let core = text.trim().split(['-', '+']).next()?;
    let parts: Vec<&str> = core.split('.').collect();
    if !(2..=4).contains(&parts.len()) {
        return None;
    }
    let mut version = [0u32; 4];
    for (slot, part) in version.iter_mut().zip(&parts) {
        if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    Some(version)
}

/// True when `release` is newer than the running version.
pub fn is_newer(release: &str, current: &str) -> bool {
    matches!((parse_version(release), parse_version(current)), (Some(r), Some(c)) if r > c)
}

/// Reads the published feed: "signature2" checked with the current key, else "signature" with the first.
pub fn parse_feed(json: &[u8]) -> Result<Release, String> {
    let root: serde_json::Value = serde_json::from_slice(json).map_err(|_| "The update information is not readable.")?;
    if root.get("signature2").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()) {
        parse_field(json, "signature2", PUBLIC_KEY_2)
    } else {
        parse(json, PUBLIC_KEY)
    }
}

/// Reads a feed. A release can be installed automatically only when its signature verifies.
pub fn parse(json: &[u8], public_key: &str) -> Result<Release, String> {
    parse_field(json, "signature", public_key)
}

fn parse_field(json: &[u8], field: &str, public_key: &str) -> Result<Release, String> {
    let root: serde_json::Value = serde_json::from_slice(json).map_err(|_| "The update information is not readable.")?;
    let text = |name: &str| root.get(name).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let version = text("version");
    let url = Some(text("url")).filter(|u| !u.is_empty()).unwrap_or_else(|| text("downloadUrl"));
    let sha256 = text("sha256").to_ascii_lowercase();
    let signature = text(field);
    if parse_version(&version).is_none() {
        return Err("Invalid release version.".into());
    }
    let link = reqwest::Url::parse(&url).map_err(|_| "The release link must use HTTPS.")?;
    if link.scheme() != "https" || !link.username().is_empty() || link.password().is_some() {
        return Err("The release link must use HTTPS.".into());
    }
    let mut signed = false;
    if !public_key.is_empty() && !signature.is_empty() {
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid release checksum.".into());
        }
        let key = BASE64.decode(public_key).ok().and_then(|der| VerifyingKey::from_public_key_der(&der).ok()).ok_or("The built-in update key is not valid.")?;
        let signature = BASE64.decode(&signature).ok().and_then(|bytes| Signature::from_slice(&bytes).ok()).ok_or("Invalid release signature.")?;
        key.verify(&payload(&version, &url, &sha256), &signature).map_err(|_| "The release signature is not valid. The update was rejected.")?;
        signed = true;
    }
    Ok(Release { version, url, sha256, signed })
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder().timeout(timeout).user_agent("Kestrel").build().map_err(|e| e.to_string())
}

async fn read(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("Could not reach the update server: {e}"))?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return Err("The download is larger than expected.".into());
        }
    }
    Ok(body)
}

/// Downloads and reads the feed (at most 64 KB).
pub async fn check(feed: &str) -> Result<Release, String> {
    let feed = if feed.trim().is_empty() { DEFAULT_FEED } else { feed.trim() };
    if !feed.starts_with("https://") {
        return Err("The update feed must use HTTPS.".into());
    }
    parse_feed(&read(&client(Duration::from_secs(30))?, feed, 64 * 1024).await?)
}

/// Downloads a signed installer and checks it against the signed checksum before it can run.
pub async fn download_installer(release: &Release) -> Result<PathBuf, String> {
    if !release.signed {
        return Err("Only signed releases can be installed automatically.".into());
    }
    let installer = read(&client(Duration::from_secs(600))?, &release.url, 300 * 1024 * 1024).await?;
    let actual: String = Sha256::digest(&installer).iter().map(|b| format!("{b:02x}")).collect();
    if actual != release.sha256 {
        return Err("The downloaded installer does not match the signed checksum.".into());
    }
    let folder = std::env::temp_dir().join(format!("kestrel-update-{}", std::process::id()));
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(format!("Kestrel-Setup-{}.exe", release.version));
    std::fs::write(&path, installer).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The feed Flux 1.0.0 was published with, signed by the release key (kept word for word: its old name is part of what is signed).
    const FEED_1_0_0: &str = r#"{
  "version": "1.0.0",
  "url": "https://github.com/AchrafTsouli/flux-downloader/releases/download/v1.0.0/Flux-Setup-1.0.0-win-x64.exe",
  "sha256": "a1018deddc275afa661ebef81374c896fc7c11607bd6a0d2001e2a9d74f33a6f",
  "signature": "RcU3GgN25plSFXQ2Ls8k5obeidfZRLjElEBZ5RgifskFarIe4TyMYcxJ2j2+X01MPHVeebDHSVHjJFUFqloJMQ=="
}"#;

    #[test]
    fn the_published_1_0_0_feed_verifies() {
        let release = parse(FEED_1_0_0.as_bytes(), PUBLIC_KEY).unwrap();
        assert!(release.signed);
        assert_eq!(release.version, "1.0.0");
    }

    #[test]
    fn any_change_breaks_the_signature() {
        for (from, to) in [("1.0.0\"", "1.0.1\""), ("v1.0.0/Flux", "v1.0.0/Fluxx"), ("a1018d", "b1018d")] {
            let tampered = FEED_1_0_0.replacen(from, to, 1);
            assert!(parse(tampered.as_bytes(), PUBLIC_KEY).unwrap_err().contains("not valid"), "{from}");
        }
    }

    #[test]
    fn a_feed_signed_with_the_second_key_is_unsigned_for_older_copies() {
        // signature2 holds a signature that is not valid for PUBLIC_KEY_2: refused by 2.1+...
        let feed = r#"{"version":"2.1.0","url":"https://example.test/s.exe","sha256":"abababababababababababababababababababababababababababababababab","signature2":"RcU3GgN25plSFXQ2Ls8k5obeidfZRLjElEBZ5RgifskFarIe4TyMYcxJ2j2+X01MPHVeebDHSVHjJFUFqloJMQ=="}"#;
        assert!(parse_feed(feed.as_bytes()).is_err());
        // ...while copies before 2.1 read only "signature": none, so an unsigned release (a download link).
        assert!(!parse(feed.as_bytes(), PUBLIC_KEY).unwrap().signed);
        // The 1.0.0 feed still verifies through parse_feed with the first key.
        assert!(parse_feed(FEED_1_0_0.as_bytes()).unwrap().signed);
    }

    #[test]
    fn unsigned_and_unsafe_feeds() {
        let unsigned = r#"{"version":"2.0.0","url":"https://example.test/setup.exe","sha256":""}"#;
        assert!(!parse(unsigned.as_bytes(), PUBLIC_KEY).unwrap().signed);
        let http = r#"{"version":"2.0.0","url":"http://example.test/setup.exe"}"#;
        assert!(parse(http.as_bytes(), PUBLIC_KEY).is_err());
        let bad = r#"{"version":"two","url":"https://example.test/setup.exe"}"#;
        assert!(parse(bad.as_bytes(), PUBLIC_KEY).is_err());
    }

    #[test]
    fn a_key_made_here_signs_what_dotnet_verifies() {
        use p256::ecdsa::{SigningKey, signature::Signer};
        use p256::pkcs8::EncodePublicKey;
        let key = SigningKey::random(&mut rand_core::OsRng);
        let public = BASE64.encode(key.verifying_key().to_public_key_der().unwrap().as_bytes());
        let sha = "ab".repeat(32);
        let signature: Signature = key.sign(&payload("2.0.0", "https://example.test/s.exe", &sha));
        let feed = serde_json::json!({ "version": "2.0.0", "url": "https://example.test/s.exe", "sha256": sha, "signature": BASE64.encode(signature.to_bytes()) });
        assert!(parse(feed.to_string().as_bytes(), &public).unwrap().signed);
        assert!(parse(feed.to_string().as_bytes(), PUBLIC_KEY).is_err(), "another key's signature is refused");
    }

    #[test]
    fn versions_compare_like_dotnet() {
        assert!(is_newer("2.0.0", "1.0.0"));
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(!is_newer("2.0.0", "2.0.0-alpha.1"));
        assert!(is_newer("2.0.1", "2.0.0-beta.2"));
        assert!(!is_newer("1.9", "2.0.0"));
        assert_eq!(parse_version("2.0.0.7"), Some([2, 0, 0, 7]));
        assert_eq!(parse_version("2"), None);
    }
}
