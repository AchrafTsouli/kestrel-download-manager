//! Kestrel's native messaging host. The browser starts it for each message from Kestrel's extension:
//! it checks the caller, answers what it can itself (connection test, capture rules) and hands the
//! rest to the running Kestrel over the per-user pipe, starting Kestrel in the tray when needed.
//!
//! `kestrel-nmhost --register [extension-id …]` writes the host manifests and registers them for
//! Chrome, Edge, Brave and Firefox (current user); `--register-firefox` does Firefox only (the
//! installer writes the Chromium one itself); `--unregister` removes them.

#![cfg_attr(not(debug_assertions), windows_subsystem = "console")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use kestrel_ipc::{CapturePolicy, HOST_NAME, Request, Response, read_frame, write_frame};

/// Kestrel's add-on ID in Firefox (browser_specific_settings.gecko.id of the Firefox build).
const FIREFOX_ID: &str = "kestrel@achraftsouli.github.io";

fn here() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default()
}
/// Where the bridge keeps its manifests: beside it on Windows (the installer writes there), in
/// Kestrel's data folder elsewhere (a system-wide install folder is not the user's to write).
fn manifest_dir() -> PathBuf {
    if cfg!(windows) { here() } else { kestrel_store::default_directory() }
}
fn manifest_path() -> PathBuf {
    manifest_dir().join(format!("{HOST_NAME}.json"))
}
fn firefox_manifest_path() -> PathBuf {
    manifest_dir().join(format!("{HOST_NAME}.firefox.json"))
}
fn app_path() -> PathBuf {
    let dir = here();
    ["Kestrel.exe", "kestrel.exe", "kestrel"].iter().map(|n| dir.join(n)).find(|p| p.exists()).unwrap_or_else(|| dir.join(if cfg!(windows) { "kestrel.exe" } else { "kestrel" }))
}

/// Where a browser looks for native messaging hosts (current user).
enum Place {
    /// Windows: a registry key whose default value is the manifest's path.
    #[cfg_attr(not(windows), allow(dead_code))]
    Registry(String),
    /// Linux, macOS: a folder holding `<host name>.json`.
    #[cfg_attr(windows, allow(dead_code))]
    Folder(PathBuf),
}

#[cfg(windows)]
fn chromium_places() -> Vec<(&'static str, Place)> {
    [("Chrome", r"Software\Google\Chrome"), ("Edge", r"Software\Microsoft\Edge"), ("Brave", r"Software\BraveSoftware\Brave-Browser")]
        .into_iter()
        .map(|(name, key)| (name, Place::Registry(format!(r"HKCU\{key}\NativeMessagingHosts\{HOST_NAME}"))))
        .collect()
}
#[cfg(windows)]
fn firefox_place() -> Place {
    Place::Registry(format!(r"HKCU\Software\Mozilla\NativeMessagingHosts\{HOST_NAME}"))
}

#[cfg(not(windows))]
fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}
#[cfg(target_os = "macos")]
fn chromium_places() -> Vec<(&'static str, Place)> {
    let support = home().join("Library/Application Support");
    [("Chrome", "Google/Chrome"), ("Chromium", "Chromium"), ("Edge", "Microsoft Edge"), ("Brave", "BraveSoftware/Brave-Browser")]
        .into_iter()
        .map(|(name, dir)| (name, Place::Folder(support.join(dir).join("NativeMessagingHosts"))))
        .collect()
}
#[cfg(target_os = "macos")]
fn firefox_place() -> Place {
    Place::Folder(home().join("Library/Application Support/Mozilla/NativeMessagingHosts"))
}
#[cfg(all(unix, not(target_os = "macos")))]
fn chromium_places() -> Vec<(&'static str, Place)> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    [("Chrome", "google-chrome"), ("Chromium", "chromium"), ("Edge", "microsoft-edge"), ("Brave", "BraveSoftware/Brave-Browser")]
        .into_iter()
        .map(|(name, dir)| (name, Place::Folder(config.join(dir).join("NativeMessagingHosts"))))
        .collect()
}
#[cfg(all(unix, not(target_os = "macos")))]
fn firefox_place() -> Place {
    Place::Folder(home().join(".mozilla/native-messaging-hosts"))
}

/// Points a browser at a manifest.
fn install(place: &Place, manifest: &Path) -> bool {
    match place {
        Place::Registry(key) => std::process::Command::new("reg").args(["add", key, "/ve", "/t", "REG_SZ", "/d", &manifest.display().to_string(), "/f"]).output().is_ok_and(|o| o.status.success()),
        Place::Folder(dir) => std::fs::create_dir_all(dir).and_then(|_| std::fs::copy(manifest, dir.join(format!("{HOST_NAME}.json")))).is_ok(),
    }
}
fn uninstall(place: &Place) {
    match place {
        Place::Registry(key) => { let _ = std::process::Command::new("reg").args(["delete", key, "/f"]).output(); }
        Place::Folder(dir) => { let _ = std::fs::remove_file(dir.join(format!("{HOST_NAME}.json"))); }
    }
}
fn write_manifest(path: &Path, manifest: &serde_json::Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_string_pretty(manifest).unwrap()).map_err(|e| e.to_string())
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

