//! Flux 2.0's desktop app: windows, tray, scheduler and commands on top of the download manager.

mod bridge;
mod crash;
mod desktop;
mod tray_state;
mod extract;
mod i18n;
mod manager;
mod media;
mod migrate;
mod model;
mod system;
mod torrents;
mod updates;
mod zippreview;
mod fileicon;

use std::collections::HashMap;
use std::sync::Mutex;

use manager::{AddRequest, Manager, Probe, Shared, now_ms};
use model::{AfterQueue, Settings, View};
use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Listener, Manager as _, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// A download waiting in the start dialog ("Download File Info") before it is added.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub id: String,
    pub request: PendingRequest,
    pub probe: Option<Probe>,
    /// What the video picker found (kept so the choice is checked against it).
    #[serde(skip)]
    pub video: Option<flux_media::Probe>,
    /// The torrent's file list, once read.
    #[serde(skip)]
    pub torrent: Option<flux_torrent::Info>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub url: String,
    pub file_name: Option<String>,
    pub description: Option<String>,
    #[serde(skip_serializing)]
    pub session: Option<flux_engine::Session>,
    pub request_id: Option<String>,
    /// A quality already chosen (in the browser's video panel or the picker).
    pub video: Option<flux_media::Selection>,
    /// Download the page itself, not its video.
    #[serde(skip)]
    pub as_file: bool,
}

#[derive(Default)]
pub struct Windows {
    pending: Mutex<HashMap<String, Pending>>,
    /// What to do when a download finishes (the progress window's "Options on completion").
    after: Mutex<HashMap<String, AfterQueue>>,
}

type Mgr<'a> = State<'a, Shared>;

// ---------------------------------------------------------------------- windows

fn open_window(app: &AppHandle, label: &str, title: &str, width: f64, height: f64, resizable: bool) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(width, height)
        .min_inner_size(width.min(360.0), height.min(200.0))
        .resizable(resizable)
        // The progress window draws its own title bar (with the "send to background" arrow).
        .decorations(!label.starts_with("progress-"))
        .center()
        .focused(true)
        .build();
    if let Err(error) = built {
        let _ = app.emit("message", format!("A window could not open: {error}"));
    }
}

pub fn open_progress(app: &AppHandle, id: &str) {
    // Its window is back: it is no longer a background download.
    app.state::<tray_state::Background>().remove(id);
    open_window(app, &format!("progress-{id}"), "Download progress", 620.0, 520.0, true);
}
pub fn open_complete(app: &AppHandle, id: &str) {
    open_window(app, &format!("complete-{id}"), "Download complete", 560.0, 300.0, false);
}
fn open_start(app: &AppHandle, pending_id: &str) {
    open_window(app, &format!("start-{pending_id}"), "Download file info", 600.0, 360.0, false);
}
fn open_torrent(app: &AppHandle, pending_id: &str) {
    open_window(app, &format!("torrent-{pending_id}"), "Download torrent", 640.0, 560.0, true);
}
/// A magnet link or a .torrent file on this computer.
fn is_torrent_source(text: &str) -> bool {
    flux_torrent::magnet_hash(text).is_some() || text.to_ascii_lowercase().ends_with(".torrent") && std::path::Path::new(text).is_file()
}
fn open_video(app: &AppHandle, pending_id: &str) {
    open_window(app, &format!("video-{pending_id}"), "Download video", 640.0, 560.0, true);
}
fn pending_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!("{:x}{:x}", now_ms() ^ (std::process::id() as u64) << 20, NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}
fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// A link to add: either straight in, or through the start dialog when that is enabled.
pub fn offer(app: &AppHandle, request: PendingRequest, force_dialog: bool) -> Result<Option<String>, String> {
    let manager = app.state::<Shared>().inner().clone();
    // Torrents open their file list.
    if is_torrent_source(&request.url) {
        if !manager.torrents.available() {
            return Err("The torrent engine is missing from this installation. Reinstall Kestrel to restore it.".into());
        }
        let pending_id = pending_id();
        app.state::<Windows>().pending.lock().unwrap().insert(pending_id.clone(), Pending { id: pending_id.clone(), request, probe: None, video: None, torrent: None });
        open_torrent(app, &pending_id);
        return Ok(None);
    }
    // A video page without a chosen quality opens the quality list first.
    if request.video.is_none() && !request.as_file && manager.media.available() && flux_media::pages::is_known_video_page(&request.url) {
        let pending_id = pending_id();
        app.state::<Windows>().pending.lock().unwrap().insert(pending_id.clone(), Pending { id: pending_id.clone(), request, probe: None, video: None, torrent: None });
        open_video(app, &pending_id);
        return Ok(None);
    }
    let show_dialog = force_dialog || manager.settings.read().unwrap().show_start_dialog;
    if !show_dialog {
        let start = if manager.settings.read().unwrap().only_add_to_queue { "queue" } else { "now" };
        let id = manager.add(AddRequest { url: request.url, file_name: request.file_name, description: request.description, session: request.session, request_id: request.request_id, video: request.video, start: Some(start.into()), ..Default::default() })?;
        if manager.settings.read().unwrap().show_progress_window {
            open_progress(app, &id);
        }
        return Ok(Some(id));
    }
    let pending_id = pending_id();
    if let Some(selection) = &request.video {
        // Nothing to ask the server: the quality list already said what the file will be.
        let probe = video_probe(&manager, &request, selection);
        app.state::<Windows>().pending.lock().unwrap().insert(pending_id.clone(), Pending { id: pending_id.clone(), request, probe: Some(probe), video: None, torrent: None });
        open_start(app, &pending_id);
        return Ok(None);
    }
    app.state::<Windows>().pending.lock().unwrap().insert(pending_id.clone(), Pending { id: pending_id.clone(), request: request.clone(), probe: None, video: None, torrent: None });
    open_start(app, &pending_id);
    // Ask the server for the real name and size while the dialog opens.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let probe = manager.probe(&request.url, request.session.clone(), request.file_name.clone()).await;
        if let Some(pending) = app.state::<Windows>().pending.lock().unwrap().get_mut(&pending_id) {
            pending.probe = Some(probe.clone());
        }
        let _ = app.emit("probe", (pending_id, probe));
    });
    Ok(None)
}

