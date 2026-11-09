//! Torrents (M5).
use flux_ipc::Response;
use tauri::AppHandle;

pub fn offer_magnet(_app: &AppHandle, _magnet: &str) -> Response {
    Response::error("Torrents arrive in a later Flux 2.0 build.")
}
