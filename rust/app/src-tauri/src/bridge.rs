//! Flux's side of the browser bridge: answers requests that `flux-nmhost` brings over the per-user pipe.

use flux_ipc::{CapturePolicy, Request, Response, read_frame, write_frame};
use tauri::{AppHandle, Emitter, Manager as _};

use crate::manager::Shared;
use crate::{PendingRequest, offer};

pub fn serve(app: AppHandle) {
    #[cfg(windows)]
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            let mut server = match flux_ipc::pipe::create_server(first) {
                Ok(server) => server,
                Err(error) => {
                    let _ = app.emit("message", format!("The browser connection could not start: {error}"));
                    return;
                }
            };
            first = false;
            if server.connect().await.is_err() {
                continue;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(Some(request)) = read_frame::<Request>(&mut server).await {
                    let response = handle(&app, request).await;
                    let _ = write_frame(&mut server, &response).await;
                }
            });
        }
    });
    #[cfg(unix)]
    tauri::async_runtime::spawn(async move {
        let listener = match flux_ipc::pipe::listen() {
            Ok(listener) => listener,
            Err(error) => {
                let _ = app.emit("message", format!("The browser connection could not start: {error}"));
                return;
            }
        };
        while let Ok(mut stream) = listener.accept().await {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(Some(request)) = read_frame::<Request>(&mut stream).await {
                    let response = handle(&app, request).await;
                    let _ = write_frame(&mut stream, &response).await;
                }
            });
        }
    });
}

async fn handle(app: &AppHandle, request: Request) -> Response {
    if let Err(error) = flux_ipc::validate(&request) {
        return Response::error(error);
    }
    let manager = app.state::<Shared>().inner().clone();
    match request.action.as_str() {
        "ping" => Response::ok(),
        "show" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            Response::ok()
        }
        "quit" => {
            manager.shutdown();
            app.exit(0);
            Response::ok()
        }
        "pause-all" => { manager.pause_all(); Response::ok() }
        "list" => {
            let list: Vec<serde_json::Value> = manager.views().into_iter().map(|v| serde_json::json!({
                "id": v.id, "name": v.name, "status": v.status, "written": v.written, "total": v.total, "bytesPerSecond": v.bytes_per_second, "url": v.url,
            })).collect();
            Response { ok: true, downloads: Some(serde_json::Value::Array(list)), ..Default::default() }
        }
        "resume-all" => { manager.resume_all(); Response::ok() }
        "capture-policy" => {
            let s = manager.settings();
            Response { ok: true, policy: Some(CapturePolicy { enabled: s.capture_enabled, context_menu: true, extensions: s.capture_extensions, excluded_sites: s.excluded_sites, excluded_addresses: s.excluded_addresses, bypass_key: s.bypass_key, force_key: s.force_key, media_panel: s.video_panel }), ..Default::default() }
        }
        "pause" | "resume" => {
            let ids: Vec<String> = manager.views().into_iter().filter(|v| request.url.as_deref() == Some(v.url.as_str())).map(|v| v.id).collect();
            if request.action == "pause" { manager.pause(&ids) } else { manager.resume(&ids) }
            Response::ok()
        }
        "video-formats" => crate::media::formats(app, &request).await,
        "download" => {
            let url = request.url.clone().unwrap_or_default();
            if flux_ipc::is_magnet(&url) {
                return crate::torrents::offer_magnet(app, &url).await;
            }
            if request.video.is_some() {
                return crate::media::download(app, &request).await;
            }
            let pending = PendingRequest { url, file_name: request.file_name.clone(), session: request.session.clone(), request_id: request.request_id.clone(), ..Default::default() };
            // The browser waits for this answer before cancelling its own copy; the start dialog
            // keeps the request, so it is accepted now and nothing is lost if the user is away.
            let app = app.clone();
            match tauri::async_runtime::spawn(async move { offer(&app, pending, false) }).await {
                Ok(Ok(id)) => Response { ok: true, job_id: id, ..Default::default() },
                Ok(Err(error)) => Response::error(error),
                Err(error) => Response::error(error.to_string()),
            }
        }
        _ => Response::error("Unsupported bridge action."),
    }
}