/// The start dialog's file information for a chosen video quality.
fn video_probe(manager: &Manager, request: &PendingRequest, selection: &flux_media::Selection) -> Probe {
    let category = if selection.audio_only { "Music" } else { "Video" };
    let stem = request.file_name.as_deref().map(|n| std::path::Path::new(n).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Video".into());
    Probe {
        url: request.url.clone(),
        file_name: flux_engine::sanitize_file_name(&format!("{stem}.{}", selection.extension)),
        total: selection.total_bytes,
        resumable: true,
        content_type: None,
        category: category.into(),
        folder: manager.folder_for(category).to_string_lossy().into_owned(),
        error: None,
    }
}

// ---------------------------------------------------------------------- commands

#[tauri::command]
fn list_downloads(manager: Mgr) -> Vec<View> {
    manager.views()
}
#[tauri::command]
fn get_download(manager: Mgr, id: String) -> Option<View> {
    manager.view(&id)
}
#[tauri::command]
fn get_settings(manager: Mgr) -> Settings {
    manager.settings()
}
#[tauri::command]
fn save_settings(app: AppHandle, manager: Mgr, settings: Settings) -> Result<(), String> {
    system::set_autostart(settings.start_with_windows);
    manager.save_settings(settings)?;
    let _ = app.emit("message", "Options saved.");
    Ok(())
}
#[tauri::command]
async fn offer_url(app: AppHandle, url: String, file_name: Option<String>, login: Option<String>, password: Option<String>) -> Result<Option<String>, String> {
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://") || flux_torrent::magnet_hash(&url).is_some()) {
        return Err("Enter a full address starting with http:// or https://, or a magnet link.".into());
    }
    // "Use authorization": the login goes with this download only, as HTTP Basic authentication.
    let session = match login.filter(|l| !l.is_empty()) {
        Some(login) => {
            use base64::Engine;
            let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
            let mut session = flux_engine::Session::for_url(&parsed);
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{login}:{}", password.unwrap_or_default()));
            session.authorization = Some(format!("Basic {token}"));
            Some(session)
        }
        None => None,
    };
    offer(&app, PendingRequest { url, file_name, session, ..Default::default() }, false)
}
/// The start dialog's "Preview": the files inside a .zip that is still on the server.
#[tauri::command]
async fn zip_preview(windows: State<'_, Windows>, id: String) -> Result<Vec<zippreview::Entry>, String> {
    let request = windows.pending.lock().unwrap().get(&id).map(|p| p.request.clone()).ok_or("This download was already added or cancelled.")?;
    zippreview::list(&request.url, request.session.as_ref()).await
}
/// The file's own icon for the list (a program's logo once it is downloaded), as a data URL.
#[tauri::command]
async fn file_icon(manager: Mgr<'_>, id: String) -> Result<Option<String>, String> {
    let Some(record) = manager.record(&id) else { return Ok(None) };
    let done = record.status == model::Status::Completed && record.destination.exists();
    let path = record.destination.clone();
    tauri::async_runtime::spawn_blocking(move || fileicon::data_url(&path, done)).await.map_err(|e| e.to_string())
}
#[tauri::command]
fn move_in_queue(manager: Mgr, id: String, delta: i32) {
    manager.move_in_queue(&id, delta)
}
#[tauri::command]
fn get_pending(windows: State<'_, Windows>, id: String) -> Option<Pending> {
    windows.pending.lock().unwrap().get(&id).cloned()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartChoice {
    pending_id: String,
    file_name: String,
    folder: String,
    category: String,
    description: String,
    /// "now", "later" or "queue"
    start: String,
    queue: Option<String>,
    /// Remember the folder for this category (1.x's "Remember this path").
    #[serde(default)]
    remember: bool,
}
#[tauri::command]
async fn confirm_pending(app: AppHandle, manager: Mgr<'_>, windows: State<'_, Windows>, choice: StartChoice) -> Result<String, String> {
    let pending = windows.pending.lock().unwrap().get(&choice.pending_id).cloned().ok_or("This download was already added or cancelled.")?;
    if choice.remember && std::path::Path::new(&choice.folder).is_absolute() && manager.folder_for(&choice.category) != std::path::Path::new(&choice.folder) {
        let mut settings = manager.settings();
        settings.category_dirs.insert(choice.category.clone(), choice.folder.clone().into());
        manager.save_settings(settings)?;
    }
    let start = if choice.start == "queue" { "queue".to_string() } else { choice.start.clone() };
    let id = manager.add(AddRequest {
        url: pending.request.url,
        file_name: Some(choice.file_name),
        folder: Some(choice.folder),
        category: Some(choice.category),
        description: Some(choice.description),
        start: Some(start.clone()),
        queue: choice.queue,
        session: pending.request.session,
        request_id: pending.request.request_id,
        total: pending.probe.and_then(|p| p.total),
        video: pending.request.video,
        ..Default::default()
    })?;
    windows.pending.lock().unwrap().remove(&choice.pending_id);
    if start == "now" && manager.settings.read().unwrap().show_progress_window {
        open_progress(&app, &id);
    }
    Ok(id)
}
#[tauri::command]
fn cancel_pending(windows: State<'_, Windows>, id: String) {
    windows.pending.lock().unwrap().remove(&id);
}

/// The video picker's list: one video's qualities or a playlist's entries.
#[tauri::command]
async fn probe_video(manager: Mgr<'_>, windows: State<'_, Windows>, id: String) -> Result<flux_media::Probe, String> {
    let request = windows.pending.lock().unwrap().get(&id).map(|p| p.request.clone()).ok_or("This download was already added or cancelled.")?;
    let session = request.session.clone().filter(|s| url::Url::parse(&request.url).is_ok_and(|u| s.validate(&u).is_ok()));
    let cancel = tokio_util::sync::CancellationToken::new();
    let found = match tokio::time::timeout(std::time::Duration::from_secs(150), manager.media.probe(&request.url, session.as_ref(), &cancel)).await {
        Ok(Ok(found)) => found,
        Ok(Err(error)) => return Err(if error.is_empty() { "Cancelled.".into() } else { error }),
        Err(_) => { cancel.cancel(); return Err("The video site did not answer in time.".into()); }
    };
    if let Some(pending) = windows.pending.lock().unwrap().get_mut(&id) {
        pending.video = Some(found.clone());
    }
    Ok(found)
}
#[tauri::command]
async fn video_exact_cut(manager: Mgr<'_>) -> Result<bool, String> {
    Ok(manager.media.supports_exact_cut().await)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoChoice {
    pending_id: String,
    /// One video: the chosen quality from the list.
    format_id: Option<String>,
    clip_start: Option<f64>,
    clip_end: Option<f64>,
    #[serde(default)]
    clip_exact: bool,
    /// A playlist: the videos to fetch, a height limit (none = best) or audio only.
    #[serde(default)]
    entries: Vec<String>,
    max_height: Option<u32>,
    #[serde(default)]
    audio_only: bool,
    #[serde(default)]
    subfolder: bool,
    /// Subtitle languages ("en,fr"), saved beside the video.
    subtitles: Option<String>,
    /// "now" or "later"
    start: String,
}
fn clock_name(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 { format!("{}.{:02}.{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}.{:02}", s / 60, s % 60) }
}
#[tauri::command]
async fn confirm_video(app: AppHandle, manager: Mgr<'_>, windows: State<'_, Windows>, choice: VideoChoice) -> Result<String, String> {
    let pending = windows.pending.lock().unwrap().get(&choice.pending_id).cloned().ok_or("This download was already added or cancelled.")?;
    let found = pending.video.clone().ok_or("The quality list is not ready yet.")?;
    if let Some(playlist) = found.playlist {
        let mut preset = flux_media::Selection::preset(choice.max_height, choice.audio_only);
        preset.subtitles = choice.subtitles.clone().filter(|_| !choice.audio_only);
        let category = if choice.audio_only { "Music" } else { "Video" };
        let mut folder = manager.folder_for(category);
        if choice.subfolder {
            folder = folder.join(flux_engine::sanitize_file_name(&playlist.title).trim_end_matches('.'));
        }
        let mut added = 0;
        let mut last_error = None;
        for entry in playlist.entries.iter().filter(|e| choice.entries.contains(&e.url)) {
            let request = AddRequest {
                url: entry.url.clone(),
                file_name: Some(format!("{}.{}", entry.title, preset.extension)),
                folder: Some(folder.to_string_lossy().into_owned()),
                category: Some(category.into()),
                description: Some(playlist.title.clone()),
                start: Some(if choice.start == "now" { "queue".into() } else { "later".into() }),
                video: Some(preset.clone()),
                ..Default::default()
            };
            match manager.add(request) {
                Ok(_) => added += 1,
                Err(error) => last_error = Some(format!("{}: {error}", entry.title)),
            }
        }
        windows.pending.lock().unwrap().remove(&choice.pending_id);
        // Many videos at once go through the main queue rather than all starting together.
        if choice.start == "now" && added > 0 {
            manager.run_waiting(model::MAIN_QUEUE);
        }
        let _ = app.emit("message", format!("{}: added {added} of {} videos{}.", playlist.title, choice.entries.len(), if choice.start == "now" { "" } else { " for later" }));
        return match (added, last_error) {
            (0, Some(error)) => Err(error),
            _ => Ok(String::new()),
        };
    }
    let catalog = found.catalog.ok_or("No video qualities were found.")?;
    let format_id = choice.format_id.ok_or("Choose a quality.")?;
    let mut selection = catalog.formats.iter().find(|f| f.format_id == format_id).cloned().ok_or("This quality is no longer available. Reload the quality list.")?;
    selection.clip_start = choice.clip_start.filter(|s| *s > 0.0);
    selection.clip_end = choice.clip_end;
    selection.clip_exact = choice.clip_exact && selection.is_clip();
    selection.subtitles = choice.subtitles.clone().filter(|_| !selection.audio_only && !selection.is_clip());
    selection.validate(&pending.request.url)?;
    let part = if selection.is_clip() { format!(" (part {}-{})", clock_name(selection.clip_start.unwrap_or(0.0)), selection.clip_end.map_or("end".to_string(), clock_name)) } else { String::new() };
    let name = flux_engine::sanitize_file_name(&format!("{}{part} ({}).{}", catalog.title, selection.quality(), selection.extension));
    let mut request = pending.request.clone();
    request.file_name = Some(name);
    request.video = Some(selection);
    if choice.start == "later" {
        windows.pending.lock().unwrap().remove(&choice.pending_id);
        return manager.add(AddRequest { url: request.url, file_name: request.file_name, session: request.session, request_id: request.request_id, video: request.video, start: Some("later".into()), ..Default::default() });
    }
    windows.pending.lock().unwrap().remove(&choice.pending_id);
    Ok(offer(&app, request, false)?.unwrap_or_default())
}
/// Reads a torrent's file list (from peers, for a magnet link; at most 3 minutes).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TorrentOffer {
    info: flux_torrent::Info,
    folder: String,
}
#[tauri::command]
async fn load_torrent(manager: Mgr<'_>, windows: State<'_, Windows>, id: String) -> Result<TorrentOffer, String> {
    let source = windows.pending.lock().unwrap().get(&id).map(|p| p.request.url.clone()).ok_or("This download was already added or cancelled.")?;
    let cancel = tokio_util::sync::CancellationToken::new();
    let info = match tokio::time::timeout(std::time::Duration::from_secs(180), manager.torrents.load(&source, &cancel)).await {
        Ok(result) => result?,
        Err(_) => {
            cancel.cancel();
            return Err("No one shared this torrent's file list within 3 minutes. Try again later, or use a .torrent file.".into());
        }
    };
    if let Some(pending) = windows.pending.lock().unwrap().get_mut(&id) {
        pending.torrent = Some(info.clone());
    }
    let folder = manager.settings.read().unwrap().download_dir.join("Torrents").to_string_lossy().into_owned();
    Ok(TorrentOffer { info, folder })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TorrentChoice {
    pending_id: String,
    /// 1-based indexes of the chosen files.
    files: Vec<usize>,
    folder: String,
    #[serde(default)]
    seed_minutes: u32,
    /// "now" or "later"
    start: String,
}
#[tauri::command]
async fn confirm_torrent(app: AppHandle, manager: Mgr<'_>, windows: State<'_, Windows>, choice: TorrentChoice) -> Result<String, String> {
    let pending = windows.pending.lock().unwrap().get(&choice.pending_id).cloned().ok_or("This download was already added or cancelled.")?;
    let info = pending.torrent.ok_or("The torrent's file list is not ready yet.")?;
    let chosen: Vec<&flux_torrent::TorrentFile> = info.files.iter().filter(|f| choice.files.contains(&f.index)).collect();
    if chosen.is_empty() {
        return Err("Choose at least one file.".into());
    }
    if !std::path::Path::new(&choice.folder).is_absolute() {
        return Err("Choose a folder to save the torrent in.".into());
    }
    let selection = flux_torrent::Selection {
        info_hash: info.info_hash.clone(),
        name: info.name.clone(),
        metadata_file: flux_torrent::metadata_path(&manager.torrents.cache, &info.info_hash),
        // Every file chosen: no list, so aria2 takes the whole torrent.
        files: if chosen.len() == info.files.len() { Vec::new() } else { chosen.iter().map(|f| f.index).collect() },
        selected_bytes: chosen.iter().map(|f| f.length).sum(),
        seed_minutes: choice.seed_minutes.min(1440),
    };
    let id = manager.add(AddRequest { url: flux_torrent::magnet(&info.info_hash, &info.name), folder: Some(choice.folder), start: Some(choice.start.clone()), request_id: pending.request.request_id, torrent: Some(selection), ..Default::default() })?;
    windows.pending.lock().unwrap().remove(&choice.pending_id);
    if choice.start == "now" && manager.settings.read().unwrap().show_progress_window {
        open_progress(&app, &id);
    }
    Ok(id)
}
#[tauri::command]
async fn open_torrent_file(app: AppHandle, path: String) -> Result<(), String> {
    if !is_torrent_source(&path) {
        return Err("Choose a .torrent file.".into());
    }
    offer(&app, PendingRequest { url: path, ..Default::default() }, false).map(|_| ())
}

/// The page is not a video after all: download the address as a file.
#[tauri::command]
async fn video_as_file(app: AppHandle, windows: State<'_, Windows>, id: String) -> Result<(), String> {
    let pending = windows.pending.lock().unwrap().remove(&id).ok_or("This download was already added or cancelled.")?;
    let mut request = pending.request;
    request.as_file = true;
    request.video = None;
    offer(&app, request, true).map(|_| ())
}
/// Tasks > Export: the addresses, one per line, or as an IDM export file (.ef2).
#[tauri::command]
fn write_link_list(path: String, urls: Vec<String>, idm: bool) -> Result<(), String> {
    let text: String = if idm { urls.iter().map(|u| format!("<\r\n{u}\r\n>\r\n")).collect() } else { urls.iter().map(|u| format!("{u}\r\n")).collect() };
    std::fs::write(&path, text).map_err(|e| e.to_string())
}
/// Tasks > Import: the download links found in a text or IDM export file.
#[tauri::command]
fn read_link_list(path: String) -> Result<Vec<String>, String> {
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > 16 * 1024 * 1024 {
        return Err("That file is too large to be a list of links.".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(links_in(&String::from_utf8_lossy(&bytes)))
}
fn links_in(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    text.split_whitespace()
        .filter(|w| { let l = w.to_ascii_lowercase(); l.starts_with("http://") || l.starts_with("https://") || l.starts_with("magnet:?") })
        .filter(|w| seen.insert(w.to_string()))
        .map(str::to_string)
        .collect()
}
#[tauri::command]
fn add_downloads(manager: Mgr, urls: Vec<String>, start: String) -> Vec<Result<String, String>> {
    urls.into_iter().map(|url| manager.add(AddRequest { url, start: Some(start.clone()), ..Default::default() })).collect()
}
#[tauri::command]
fn pause_downloads(manager: Mgr, ids: Vec<String>) {
    manager.pause(&ids)
}
#[tauri::command]
fn resume_downloads(manager: Mgr, ids: Vec<String>) {
    manager.resume(&ids)
}
#[tauri::command]
fn pause_all(manager: Mgr) {
    manager.pause_all()
}
#[tauri::command]
fn resume_all(manager: Mgr) {
    manager.resume_all()
}
#[tauri::command]
fn remove_downloads(manager: Mgr, ids: Vec<String>, delete_files: bool) -> Vec<String> {
    manager.remove(&ids, delete_files)
}
#[tauri::command]
fn remove_completed(manager: Mgr) -> Vec<String> {
    manager.remove_completed()
}
#[tauri::command]
fn rename_download(manager: Mgr, id: String, name: String) -> Result<(), String> {
    manager.rename(&id, &name)
}
#[tauri::command]
fn redownload(manager: Mgr, id: String) -> Result<String, String> {
    manager.redownload(&id)
}
#[tauri::command]
fn set_speed_limit(manager: Mgr, id: String, bytes_per_second: u64) {
    manager.set_speed_limit(&id, bytes_per_second)
}
#[tauri::command]
fn set_queue(manager: Mgr, ids: Vec<String>, queue: String) {
    manager.set_queue(&ids, &queue)
}
#[tauri::command]
fn enqueue(manager: Mgr, ids: Vec<String>) {
    manager.enqueue(&ids)
}
#[tauri::command]
fn start_queue(manager: Mgr, name: String) {
    manager.start_queue(&name)
}
#[tauri::command]
fn run_waiting(manager: Mgr, name: String) {
    manager.run_waiting(&name)
}
#[tauri::command]
fn stop_queue(manager: Mgr, name: String) {
    manager.stop_queue(&name)
}
#[tauri::command]
fn running_queues(manager: Mgr) -> Vec<String> {
    manager.running_queues.lock().unwrap().iter().cloned().collect()
}
#[tauri::command]
fn open_file(manager: Mgr, id: String) -> Result<(), String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    if !record.destination.exists() {
        return Err("The file is no longer there.".into());
    }
    system::open_path(&record.destination)
}
#[tauri::command]
fn show_in_folder(manager: Mgr, id: String) -> Result<(), String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    system::reveal(&record.destination)
}
#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    system::open_path(std::path::Path::new(&path))
}
#[tauri::command]
async fn show_progress(app: AppHandle, id: String) -> Result<(), String> {
    open_progress(&app, &id);
    Ok(())
}
#[tauri::command]
async fn show_window(app: AppHandle, name: String) -> Result<(), String> {
    match name.as_str() {
        "options" => open_window(&app, "options", "Options", 720.0, 600.0, true),
        "scheduler" => open_window(&app, "scheduler", "Scheduler", 700.0, 580.0, true),
        "batch" => open_window(&app, "batch", "Add batch download", 560.0, 420.0, true),
        "grabber" => open_window(&app, "grabber", "Site grabber", 640.0, 540.0, true),
        _ => show_main(&app),
    }
    Ok(())
}
#[tauri::command]
fn power_action(app: AppHandle, action: AfterQueue) -> Result<(), String> {
    if action == AfterQueue::Exit {
        app.state::<Shared>().shutdown();
        app.exit(0);
        return Ok(());
    }
    app.state::<Shared>().shutdown();
    system::power(action)
}
#[tauri::command]
fn exit_app(app: AppHandle) {
    app.state::<Shared>().shutdown();
    app.exit(0);
}
/// Registers this copy's browser bridge for Chrome, Edge and Brave (the installer does it too).
#[tauri::command]
async fn register_browsers() -> Result<String, String> {
    let host = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name(format!("flux-nmhost{}", std::env::consts::EXE_SUFFIX));
    if !host.exists() {
        return Err("flux-nmhost.exe is missing beside Kestrel.".into());
    }
    let mut command = std::process::Command::new(host);
    command.arg("--register");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(if output.status.success() { &output.stdout } else { &output.stderr }).trim().to_string();
    if output.status.success() { Ok(text) } else { Err(text) }
}
/// Options > General > Test browser integration: is each browser connected to this Kestrel?
#[derive(Serialize)]
struct BrowserCheck {
    browser: String,
    ok: bool,
    detail: String,
}
#[tauri::command]
fn test_browsers() -> Vec<BrowserCheck> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let host = std::env::current_exe().map(|e| e.with_file_name("flux-nmhost.exe")).unwrap_or_default();
        [("Google Chrome", r"Google\Chrome"), ("Microsoft Edge", r"Microsoft\Edge"), ("Brave", r"BraveSoftware\Brave-Browser"), ("Mozilla Firefox", "Mozilla")]
            .into_iter()
            .map(|(name, key)| {
                let key = format!(r"HKCU\Software\{key}\NativeMessagingHosts\com.swiftfetch.bridge");
                let manifest = std::process::Command::new("reg").args(["query", &key, "/ve"]).creation_flags(0x0800_0000).output().ok()
                    .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().find(|l| l.contains("REG_SZ")).and_then(|l| l.split("REG_SZ").nth(1)).map(|v| std::path::PathBuf::from(v.trim())));
                let (ok, detail) = match manifest {
                    None => (false, "Not connected: use Connect browsers.".to_string()),
                    Some(m) if !m.is_file() => (false, "Connected to a copy that is gone: use Connect browsers.".to_string()),
                    Some(m) => {
                        let path = std::fs::read_to_string(&m).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["path"].as_str().map(std::path::PathBuf::from));
                        match path {
                            Some(p) if p == host => (true, "Connected to this Kestrel.".to_string()),
                            Some(p) if p.is_file() => (true, format!("Connected to another copy: {}", p.display())),
                            _ => (false, "Its bridge program is missing: use Connect browsers.".to_string()),
                        }
                    }
                };
                BrowserCheck { browser: name.into(), ok, detail }
            })
            .collect()
    }
    #[cfg(not(windows))]
    Vec::new()
}
/// Site grabber: links to files of the given types on one site.
#[tauri::command]
async fn grab_site(manager: Mgr<'_>, url: String, extensions: String) -> Result<Vec<String>, String> {
    let root = url::Url::parse(url.trim()).map_err(|_| "Enter a full address starting with http:// or https://.".to_string())?;
    let session = manager.with_login(&root, None);
    manager.engine.grab(&root, &extensions, session.as_ref()).await
}
#[tauri::command]
async fn check_updates(app: AppHandle) -> Result<updates::UpdateInfo, String> {
    updates::check(&app).await
}
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    updates::install(&app).await
}
/// What the windows need to drag a finished file out of Flux (to Explorer, a mail, a chat).
#[derive(Serialize)]
struct DragItem {
    path: String,
    icon: String,
}
#[tauri::command]
fn drag_item(manager: Mgr, id: String) -> Result<DragItem, String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    if record.status != model::Status::Completed || !record.destination.exists() {
        return Err("Only a finished file that is still there can be dragged.".into());
    }
    Ok(DragItem { path: record.destination.to_string_lossy().into_owned(), icon: desktop::drag_icon()?.to_string_lossy().into_owned() })
}
#[tauri::command]
fn crash_reports() -> Vec<crash::Report> {
    crash::pending()
}
#[tauri::command]
fn dismiss_crash(id: String) -> Result<(), String> {
    crash::dismiss(&id)
}
/// Opens the report as a new GitHub issue in the browser (the user reads it and sends it there).
#[tauri::command]
fn report_crash(id: String) -> Result<(), String> {
    let url = crash::issue_url(&id)?;
    system::open_path(std::path::Path::new(&url))?;
    crash::dismiss(&id)
}
/// The progress window's green arrow: the download goes on in the background, in the tray.
#[tauri::command]
async fn to_background(app: AppHandle, id: String) {
    tray_state::send(&app, &id);
}
#[tauri::command]
fn set_after_complete(windows: State<'_, Windows>, id: String, action: AfterQueue) {
    let mut after = windows.after.lock().unwrap();
    if action == AfterQueue::Nothing { after.remove(&id); } else { after.insert(id, action); }
}
#[tauri::command]
fn get_after_complete(windows: State<'_, Windows>, id: String) -> AfterQueue {
    windows.after.lock().unwrap().get(&id).copied().unwrap_or_default()
}
/// Windows' "Open with" chooser for a finished file.
#[tauri::command]
fn open_with(manager: Mgr, id: String) -> Result<(), String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    if !record.destination.exists() {
        return Err("The file is no longer there.".into());
    }
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32.exe").arg("shell32.dll,OpenAs_RunDLL").arg(&record.destination).spawn().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(windows))]
    system::open_path(&record.destination)
}
#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

