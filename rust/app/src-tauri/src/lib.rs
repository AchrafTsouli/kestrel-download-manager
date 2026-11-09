//! Flux 2.0's desktop app: windows, tray, scheduler and commands on top of the download manager.

mod bridge;
mod manager;
mod media;
mod model;
mod system;
mod torrents;

use std::collections::HashMap;
use std::sync::Mutex;

use manager::{AddRequest, Manager, Probe, Shared, now_ms};
use model::{AfterQueue, Settings, View};
use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Listener, Manager as _, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// A download waiting in the start dialog ("Download File Info") before it is added.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub id: String,
    pub request: PendingRequest,
    pub probe: Option<Probe>,
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
}

#[derive(Default)]
pub struct Windows {
    pending: Mutex<HashMap<String, Pending>>,
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
        .center()
        .focused(true)
        .build();
    if let Err(error) = built {
        let _ = app.emit("message", format!("A window could not open: {error}"));
    }
}

pub fn open_progress(app: &AppHandle, id: &str) {
    open_window(app, &format!("progress-{id}"), "Download progress", 560.0, 440.0, true);
}
pub fn open_complete(app: &AppHandle, id: &str) {
    open_window(app, &format!("complete-{id}"), "Download complete", 520.0, 300.0, false);
}
fn open_start(app: &AppHandle, pending_id: &str) {
    open_window(app, &format!("start-{pending_id}"), "Download file info", 600.0, 340.0, false);
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
    let show_dialog = force_dialog || manager.settings.read().unwrap().show_start_dialog;
    if !show_dialog {
        let id = manager.add(AddRequest { url: request.url, file_name: request.file_name, description: request.description, session: request.session, request_id: request.request_id, start: Some("now".into()), ..Default::default() })?;
        if manager.settings.read().unwrap().show_progress_window {
            open_progress(app, &id);
        }
        return Ok(Some(id));
    }
    let pending_id = format!("{:x}", now_ms() ^ (std::process::id() as u64) << 20);
    app.state::<Windows>().pending.lock().unwrap().insert(pending_id.clone(), Pending { id: pending_id.clone(), request: request.clone(), probe: None });
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
async fn offer_url(app: AppHandle, url: String, file_name: Option<String>) -> Result<Option<String>, String> {
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Enter a full address starting with http:// or https://.".into());
    }
    offer(&app, PendingRequest { url, file_name, ..Default::default() }, false)
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
}
#[tauri::command]
async fn confirm_pending(app: AppHandle, manager: Mgr<'_>, windows: State<'_, Windows>, choice: StartChoice) -> Result<String, String> {
    let pending = windows.pending.lock().unwrap().get(&choice.pending_id).cloned().ok_or("This download was already added or cancelled.")?;
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
        "options" => open_window(&app, "options", "Options", 640.0, 520.0, true),
        "scheduler" => open_window(&app, "scheduler", "Scheduler", 680.0, 500.0, true),
        "batch" => open_window(&app, "batch", "Add batch download", 560.0, 420.0, true),
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

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Open Flux", true, None::<&str>)?;
    let add = MenuItem::with_id(app, "add", "Add URL…", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause all", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume", "Resume all", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Exit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &add, &PredefinedMenuItem::separator(app)?, &pause, &resume, &PredefinedMenuItem::separator(app)?, &quit])?;
    TrayIconBuilder::with_id("flux")
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip("Flux")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "add" => { show_main(app); let _ = app.emit("open-add", ()); }
            "pause" => app.state::<Shared>().pause_all(),
            "resume" => app.state::<Shared>().resume_all(),
            "quit" => { app.state::<Shared>().shutdown(); app.exit(0); }
            _ => {}
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
            if let Some(url) = args.iter().skip(1).find(|a| a.starts_with("http://") || a.starts_with("https://")).cloned() {
                // Off the main thread: opening a window from it would wait on itself.
                let app = app.clone();
                tauri::async_runtime::spawn(async move { let _ = offer(&app, PendingRequest { url, ..Default::default() }, false); });
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(Windows::default())
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = flux_store::default_directory();
            let manager = Manager::open(handle.clone(), &data_dir).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            app.manage(manager.clone());
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
                if settings.play_sounds { system::sound(true); }
                if settings.show_complete_dialog { open_complete(&completer, &id); }
            });
            tray(&handle)?;
            scheduler(handle.clone());
            clipboard_monitor(handle.clone());
            bridge::serve(handle.clone());
            // The main window starts hidden; Flux started by Windows or the browser stays in the tray.
            if !std::env::args().any(|a| a == "--background") {
                if let Some(window) = handle.get_webview_window("main") { let _ = window.show(); let _ = window.set_focus(); }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let manager = window.app_handle().state::<Shared>();
                    if manager.settings.read().unwrap().minimize_to_tray {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        manager.shutdown();
                        window.app_handle().exit(0);
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_downloads, get_download, get_settings, save_settings, offer_url, get_pending, confirm_pending, cancel_pending,
            add_downloads, pause_downloads, resume_downloads, pause_all, resume_all, remove_downloads, remove_completed,
            rename_download, redownload, set_speed_limit, set_queue, enqueue, start_queue, stop_queue, running_queues,
            open_file, show_in_folder, open_folder, show_progress, show_window, power_action, exit_app, app_version
        ])
        .run(tauri::generate_context!())
        .expect("Flux could not start");
}
