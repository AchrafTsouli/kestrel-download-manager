//! Kestrel 2.0's desktop app: windows, tray, scheduler and commands on top of the download manager.

mod applog;
mod commands;
mod tray;
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
mod exeicon;
mod browsers;

use std::collections::HashMap;
use parking_lot::Mutex;

use tray::*;
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
    pub video: Option<kestrel_media::Probe>,
    /// The torrent's file list, once read.
    #[serde(skip)]
    pub torrent: Option<kestrel_torrent::Info>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub url: String,
    pub file_name: Option<String>,
    pub description: Option<String>,
    #[serde(skip_serializing)]
    pub session: Option<kestrel_engine::Session>,
    pub request_id: Option<String>,
    /// A quality already chosen (in the browser's video panel or the picker).
    pub video: Option<kestrel_media::Selection>,
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
    kestrel_torrent::magnet_hash(text).is_some() || text.to_ascii_lowercase().ends_with(".torrent") && std::path::Path::new(text).is_file()
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
        app.state::<Windows>().pending.lock().insert(pending_id.clone(), Pending { id: pending_id.clone(), request, probe: None, video: None, torrent: None });
        open_torrent(app, &pending_id);
        return Ok(None);
    }
    // A video page without a chosen quality opens the quality list first.
    if request.video.is_none() && !request.as_file && manager.media.available() && kestrel_media::pages::is_known_video_page(&request.url) {
        let pending_id = pending_id();
        app.state::<Windows>().pending.lock().insert(pending_id.clone(), Pending { id: pending_id.clone(), request, probe: None, video: None, torrent: None });
        open_video(app, &pending_id);
        return Ok(None);
    }
    let show_dialog = force_dialog || manager.settings.read().show_start_dialog;
    if !show_dialog {
        let start = if manager.settings.read().only_add_to_queue { "queue" } else { "now" };
        let id = manager.add(AddRequest { url: request.url, file_name: request.file_name, description: request.description, session: request.session, request_id: request.request_id, video: request.video, start: Some(start.into()), ..Default::default() })?;
        if manager.settings.read().show_progress_window {
            open_progress(app, &id);
        }
        return Ok(Some(id));
    }
    let pending_id = pending_id();
    if let Some(selection) = &request.video {
        // Nothing to ask the server: the quality list already said what the file will be.
        let probe = video_probe(&manager, &request, selection);
        app.state::<Windows>().pending.lock().insert(pending_id.clone(), Pending { id: pending_id.clone(), request, probe: Some(probe), video: None, torrent: None });
        open_start(app, &pending_id);
        return Ok(None);
    }
    app.state::<Windows>().pending.lock().insert(pending_id.clone(), Pending { id: pending_id.clone(), request: request.clone(), probe: None, video: None, torrent: None });
    open_start(app, &pending_id);
    // Ask the server for the real name and size while the dialog opens.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let probe = manager.probe(&request.url, request.session.clone(), request.file_name.clone()).await;
        if let Some(pending) = app.state::<Windows>().pending.lock().get_mut(&pending_id) {
            pending.probe = Some(probe.clone());
        }
        let _ = app.emit("probe", (pending_id, probe));
    });
    Ok(None)
}