// ---------------------------------------------------------------------- background work

/// Starts and stops queues at their scheduled times (checked every 20 seconds).
fn scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut fired: std::collections::HashSet<String> = Default::default();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            let manager = app.state::<Shared>().inner().clone();
            let (day, now) = system::local_time();
            let stamp = format!("{day}-{now}");
            for queue in manager.settings.read().unwrap().queues.clone() {
                if !queue.schedule_enabled || !(queue.days.is_empty() || queue.days.contains(&day)) {
                    continue;
                }
                if queue.start_time.as_deref() == Some(now.as_str()) && fired.insert(format!("{stamp}-start-{}", queue.name)) {
                    manager.start_queue(&queue.name);
                    if let Some(sound) = manager.settings.read().unwrap().sounds.get(2).filter(|s| s.on) { system::play(&sound.file, true); }
                    let _ = app.emit("message", format!("{} started on schedule.", queue.name));
                }
                if queue.stop_time.as_deref() == Some(now.as_str()) && fired.insert(format!("{stamp}-stop-{}", queue.name)) {
                    manager.stop_queue(&queue.name);
                    let _ = app.emit("message", format!("{} stopped on schedule.", queue.name));
                }
            }
            if fired.len() > 500 {
                fired.clear();
            }
        }
    });
}

