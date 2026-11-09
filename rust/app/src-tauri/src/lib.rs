//! Flux 2.0's desktop shell: the main window's commands on top of flux-engine. Queues, settings,
//! video, torrents and the browser bridge arrive in later milestones (see the 2.0 plan).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use flux_engine::{Engine, Event, HostLimits, Job, Options, Outcome};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadView {
    id: String,
    name: String,
    url: String,
    path: String,
    category: String,
    description: String,
    status: String,
    written: u64,
    total: Option<u64>,
    bytes_per_second: f64,
    connections: usize,
    added_ms: u64,
    last_try_ms: Option<u64>,
    error: Option<String>,
}

struct Entry {
    job: Job,
    view: DownloadView,
    cancel: Option<CancellationToken>,
}

struct AppState {
    engine: Engine,
    downloads: Mutex<HashMap<String, Arc<Mutex<Entry>>>>,
    order: Mutex<Vec<String>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn emit(app: &AppHandle, view: &DownloadView) {
    let _ = app.emit("download", view.clone());
}

/// The category a file belongs to, from its extension (same groups as Flux 1.x).
fn category_for(name: &str) -> &'static str {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "iso" | "cab" | "arj" | "lzh" => "Compressed",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "odt" | "ods" | "epub" | "csv" => "Documents",
        "mp3" | "flac" | "wav" | "m4a" | "aac" | "ogg" | "opus" | "wma" => "Music",
        "exe" | "msi" | "msix" | "appx" | "dmg" | "pkg" | "deb" | "rpm" | "apk" | "bat" | "appimage" => "Programs",
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "m4v" | "ts" | "3gp" => "Video",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "tif" | "tiff" | "heic" | "ico" => "Images",
        _ => "Other",
    }
}

fn unique_destination(folder: &Path, name: &str) -> PathBuf {
    let path = folder.join(name);
    if !path.exists() {
        return path;
    }
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..).map(|n| folder.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap()
}

fn entry(state: &AppState, id: &str) -> Option<Arc<Mutex<Entry>>> {
    state.downloads.lock().unwrap().get(id).cloned()
}

#[tauri::command]
fn list_downloads(state: State<'_, Arc<AppState>>) -> Vec<DownloadView> {
    let downloads = state.downloads.lock().unwrap();
    state.order.lock().unwrap().iter().filter_map(|id| downloads.get(id)).map(|e| e.lock().unwrap().view.clone()).collect()
}

#[tauri::command]
fn add_download(app: AppHandle, state: State<'_, Arc<AppState>>, url: String, start_now: Option<bool>) -> Result<String, String> {
    let url = Url::parse(url.trim()).map_err(|_| "Enter a full address starting with http:// or https://.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
        return Err("Only HTTP and HTTPS links without a user name in them are supported.".into());
    }
    let raw = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    let name = flux_engine::sanitize_file_name(&url_decode(raw));
    let name = if name.contains('.') { name } else { "download.bin".to_string() };
    let category = category_for(&name);
    let base = app.path().download_dir().map_err(|e| e.to_string())?;
    let folder = if category == "Other" { base } else { base.join(category) };
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let destination = unique_destination(&folder, &name);
    let id = format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
    let view = DownloadView {
        id: id.clone(),
        name: destination.file_name().unwrap().to_string_lossy().into(),
        url: url.to_string(),
        path: destination.to_string_lossy().into(),
        category: category.into(),
        description: url.host_str().unwrap_or_default().trim_start_matches("www.").to_string(),
        status: "Queued".into(),
        written: 0,
        total: None,
        bytes_per_second: 0.0,
        connections: 0,
        added_ms: now_ms(),
        last_try_ms: None,
        error: None,
    };
    let entry = Arc::new(Mutex::new(Entry { job: Job::new(url, destination), view: view.clone(), cancel: None }));
    state.downloads.lock().unwrap().insert(id.clone(), entry.clone());
    state.order.lock().unwrap().push(id.clone());
    emit(&app, &view);
    if start_now.unwrap_or(true) {
        start(app, state.inner().clone(), entry);
    } else {
        let mut e = entry.lock().unwrap();
        e.view.status = "Paused".into();
        emit(&app, &e.view);
    }
    Ok(id)
}

#[tauri::command]
fn pause_downloads(state: State<'_, Arc<AppState>>, ids: Vec<String>) {
    for id in ids {
        if let Some(entry) = entry(&state, &id) {
            if let Some(cancel) = entry.lock().unwrap().cancel.take() {
                cancel.cancel();
            }
        }
    }
}

#[tauri::command]
fn resume_downloads(app: AppHandle, state: State<'_, Arc<AppState>>, ids: Vec<String>) {
    for id in ids {
        if let Some(entry) = entry(&state, &id) {
            let idle = { let e = entry.lock().unwrap(); e.cancel.is_none() && e.view.status != "Completed" };
            if idle {
                start(app.clone(), state.inner().clone(), entry);
            }
        }
    }
}