/// The start dialog's file information for a chosen video quality.
fn video_probe(manager: &Manager, request: &PendingRequest, selection: &kestrel_media::Selection) -> Probe {
    let category = if selection.audio_only { "Music" } else { "Video" };
    let stem = request.file_name.as_deref().map(|n| std::path::Path::new(n).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Video".into());
    Probe {
        url: request.url.clone(),
        file_name: kestrel_engine::sanitize_file_name(&format!("{stem}.{}", selection.extension)),
        total: selection.total_bytes,
        resumable: true,
        content_type: None,
        category: category.into(),
        folder: manager.folder_for(category).to_string_lossy().into_owned(),
        error: None,
        final_url: None,
    }
}

pub fn run() {
    // A portable copy keeps the window's browser data in its own data folder too, so nothing is
    // left in %LOCALAPPDATA% (WebView2 reads this variable; set before any thread or window starts).
    if kestrel_store::is_portable() {
        let webview = kestrel_store::default_directory().join("webview");
        let _ = std::fs::create_dir_all(&webview);
        // SAFETY: the first thing the program does, before any other thread exists.
        unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview) };
    }
    // The app's identifier was com.achraftsouli.flux before the rename: its window data moves once.
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).filter(|_| !kestrel_store::is_portable()) {
        let (old, new) = (local.join("com.achraftsouli.flux"), local.join("com.achraftsouli.kestrel"));
        if !new.exists() && old.is_dir() {
            let _ = std::fs::rename(old, new);
        }
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A second start (or a link passed on the command line) goes to the running Kestrel.
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
            applog::info(format!("Kestrel {} started ({} {})", app.package_info().version, std::env::consts::OS, std::env::consts::ARCH));
            system::hold_running_mutex();
            // Off the start-up path: a few registry reads, and a registration when it is missing.
            std::thread::spawn(system::ensure_browser_bridge);
            let data_dir = kestrel_store::default_directory();
            let manager = Manager::open(handle.clone(), &data_dir).map_err(Box::<dyn std::error::Error>::from)?;
            app.manage(manager.clone());
            // The first start: how to add the browser extension (until it is in the stores).
            if !manager.settings().extension_guide_shown {
                let mut settings = manager.settings();
                settings.extension_guide_shown = true;
                let _ = manager.save_settings(settings);
                let shower = handle.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    open_window(&shower, "extension", "Add the browser extension", 600.0, 470.0, false);
                });
            }
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
                let settings = manager.settings.read().clone();
                if let Some(window) = completer.get_webview_window(&format!("progress-{id}")) { let _ = window.close(); }
                if let Some(sound) = settings.sounds.first().filter(|s| s.on) { system::play(&sound.file, true); }
                if settings.show_complete_dialog { open_complete(&completer, &id); }
                // "Turn off computer / exit when done" chosen in its progress window: the main
                // window counts down and can still cancel it.
                if let Some(action) = completer.state::<Windows>().after.lock().remove(&id) {
                    let name = manager.view(&id).map(|v| v.name).unwrap_or_default();
                    show_main(&completer);
                    let _ = completer.emit("queue-finished", (name, action));
                }
                desktop::completed(&completer, &id, settings.show_complete_dialog);
            });
            let failures = handle.clone();
            handle.listen("failed", move |event| {
                if let Some(sound) = failures.state::<Shared>().settings.read().sounds.get(1).filter(|s| s.on) { system::play(&sound.file, false); }
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
            // The main window starts hidden; Kestrel started by Windows or the browser stays in the tray.
            if !std::env::args().any(|a| a == "--background")
                && let Some(window) = handle.get_webview_window("main") { let _ = window.show(); let _ = window.set_focus(); }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main" {
                    let manager = window.app_handle().state::<Shared>();
                    if manager.settings.read().minimize_to_tray {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        manager.shutdown();
                        window.app_handle().exit(0);
                    }
                }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_downloads, commands::get_download, commands::get_settings, commands::save_settings, commands::offer_url, commands::get_pending, commands::confirm_pending, commands::cancel_pending,
            commands::probe_video, commands::video_exact_cut, commands::confirm_video, commands::video_as_file, commands::load_torrent, commands::confirm_torrent, commands::open_torrent_file, commands::check_updates, commands::install_update, commands::drag_item, commands::grab_site, commands::run_waiting, commands::register_browsers, commands::test_browsers, commands::set_after_complete, commands::get_after_complete, commands::open_with, commands::to_background, commands::crash_reports, commands::dismiss_crash, commands::report_crash, commands::open_log, commands::refresh_address, commands::rename_with_pattern, commands::file_checksum, commands::export_settings, commands::import_settings,
            commands::add_downloads, commands::pause_downloads, commands::resume_downloads, commands::pause_all, commands::resume_all, commands::remove_downloads, commands::remove_completed,
            commands::rename_download, commands::redownload, commands::zip_preview, commands::extension_setup, commands::open_extensions_page, commands::file_icon, commands::pending_icon, commands::type_icon, commands::move_in_queue, commands::write_link_list, commands::read_link_list, commands::set_speed_limit, commands::set_queue, commands::enqueue, commands::start_queue, commands::stop_queue, commands::running_queues,
            commands::open_file, commands::show_in_folder, commands::open_folder, commands::show_progress, commands::show_window, commands::power_action, commands::exit_app, commands::app_version
        ])
        .run(tauri::generate_context!())
        .expect("Kestrel could not start");
}