/// Offers download links copied to the clipboard (when enabled in Options).
fn clipboard_monitor(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last = String::new();
        let mut clipboard = arboard::Clipboard::new().ok();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let manager = app.state::<Shared>().inner().clone();
            let settings = manager.settings.read().unwrap().clone();
            if !settings.monitor_clipboard {
                continue;
            }
            let Some(text) = clipboard.as_mut().and_then(|c| c.get_text().ok()).map(|t| t.trim().to_string()) else { continue };
            if text == last || text.len() > 4096 {
                continue;
            }
            last = text.clone();
            let Ok(url) = url::Url::parse(&text) else { continue };
            let ext = std::path::Path::new(url.path()).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let wanted = !ext.is_empty() && settings.capture_extensions.split_whitespace().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext));
            if matches!(url.scheme(), "http" | "https") && wanted && !manager.views().iter().any(|v| v.url == text) {
                let _ = offer(&app, PendingRequest { url: text, ..Default::default() }, true);
            }
        }
    });
}

/// What the tray menu shows; it is rebuilt only when this changes (a rebuild closes an open menu).
#[derive(Clone, PartialEq, Default)]
struct TrayLook {
    language: String,
    limiter: bool,
    main_running: bool,
    running: Vec<(String, String)>,
}
fn tray_look(manager: &Shared) -> TrayLook {
    let settings = manager.settings.read().unwrap();
    TrayLook {
        language: i18n::resolve(&settings.language).to_string(),
        limiter: settings.speed_limit > 0,
        main_running: manager.running_queues.lock().unwrap().iter().any(|q| q == crate::model::MAIN_QUEUE),
        running: manager.views().into_iter().filter(|v| v.status.running()).map(|v| (v.id, v.name)).collect(),
    }
}
/// A download's name as the menu shows it: short, and "&" doubled (menus read it as a shortcut mark).
fn menu_name(name: &str) -> String {
    let mut s: String = name.chars().take(48).collect();
    if name.chars().count() > 48 {
        s.push('…');
    }
    s.replace('&', "&&")
}

