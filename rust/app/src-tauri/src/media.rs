//! Video downloads (M4).
use flux_ipc::{Request, Response};
use tauri::AppHandle;

pub async fn formats(_app: &AppHandle, _request: &Request) -> Response {
    Response::error("Video downloads arrive in the next Flux 2.0 build.")
}
pub fn download(_app: &AppHandle, _request: &Request) -> Response {
    Response::error("Video downloads arrive in the next Flux 2.0 build.")
}
