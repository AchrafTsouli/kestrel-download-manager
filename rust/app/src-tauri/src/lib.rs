//! Flux 2.0's desktop shell. M0: add a link, download it with flux-engine, show progress, pause and
//! resume. The full window set (queues, settings, video, torrents, browser bridge) arrives in M2–M5.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
    status: String,
    written: u64,
    total: Option<u64>,
    bytes_per_second: f64,
    connections: usize,
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

fn emit(app: &AppHandle, view: &DownloadView) {
    let _ = app.emit("download", view.clone());
}

fn unique_destination(folder: &PathBuf, name: &str) -> PathBuf {
    let path = folder.join(name);
    if !path.exists() {
        return path;
    }
    let stem = std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = std::path::Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..).map(|n| folder.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap()
}

#[tauri::command]
fn list_downloads(state: State<'_, Arc<AppState>>) -> Vec<DownloadView> {
    let downloads = state.downloads.lock().unwrap();
    state.order.lock().unwrap().iter().filter_map(|id| downloads.get(id)).map(|e| e.lock().unwrap().view.clone()).collect()
}

#[tauri::command]
fn add_download(app: AppHandle, state: State<'_, Arc<AppState>>, url: String) -> Result<String, String> {
    let url = Url::parse(url.trim()).map_err(|_| "Enter a full address starting with http:// or https://.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
        return Err("Only HTTP and HTTPS links without a user name in them are supported.".into());
    }
    let folder = app.path().download_dir().map_err(|e| e.to_string())?;
    let raw = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    let name = flux_engine::sanitize_file_name(&url_decode(raw));
    let destination = unique_destination(&folder, if name.contains('.') { &name } else { "download.bin" });
    let id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let view = DownloadView {
        id: id.clone(), name: destination.file_name().unwrap().to_string_lossy().into(), url: url.to_string(), path: destination.to_string_lossy().into(),
        status: "Queued".into(), written: 0, total: None, bytes_per_second: 0.0, connections: 0, error: None,
    };
    let entry = Arc::new(Mutex::new(Entry { job: Job::new(url, destination), view: view.clone(), cancel: None }));
    state.downloads.lock().unwrap().insert(id.clone(), entry.clone());
    state.order.lock().unwrap().push(id.clone());
    emit(&app, &view);
    start(app, state.inner().clone(), entry);
    Ok(id)
}

#[tauri::command]
fn pause_download(state: State<'_, Arc<AppState>>, id: String) {
    if let Some(entry) = state.downloads.lock().unwrap().get(&id)
        && let Some(cancel) = entry.lock().unwrap().cancel.take() {
            cancel.cancel();
        }
}

#[tauri::command]
fn resume_download(app: AppHandle, state: State<'_, Arc<AppState>>, id: String) {
    let entry = state.downloads.lock().unwrap().get(&id).cloned();
    if let Some(entry) = entry
        && entry.lock().unwrap().cancel.is_none() && entry.lock().unwrap().view.status != "Completed" {
            start(app, state.inner().clone(), entry);
        }
}

fn start(app: AppHandle, state: Arc<AppState>, entry: Arc<Mutex<Entry>>) {
    let cancel = CancellationToken::new();
    let mut job = {
        let mut e = entry.lock().unwrap();
        e.cancel = Some(cancel.clone());
        e.view.status = "Connecting".into();
        e.view.error = None;
        emit(&app, &e.view);
        e.job.clone()
    };
    tauri::async_runtime::spawn(async move {
        let (events_app, events_entry) = (app.clone(), entry.clone());
        let result = state.engine.download(&mut job, move |event| {
            let mut e = events_entry.lock().unwrap();
            match event {
                Event::Probed(p) => { e.view.total = p.total; e.view.status = "Downloading".into(); }
                Event::Progress { written, total, bytes_per_second, connections } => {
                    e.view.written = written; e.view.total = total; e.view.bytes_per_second = bytes_per_second; e.view.connections = connections;
                }
                Event::Checkpoint(resume) => e.job.resume = resume,
                Event::Retrying { delay, error, .. } => { e.view.status = format!("Retrying in {}s", delay.as_secs().max(1)); e.view.error = Some(error.message); }
                Event::Note(_) => {}
            }
            emit(&events_app, &e.view);
        }, cancel).await;
        let mut e = entry.lock().unwrap();
        e.job.resume = job.resume.clone();
        e.job.session = job.session.clone();
        e.cancel = None;
        e.view.bytes_per_second = 0.0;
        e.view.connections = 0;
        match result {
            Ok(Outcome::Completed { bytes, .. }) => { e.view.status = "Completed".into(); e.view.written = bytes; e.view.total = Some(bytes); e.view.error = None; }
            Ok(Outcome::Paused) => e.view.status = "Paused".into(),
            Err(error) => { e.view.status = "Failed".into(); e.view.error = Some(error.message); }
        }
        emit(&app, &e.view);
    });
}

fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) { out.push(b); i += 3; continue; }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn run() {
    let state = Arc::new(AppState { engine: Engine::new(Options::default(), HostLimits::new([])), downloads: Mutex::new(HashMap::new()), order: Mutex::new(Vec::new()) });
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![list_downloads, add_download, pause_download, resume_download])
        .run(tauri::generate_context!())
        .expect("Flux could not start");
}
