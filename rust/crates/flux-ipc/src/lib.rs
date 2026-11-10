//! The browser bridge: Chrome/Edge/Brave → native messaging → `flux-nmhost` → named pipe → Flux.
//!
//! Messages are the same JSON as Flux 1.x (`SwiftFetch.Bridge/Protocol.cs`), so the published extension
//! works unchanged. Frames are a 4-byte little-endian length followed by UTF-8 JSON, both for native
//! messaging and for the pipe.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use url::Url;

/// Native messaging host name registered for the browsers (kept from 1.x so the extension needs no change).
pub const HOST_NAME: &str = "com.swiftfetch.bridge";
/// Largest message accepted in either direction.
pub const MAX_MESSAGE: usize = 1 << 20;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub action: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub session: Option<flux_engine::Session>,
    /// A video quality chosen in the browser (M4).
    #[serde(default)]
    pub video: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<CapturePolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<serde_json::Value>,
    /// The download list (`list`, for the command line).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<serde_json::Value>,
}

impl Response {
    pub fn ok() -> Response {
        Response { ok: true, ..Default::default() }
    }
    pub fn error(message: impl Into<String>) -> Response {
        Response { ok: false, error: Some(message.into()), ..Default::default() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePolicy {
    pub enabled: bool,
    pub context_menu: bool,
    pub extensions: String,
    pub excluded_sites: String,
    pub excluded_addresses: String,
    pub bypass_key: String,
    pub force_key: String,
    pub media_panel: bool,
}

fn is_guid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5 && groups.iter().zip([8, 4, 4, 4, 12]).all(|(g, n)| g.len() == n && g.chars().all(|c| c.is_ascii_hexdigit()))
}

/// A magnet link with a BitTorrent info hash.
pub fn is_magnet(url: &str) -> bool {
    url.len() <= 8192 && url.starts_with("magnet:?") && url.contains("xt=urn:btih:")
}

/// Rejects anything the bridge must not act on (same rules as 1.x).
pub fn validate(request: &Request) -> Result<(), String> {
    let web = |url: &Option<String>| -> Result<Url, String> {
        let text = url.as_deref().filter(|u| u.len() <= 16384).ok_or("Only HTTP/HTTPS links without embedded credentials are supported.")?;
        let parsed = Url::parse(text).map_err(|_| "Only HTTP/HTTPS links without embedded credentials are supported.")?;
        if !matches!(parsed.scheme(), "http" | "https") || !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("Only HTTP/HTTPS links without embedded credentials are supported.".into());
        }
        Ok(parsed)
    };
    let session = |url: &Url| request.session.as_ref().map_or(Ok(()), |s| s.validate(url).map_err(|_| "Invalid download session.".to_string()));
    match request.action.as_str() {
        "ping" | "show" | "quit" | "pause-all" | "resume-all" | "capture-policy" | "pause" | "resume" | "list" => Ok(()),
        "video-formats" => session(&web(&request.url)?),
        "download" => {
            if !request.request_id.as_deref().is_some_and(is_guid) {
                return Err("A valid request ID is required.".into());
            }
            if request.url.as_deref().is_some_and(is_magnet) {
                return if request.video.is_none() && request.session.is_none() { Ok(()) } else { Err("Magnet links carry no video selection or session.".into()) };
            }
            let url = web(&request.url)?;
            if request.file_name.as_ref().is_some_and(|n| n.len() > 1024) {
                return Err("The file name is too long.".into());
            }
            session(&url)
        }
        _ => Err("Unsupported bridge action.".into()),
    }
}

pub async fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Option<T>> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid message size"));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map(Some).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub async fn write_frame<T: Serialize>(writer: &mut (impl AsyncWrite + Unpin), value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if body.len() > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    writer.write_all(&(body.len() as u32).to_le_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await
}

/// Starts Flux (in the tray) for a caller that is about to exit: it must not keep the caller's
/// console or pipes (the browser's, a script's) open, or they would wait for Flux to close.
pub fn start_app(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        // Windows hands every inheritable handle to the child, whatever its own stdio is.
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(id);
            if !handle.is_null() {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
    std::process::Command::new(path)
        .arg("--background")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(windows)]
pub mod pipe;
#[cfg(unix)]
#[path = "pipe_unix.rs"]
pub mod pipe;

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action: &str, url: &str) -> Request {
        Request { action: action.into(), url: Some(url.into()), request_id: Some("6f9619ff-8b86-d011-b42d-00c04fc964ff".into()), ..Default::default() }
    }

    #[test]
    fn validation_matches_flux_1() {
        assert!(validate(&request("download", "https://files.test/a.zip")).is_ok());
        assert!(validate(&request("download", "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567")).is_ok());
        assert!(validate(&request("download", "file:///C:/Windows/win.ini")).is_err());
        assert!(validate(&request("download", "https://u:p@files.test/a.zip")).is_err());
        assert!(validate(&Request { request_id: Some("nope".into()), ..request("download", "https://files.test/a") }).is_err());
        assert!(validate(&request("video-formats", "https://example.test/page")).is_ok());
        assert!(validate(&request("format-c", "https://x.test")).is_err());
        let foreign = Request { session: Some(flux_engine::Session { origin: "https://evil.test/".into(), ..Default::default() }), ..request("download", "https://files.test/a.zip") };
        assert_eq!(validate(&foreign).unwrap_err(), "Invalid download session.");
    }

    #[tokio::test]
    async fn frames_round_trip_and_bad_sizes_are_refused() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, &request("ping", "https://x.test")).await.unwrap();
        let back: Request = read_frame(&mut buffer.as_slice()).await.unwrap().unwrap();
        assert_eq!(back.action, "ping");
        let huge = (MAX_MESSAGE as u32 + 1).to_le_bytes();
        assert!(read_frame::<Request>(&mut huge.as_slice()).await.is_err());
        assert!(read_frame::<Request>(&mut [].as_slice()).await.unwrap().is_none());
    }
}
