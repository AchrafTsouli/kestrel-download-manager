//! Video requests from the browser, and keeping yt-dlp current.

use std::time::{Duration, SystemTime};

use kestrel_ipc::{Request, Response};
use tauri::{AppHandle, Emitter, Manager as _};

use crate::manager::Shared;
use crate::{PendingRequest, offer};

/// The video panel's quality list for a page.
pub async fn formats(app: &AppHandle, request: &Request) -> Response {
    let manager = app.state::<Shared>().inner().clone();
    if !manager.media.available() {
        return Response::error("Kestrel's video tools are missing. Reinstall Kestrel to restore them.");
    }
    let url = request.url.clone().unwrap_or_default();
    let cancel = tokio_util::sync::CancellationToken::new();
    match tokio::time::timeout(Duration::from_secs(115), manager.media.formats(&url, request.session.as_ref(), &cancel)).await {
        Ok(Ok(catalog)) => Response { ok: true, video: serde_json::to_value(catalog).ok(), ..Default::default() },
        Ok(Err(error)) => Response::error(error),
        Err(_) => {
            cancel.cancel();
            Response::error("The video site did not answer in time.")
        }
    }
}

/// A quality chosen in the browser's video panel.
pub async fn download(app: &AppHandle, request: &Request) -> Response {
    let selection: kestrel_media::Selection = match request.video.clone().map(serde_json::from_value) {
        Some(Ok(selection)) => selection,
        _ => return Response::error("Invalid video format selection."),
    };
    let url = request.url.clone().unwrap_or_default();
    if let Err(error) = selection.validate(&url) {
        return Response::error(error);
    }
    let pending = PendingRequest { url, file_name: request.file_name.clone(), session: request.session.clone(), request_id: request.request_id.clone(), video: Some(selection), ..Default::default() };
    let app = app.clone();
    match tauri::async_runtime::spawn(async move { offer(&app, pending, false) }).await {
        Ok(Ok(id)) => Response { ok: true, job_id: id, ..Default::default() },
        Ok(Err(error)) => Response::error(error),
        Err(error) => Response::error(error.to_string()),
    }
}

/// Once a day (when enabled in Options), installs a newer verified yt-dlp: sites change often.
pub fn keep_tools_current(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            let manager = app.state::<Shared>().inner().clone();
            let marker = manager.media.update_directory.join("last-check");
            let due = std::fs::metadata(&marker).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_none_or(|age| age > Duration::from_secs(24 * 3600));
            if manager.settings.read().unwrap().auto_update_tools && manager.media.available() && due {
                let _ = std::fs::create_dir_all(&manager.media.update_directory);
                let _ = std::fs::write(&marker, b"");
                if let Ok(Some(version)) = kestrel_media::update::update(&manager.media.directory, &manager.media.update_directory).await {
                    let _ = app.emit("message", format!("Video support updated (yt-dlp {version})."));
                }
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}
