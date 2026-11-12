//! Magnet links from the browser.
use kestrel_ipc::Response;
use tauri::AppHandle;

use crate::{PendingRequest, offer};

/// Opens the torrent window for a magnet link; the browser only needs to know it was taken.
pub async fn offer_magnet(app: &AppHandle, magnet: &str) -> Response {
    let app = app.clone();
    let pending = PendingRequest { url: magnet.to_string(), ..Default::default() };
    match tauri::async_runtime::spawn(async move { offer(&app, pending, false) }).await {
        Ok(Ok(_)) => Response::ok(),
        Ok(Err(error)) => Response::error(error),
        Err(error) => Response::error(error.to_string()),
    }
}
