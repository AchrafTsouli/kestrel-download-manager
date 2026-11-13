//! Windows niceties: overall progress on the taskbar button, notifications when a download ends
//! while Kestrel is out of sight, and the picture shown while dragging a finished file out.

use std::path::PathBuf;
use std::time::Duration;

use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Manager as _};
use tauri_plugin_notification::NotificationExt;

use crate::manager::Shared;
use crate::model::Status;

/// The taskbar button fills with the progress of everything running (green; yellow when only
/// paused or retrying downloads are left with work; nothing when idle).
pub fn taskbar_progress(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last: Option<(u8, u64)> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let running = app.state::<Shared>().views_where(|r| r.status.running());
            let (written, total) = running.iter().filter_map(|v| v.total.map(|t| (v.written.min(t), t))).fold((0u64, 0u64), |(w, t), (a, b)| (w + a, t + b));
            let retrying = running.iter().any(|v| v.status == Status::Retrying);
            let state = if running.is_empty() {
                (0, 0)
            } else {
                match (written * 100).checked_div(total) {
                    None => (1, 0), // sizes not known yet
                    Some(percent) => (if retrying { 3 } else { 2 }, percent),
                }
            };
            if last == Some(state) {
                continue;
            }
            last = Some(state);
            let Some(window) = app.get_webview_window("main") else { continue };
            let bar = match state {
                (0, _) => ProgressBarState { status: Some(ProgressBarStatus::None), progress: None },
                (1, _) => ProgressBarState { status: Some(ProgressBarStatus::Indeterminate), progress: None },
                (3, p) => ProgressBarState { status: Some(ProgressBarStatus::Paused), progress: Some(p) },
                (_, p) => ProgressBarState { status: Some(ProgressBarStatus::Normal), progress: Some(p) },
            };
            let _ = window.set_progress_bar(bar);
        }
    });
}

/// True when the user is looking at Kestrel's main window.
fn in_view(app: &AppHandle) -> bool {
    app.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

fn notify(app: &AppHandle, title: &'static str, body: &str) {
    let language = crate::i18n::resolve(&app.state::<Shared>().settings.read().language);
    let _ = app.notification().builder().title(crate::i18n::tr(language, title)).body(body).show();
}

/// A finished download: a notification, unless its "Download complete" window shows instead.
pub fn completed(app: &AppHandle, id: &str, window_shown: bool) {
    if window_shown || in_view(app) {
        return;
    }
    if let Some(view) = app.state::<Shared>().view(id) {
        notify(app, "Download complete", &format!("{}\n{}", view.name, view.folder));
    }
}

pub fn failed(app: &AppHandle, id: &str, error: &str) {
    if in_view(app) {
        return;
    }
    if let Some(view) = app.state::<Shared>().view(id) {
        notify(app, "Download failed", &format!("{}\n{}", view.name, error.chars().take(200).collect::<String>()));
    }
}

/// The picture under the pointer while a file is dragged out of Kestrel.
pub fn drag_icon() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join("kestrel-drag-icon.png");
    if !path.exists() {
        std::fs::write(&path, include_bytes!("../icons/32x32.png")).map_err(|e| e.to_string())?;
    }
    Ok(path)
}