fn manifest_list(path: &Path, field: &str) -> Vec<String> {
    std::fs::read_to_string(path).ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|m| m[field].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

/// Chromium browsers pass the caller's origin first; Firefox passes its manifest's path, then the
/// add-on's ID. Only Kestrel's own extension, as registered, is served.
fn caller_allowed(args: &[String]) -> bool {
    match args.first() {
        Some(origin) if origin.starts_with("chrome-extension://") => manifest_list(&manifest_path(), "allowed_origins").contains(origin),
        Some(_) => args.get(1).is_some_and(|id| manifest_list(&firefox_manifest_path(), "allowed_extensions").contains(id)),
        None => false,
    }
}

fn register_firefox() -> Result<(), String> {
    let manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": "Kestrel browser bridge",
        "path": std::env::current_exe().map_err(|e| e.to_string())?.display().to_string(),
        "type": "stdio",
        "allowed_extensions": [FIREFOX_ID],
    });
    write_manifest(&firefox_manifest_path(), &manifest)?;
    if !install(&firefox_place(), &firefox_manifest_path()) {
        return Err("Could not register for Firefox.".into());
    }
    Ok(())
}

fn register(ids: &[String]) -> Result<(), String> {
    let mut ids = ids.to_vec();
    if ids.is_empty() {
        // The extension shipped beside Kestrel has a fixed key, hence a fixed ID.
        for folder in [here().join("BrowserExtension"), here().join("../../../BrowserExtension"), here().join("../../../../BrowserExtension")] {
            if let Some(key) = std::fs::read_to_string(folder.join("manifest.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|m| m["key"].as_str().map(str::to_string)) {
                ids.push(extension_id(&key).ok_or("The extension's key is not valid.")?);
                break;
            }
        }
    }
    if ids.is_empty() {
        return Err("Pass the extension ID: kestrel-nmhost --register <id>".into());
    }
    let manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": "Kestrel browser bridge",
        "path": std::env::current_exe().map_err(|e| e.to_string())?.display().to_string(),
        "type": "stdio",
        "allowed_origins": ids.iter().map(|id| format!("chrome-extension://{id}/")).collect::<Vec<_>>(),
    });
    write_manifest(&manifest_path(), &manifest)?;
    let places = chromium_places();
    for (browser, place) in &places {
        if !install(place, &manifest_path()) {
            return Err(format!("Could not register for {browser}."));
        }
    }
    register_firefox()?;
    let names: Vec<&str> = places.iter().map(|(name, _)| *name).collect();
    println!("Registered {HOST_NAME} for {} ({}) and Firefox ({FIREFOX_ID}).", names.join(", "), ids.join(", "));
    Ok(())
}

fn unregister() {
    for (_, place) in chromium_places() {
        uninstall(&place);
    }
    uninstall(&firefox_place());
    let _ = std::fs::remove_file(manifest_path());
    let _ = std::fs::remove_file(firefox_manifest_path());
    println!("Unregistered {HOST_NAME}.");
}

/// Capture rules read straight from Kestrel's settings, so the browser need not wake Kestrel to check a link.
fn capture_policy() -> Response {
    let settings = kestrel_store::Store::open(&kestrel_store::default_directory().join("history.db")).ok().and_then(|s| s.get::<serde_json::Value>("settings").ok().flatten()).unwrap_or_default();
    let text = |key: &str, default: &str| settings[key].as_str().unwrap_or(default).to_string();
    let flag = |key: &str| settings[key].as_bool().unwrap_or(true);
    Response {
        ok: true,
        policy: Some(CapturePolicy {
            enabled: flag("captureEnabled"),
            context_menu: true,
            extensions: text("captureExtensions", ""),
            excluded_sites: text("excludedSites", ""),
            excluded_addresses: text("excludedAddresses", ""),
            bypass_key: text("bypassKey", "Alt"),
            force_key: text("forceKey", "Shift"),
            media_panel: flag("videoPanel"),
        }),
        ..Default::default()
    }
}

/// Sends the request to the running Kestrel, starting it (in the tray) when it is not running.
async fn forward(request: &Request) -> Response {
    let mut started = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match kestrel_ipc::pipe::connect(Duration::from_millis(500)).await {
            Ok(mut pipe) => {
                if let Err(e) = write_frame(&mut pipe, request).await {
                    return Response::error(format!("Kestrel did not take the request: {e}"));
                }
                return match tokio::time::timeout(Duration::from_secs(150), read_frame::<Response>(&mut pipe)).await {
                    Ok(Ok(Some(response))) => response,
                    Ok(_) => Response::error("Kestrel closed the connection."),
                    Err(_) => Response::error("Kestrel did not answer in time."),
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Response::error(e.to_string()),
            Err(_) => {
                if !started {
                    started = true;
                    if kestrel_ipc::start_app(&app_path()).is_err() {
                        return Response::error("Kestrel is not installed beside the browser bridge.");
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    return Response::error("Kestrel did not start in time.");
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
        Some("--register-firefox") => { if let Err(e) = register_firefox() { eprintln!("{e}"); std::process::exit(1) } return; }
        _ => {}
    }
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let response = if !caller_allowed(&args) {
        Response::error("This extension is not registered for Kestrel.")
    } else {
        match tokio::time::timeout(Duration::from_secs(10), read_frame::<Request>(&mut stdin)).await {
            Ok(Ok(Some(request))) => match kestrel_ipc::validate(&request) {
                Err(error) => Response::error(error),
                Ok(()) => match request.action.as_str() {
                    "ping" => if app_path().exists() { Response::ok() } else { Response::error("The Kestrel application is missing.") },
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
    fn extension_id_matches_the_one_chrome_gives_kestrel() {
        // The ID Chrome shows for Kestrel's extension (registered by Flux 1.x) comes from its manifest key.
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../../../../BrowserExtension/manifest.json")).unwrap();
        assert_eq!(super::extension_id(manifest["key"].as_str().unwrap()).unwrap(), "ljhopjjfadnlpedjofcnnoophngnadml");
    }
}