#[tauri::command]
fn pause_all(state: State<'_, Arc<AppState>>) {
    for entry in state.downloads.lock().unwrap().values() {
        if let Some(cancel) = entry.lock().unwrap().cancel.take() {
            cancel.cancel();
        }
    }
}

/// Removes downloads from the list. Running ones are stopped first; `delete_files` also deletes the
/// finished file or the partial file from disk.
#[tauri::command]
fn remove_downloads(app: AppHandle, state: State<'_, Arc<AppState>>, ids: Vec<String>, delete_files: bool) -> Vec<String> {
    let mut removed = Vec::new();
    for id in ids {
        let Some(entry) = state.downloads.lock().unwrap().remove(&id) else { continue };
        let e = entry.lock().unwrap();
        if let Some(cancel) = &e.cancel {
            cancel.cancel();
        }
        let part = flux_engine::file::part_path(&e.job.destination);
        if delete_files {
            let _ = std::fs::remove_file(&e.job.destination);
        }
        if e.view.status != "Completed" || delete_files {
            let _ = std::fs::remove_file(part);
        }
        removed.push(id.clone());
    }
    state.order.lock().unwrap().retain(|id| !removed.contains(id));
    let _ = app.emit("removed", removed.clone());
    removed
}

#[tauri::command]
fn remove_completed(app: AppHandle, state: State<'_, Arc<AppState>>) -> Vec<String> {
    let ids: Vec<String> = state.downloads.lock().unwrap().iter().filter(|(_, e)| e.lock().unwrap().view.status == "Completed").map(|(id, _)| id.clone()).collect();
    remove_downloads(app, state, ids, false)
}

#[tauri::command]
fn open_file(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    let path = entry(&state, &id).map(|e| e.lock().unwrap().job.destination.clone()).ok_or("Unknown download.")?;
    if !path.exists() {
        return Err("The file is no longer there.".into());
    }
    #[cfg(windows)]
    std::process::Command::new("cmd").args(["/C", "start", ""]).arg(&path).spawn().map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" }).arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn show_in_folder(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    let path = entry(&state, &id).map(|e| e.lock().unwrap().job.destination.clone()).ok_or("Unknown download.")?;
    #[cfg(windows)]
    {
        let target = if path.exists() { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
        std::process::Command::new("explorer").arg(format!("/select,{}", target.display())).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" }).arg(path.parent().unwrap_or(&path)).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn start(app: AppHandle, state: Arc<AppState>, entry: Arc<Mutex<Entry>>) {
    let cancel = CancellationToken::new();
    let mut job = {
        let mut e = entry.lock().unwrap();
        e.cancel = Some(cancel.clone());
        e.view.status = "Connecting".into();
        e.view.error = None;
        e.view.last_try_ms = Some(now_ms());
        emit(&app, &e.view);
        e.job.clone()
    };
    tauri::async_runtime::spawn(async move {
        let (events_app, events_entry) = (app.clone(), entry.clone());
        let result = state
            .engine
            .download(&mut job, move |event| {
                let mut e = events_entry.lock().unwrap();
                match event {
                    Event::Probed(p) => {
                        e.view.total = p.total;
                        e.view.status = "Downloading".into();
                    }
                    Event::Progress { written, total, bytes_per_second, connections } => {
                        e.view.written = written;
                        e.view.total = total;
                        e.view.bytes_per_second = bytes_per_second;
                        e.view.connections = connections;
                    }
                    Event::Checkpoint(resume) => e.job.resume = resume,
                    Event::Retrying { delay, error, .. } => {
                        e.view.status = format!("Retrying in {}s", delay.as_secs().max(1));
                        e.view.error = Some(error.message);
                    }
                    Event::Note(_) => {}
                }
                emit(&events_app, &e.view);
            }, cancel)
            .await;
        let mut e = entry.lock().unwrap();
        e.job.resume = job.resume.clone();
        e.job.session = job.session.clone();
        e.cancel = None;
        e.view.bytes_per_second = 0.0;
        e.view.connections = 0;
        match result {
            Ok(Outcome::Completed { bytes, .. }) => {
                e.view.status = "Completed".into();
                e.view.written = bytes;
                e.view.total = Some(bytes);
                e.view.error = None;
            }
            Ok(Outcome::Paused) => e.view.status = "Paused".into(),
            Err(error) => {
                e.view.status = "Failed".into();
                e.view.error = Some(error.message);
            }
        }
        emit(&app, &e.view);
    });
}

fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn run() {
    let state = Arc::new(AppState { engine: Engine::new(Options::default(), HostLimits::new([])), downloads: Mutex::new(HashMap::new()), order: Mutex::new(Vec::new()) });
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_downloads, add_download, pause_downloads, resume_downloads, pause_all, remove_downloads, remove_completed, open_file, show_in_folder
        ])
        .run(tauri::generate_context!())
        .expect("Flux could not start");
}