/// The tray menu in the interface language, like MDM's.
fn tray_menu(app: &AppHandle, look: &TrayLook) -> tauri::Result<Menu<tauri::Wry>> {
    let language = look.language.as_str();
    let item = |id: &str, text: &'static str, on: bool| MenuItem::with_id(app, id, i18n::tr(language, text), on, None::<&str>);
    let sep = || PredefinedMenuItem::separator(app);
    let downloads = Submenu::new(app, i18n::tr(language, "Downloads in progress"), !look.running.is_empty())?;
    for (id, name) in &look.running {
        downloads.append(&MenuItem::with_id(app, format!("dl:{id}"), menu_name(name), true, None::<&str>)?)?;
    }
    let limiter = Submenu::with_items(app, i18n::tr(language, "Speed Limiter"), true, &[
        &CheckMenuItem::with_id(app, "limit-on", i18n::tr(language, "Turn on"), true, look.limiter, None::<&str>)?,
        &CheckMenuItem::with_id(app, "limit-off", i18n::tr(language, "Turn off"), true, !look.limiter, None::<&str>)?,
    ])?;
    Menu::with_items(app, &[
        &item("show", "Open Kestrel", true)?,
        &sep()?,
        &item("add", "Add new download", true)?,
        &item("batch", "Add batch download", true)?,
        &downloads,
        &sep()?,
        &limiter,
        &item("scheduler", "Scheduler", true)?,
        &item("queue-start", "Start main download queue", !look.main_running)?,
        &item("queue-stop", "Stop main download queue", look.main_running)?,
        &item("pause", "Pause all", true)?,
        &item("resume", "Resume all", true)?,
        &item("options", "Options", true)?,
        &sep()?,
        &item("quit", "Exit", true)?,
    ])
}

