//! Flux's native messaging host. The browser starts it for each message from Flux's extension:
//! it checks the caller, answers what it can itself (connection test, capture rules) and hands the
//! rest to the running Flux over the per-user pipe, starting Flux in the tray when needed.
//!
//! `flux-nmhost --register [extension-id …]` writes the host manifest and registers it for Chrome,
//! Edge and Brave (current user); `--unregister` removes that. The installer does the same.

#![cfg_attr(not(debug_assertions), windows_subsystem = "console")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use flux_ipc::{CapturePolicy, HOST_NAME, Request, Response, read_frame, write_frame};

const BROWSERS: &[&str] = &[r"Software\Google\Chrome", r"Software\Microsoft\Edge", r"Software\BraveSoftware\Brave-Browser"];

fn here() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default()
}
fn manifest_path() -> PathBuf {
    here().join(format!("{HOST_NAME}.json"))
}
fn app_path() -> PathBuf {
    let dir = here();
    ["Flux.exe", "flux.exe"].iter().map(|n| dir.join(n)).find(|p| p.exists()).unwrap_or_else(|| dir.join("flux.exe"))
}

/// The extension ID Chrome derives from a manifest's public key: SHA-256 of the key, first 16 bytes, hex a–p.
fn extension_id(key_base64: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let key = base64(key_base64)?;
    Some(Sha256::digest(&key).iter().take(16).flat_map(|b| [b >> 4, b & 15]).map(|n| (b'a' + n) as char).collect())
}
fn base64(text: &str) -> Option<Vec<u8>> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut out, mut buffer, mut bits) = (Vec::new(), 0u32, 0);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        buffer = buffer << 6 | A.iter().position(|a| *a == c)? as u32;
        bits += 6;
        if bits >= 8 { bits -= 8; out.push((buffer >> bits) as u8); }
    }
    Some(out)
}

fn allowed_origins() -> Vec<String> {
    std::fs::read_to_string(manifest_path()).ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|m| m["allowed_origins"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

fn register(ids: &[String]) -> Result<(), String> {
    let mut ids = ids.to_vec();
    if ids.is_empty() {
        // The extension shipped beside Flux has a fixed key, hence a fixed ID.
        for folder in [here().join("BrowserExtension"), here().join("../../../BrowserExtension"), here().join("../../../../BrowserExtension")] {
            if let Some(key) = std::fs::read_to_string(folder.join("manifest.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|m| m["key"].as_str().map(str::to_string)) {
                ids.push(extension_id(&key).ok_or("The extension's key is not valid.")?);
                break;
            }
        }
    }
    if ids.is_empty() {
        return Err("Pass the extension ID: flux-nmhost --register <id>".into());
    }
    let manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": "Flux browser bridge",
        "path": std::env::current_exe().map_err(|e| e.to_string())?.display().to_string(),
        "type": "stdio",
        "allowed_origins": ids.iter().map(|id| format!("chrome-extension://{id}/")).collect::<Vec<_>>(),
    });
    std::fs::write(manifest_path(), serde_json::to_string_pretty(&manifest).unwrap()).map_err(|e| e.to_string())?;
    for browser in BROWSERS {
        let key = format!(r"HKCU\{browser}\NativeMessagingHosts\{HOST_NAME}");
        let status = std::process::Command::new("reg").args(["add", &key, "/ve", "/t", "REG_SZ", "/d", &manifest_path().display().to_string(), "/f"]).output();
        if !status.is_ok_and(|o| o.status.success()) {
            return Err(format!("Could not register for {browser}."));
        }
    }
    println!("Registered {HOST_NAME} for Chrome, Edge and Brave: {}", ids.join(", "));
    Ok(())
}

fn unregister() {
    for browser in BROWSERS {
        let _ = std::process::Command::new("reg").args(["delete", &format!(r"HKCU\{browser}\NativeMessagingHosts\{HOST_NAME}"), "/f"]).output();
    }
    let _ = std::fs::remove_file(manifest_path());
    println!("Unregistered {HOST_NAME}.");
}

/// Capture rules read straight from Flux's settings, so the browser need not wake Flux to check a link.
fn capture_policy() -> Response {
    let settings = flux_store::Store::open(&flux_store::default_directory().join("history.db")).ok().and_then(|s| s.get::<serde_json::Value>("settings").ok().flatten()).unwrap_or_default();
    let text = |key: &str, default: &str| settings[key].as_str().unwrap_or(default).to_string();
    let flag = |key: &str| settings[key].as_bool().unwrap_or(true);
    Response {
        ok: true,
        policy: Some(CapturePolicy {
            enabled: flag("captureEnabled"),
            context_menu: true,
            extensions: text("captureExtensions", ""),
            excluded_sites: text("excludedSites", ""),
            excluded_addresses: String::new(),
            bypass_key: "Alt".into(),
            force_key: "Shift".into(),
            media_panel: flag("videoPanel"),
        }),
        ..Default::default()
    }
}

/// Sends the request to the running Flux, starting it (in the tray) when it is not running.
async fn forward(request: &Request) -> Response {
    let mut started = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match flux_ipc::pipe::connect(Duration::from_millis(500)).await {
            Ok(mut pipe) => {
                if let Err(e) = write_frame(&mut pipe, request).await {
                    return Response::error(format!("Flux did not take the request: {e}"));
                }
                return match tokio::time::timeout(Duration::from_secs(150), read_frame::<Response>(&mut pipe)).await {
                    Ok(Ok(Some(response))) => response,
                    Ok(_) => Response::error("Flux closed the connection."),
                    Err(_) => Response::error("Flux did not answer in time."),
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Response::error(e.to_string()),
            Err(_) => {
                if !started {
                    started = true;
                    if std::process::Command::new(app_path()).arg("--background").spawn().is_err() {
                        return Response::error("Flux is not installed beside the browser bridge.");
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    return Response::error("Flux did not start in time.");
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--register") => { if let Err(e) = register(&args[1..]) { eprintln!("{e}"); std::process::exit(1) } return; }
        Some("--unregister") => { unregister(); return; }
        _ => {}
    }
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    // Chrome passes the calling extension's origin first; only Flux's own extension is served.
    let origin = args.first().cloned().unwrap_or_default();
    let response = if !allowed_origins().contains(&origin) {
        Response::error("This extension is not registered for Flux.")
    } else {
        match tokio::time::timeout(Duration::from_secs(10), read_frame::<Request>(&mut stdin)).await {
            Ok(Ok(Some(request))) => match flux_ipc::validate(&request) {
                Err(error) => Response::error(error),
                Ok(()) => match request.action.as_str() {
                    "ping" => if app_path().exists() { Response::ok() } else { Response::error("The Flux application is missing.") },
                    "capture-policy" => capture_policy(),
                    _ => forward(&request).await,
                },
            },
            _ => return,
        }
    };
    let _ = write_frame(&mut stdout, &response).await;
}

#[cfg(test)]
mod tests {
    #[test]
    fn extension_id_matches_the_one_chrome_gives_flux() {
        // The ID Chrome shows for Flux's extension (registered by Flux 1.x) comes from its manifest key.
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../../../../BrowserExtension/manifest.json")).unwrap();
        assert_eq!(super::extension_id(manifest["key"].as_str().unwrap()).unwrap(), "ljhopjjfadnlpedjofcnnoophngnadml");
    }
}