fn set_limiter(app: &AppHandle, on: bool) {
    let manager = app.state::<Shared>();
    let mut settings = manager.settings();
    settings.speed_limit = if on { if settings.last_speed_limit > 0 { settings.last_speed_limit } else { 1024 * 1024 } } else { 0 };
    let _ = manager.save_settings(settings);
}

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let first = tray_look(&app.state::<Shared>());
    let menu = tray_menu(app, &first)?;
    // Kept in step with the app (language, limiter, queue, running downloads), rebuilt only on change.
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut shown = first;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let look = tray_look(&handle.state::<Shared>());
            if look == shown {
                continue;
            }
            if let (Some(tray), Ok(menu)) = (handle.tray_by_id("flux"), tray_menu(&handle, &look)) {
                let _ = tray.set_menu(Some(menu));
            }
            shown = look;
        }
    });
    TrayIconBuilder::with_id("flux")
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip("Flux")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "add" => { show_main(app); let _ = app.emit("open-add", ()); }
            "batch" => open_window(app, "batch", "Add batch download", 560.0, 420.0, true),
            "scheduler" => open_window(app, "scheduler", "Scheduler", 700.0, 580.0, true),
            "options" => open_window(app, "options", "Options", 720.0, 600.0, true),
            "limit-on" => set_limiter(app, true),
            "limit-off" => set_limiter(app, false),
            "queue-start" => app.state::<Shared>().start_queue(crate::model::MAIN_QUEUE),
            "queue-stop" => app.state::<Shared>().stop_queue(crate::model::MAIN_QUEUE),
            "pause" => app.state::<Shared>().pause_all(),
            "resume" => app.state::<Shared>().resume_all(),
            "quit" => { app.state::<Shared>().shutdown(); app.exit(0); }
            other => {
                if let Some(id) = other.strip_prefix("dl:") {
                    open_progress(app, id);
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A second start (or a link passed on the command line) goes to the running Flux.
            show_main(app);
            if let Some(url) = args.iter().skip(1).find(|a| a.starts_with("http://") || a.starts_with("https://") || is_torrent_source(a)).cloned() {
                // Off the main thread: opening a window from it would wait on itself.
                let app = app.clone();
                tauri::async_runtime::spawn(async move { let _ = offer(&app, PendingRequest { url, ..Default::default() }, false); });
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_drag::init())
        .manage(Windows::default())
        .manage(tray_state::Background::default())
        .setup(|app| {
            let handle = app.handle().clone();
            crash::install(app.package_info().version.to_string());
            system::hold_running_mutex();
            // Off the start-up path: a few registry reads, and a registration when it is missing.
            std::thread::spawn(system::ensure_browser_bridge);
            let data_dir = flux_store::default_directory();
            let manager = Manager::open(handle.clone(), &data_dir).map_err(Box::<dyn std::error::Error>::from)?;
            app.manage(manager.clone());
            // Scheduler: "Start download on Kestrel startup".
            for queue in manager.settings().queues.iter().filter(|q| q.start_on_launch) {
                manager.start_queue(&queue.name);
            }
            // Learned per-site connection limits are kept with the settings.
            let saver = manager.clone();
            handle.listen("host-limit", move |_| { let _ = saver.save_settings(saver.settings()); });
            // Completion: sound, and the "Download complete" window when enabled.
            let completer = handle.clone();
            handle.listen("completed", move |event| {
                let id: String = serde_json::from_str(event.payload()).unwrap_or_default();
                let manager = completer.state::<Shared>();
                let settings = manager.settings.read().unwrap().clone();
                if let Some(window) = completer.get_webview_window(&format!("progress-{id}")) { let _ = window.close(); }
                if let Some(sound) = settings.sounds.first().filter(|s| s.on) { system::play(&sound.file, true); }
                if settings.show_complete_dialog { open_complete(&completer, &id); }
                // "Turn off computer / exit when done" chosen in its progress window: the main
                // window counts down and can still cancel it.
                if let Some(action) = completer.state::<Windows>().after.lock().unwrap().remove(&id) {
                    let name = manager.view(&id).map(|v| v.name).unwrap_or_default();
                    show_main(&completer);
                    let _ = completer.emit("queue-finished", (name, action));
                }
                desktop::completed(&completer, &id, settings.show_complete_dialog);
            });
            let failures = handle.clone();
            handle.listen("failed", move |event| {
                if let Some(sound) = failures.state::<Shared>().settings.read().unwrap().sounds.get(1).filter(|s| s.on) { system::play(&sound.file, false); }
                if let Ok((id, error)) = serde_json::from_str::<(String, String)>(event.payload()) {
                    desktop::failed(&failures, &id, &error);
                }
            });
            tray(&handle)?;
            desktop::taskbar_progress(handle.clone());
            tray_state::watch(handle.clone());
            scheduler(handle.clone());
            clipboard_monitor(handle.clone());
            bridge::serve(handle.clone());
            media::keep_tools_current(handle.clone());
            updates::check_daily(handle.clone());
            // The main window starts hidden; Flux started by Windows or the browser stays in the tray.
            if !std::env::args().any(|a| a == "--background")
                && let Some(window) = handle.get_webview_window("main") { let _ = window.show(); let _ = window.set_focus(); }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main" {
                    let manager = window.app_handle().state::<Shared>();
                    if manager.settings.read().unwrap().minimize_to_tray {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        manager.shutdown();
                        window.app_handle().exit(0);
                    }
                }
        })
        .invoke_handler(tauri::generate_handler![
            list_downloads, get_download, get_settings, save_settings, offer_url, get_pending, confirm_pending, cancel_pending,
            probe_video, video_exact_cut, confirm_video, video_as_file, load_torrent, confirm_torrent, open_torrent_file, check_updates, install_update, drag_item, grab_site, run_waiting, register_browsers, test_browsers, set_after_complete, get_after_complete, open_with, to_background, crash_reports, dismiss_crash, report_crash,
            add_downloads, pause_downloads, resume_downloads, pause_all, resume_all, remove_downloads, remove_completed,
            rename_download, redownload, zip_preview, file_icon, move_in_queue, write_link_list, read_link_list, set_speed_limit, set_queue, enqueue, start_queue, stop_queue, running_queues,
            open_file, show_in_folder, open_folder, show_progress, show_window, power_action, exit_app, app_version
        ])
        .run(tauri::generate_context!())
        .expect("Kestrel could not start");
}
