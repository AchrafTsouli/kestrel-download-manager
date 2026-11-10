//! The tray icon shows what Kestrel is doing (the icon pack's five states, for a dark or a light
//! taskbar) and its tooltip sums it up: "Kestrel — 3 downloads • 12.4 MB/s".
//!
//! Priority, as the icon pack recommends: a recent error, then active downloads, then paused
//! work, then a just-finished download, then idle.

use std::time::Duration;

use tauri::image::Image;
use tauri::{AppHandle, Manager as _};

use crate::manager::{Shared, now_ms};
use crate::model::Status;

const ICONS: [(&str, &[u8], &[u8]); 5] = [
    ("idle", include_bytes!("../icons/tray/dark-idle.png"), include_bytes!("../icons/tray/light-idle.png")),
    ("downloading", include_bytes!("../icons/tray/dark-downloading.png"), include_bytes!("../icons/tray/light-downloading.png")),
    ("paused", include_bytes!("../icons/tray/dark-paused.png"), include_bytes!("../icons/tray/light-paused.png")),
    ("complete", include_bytes!("../icons/tray/dark-complete.png"), include_bytes!("../icons/tray/light-complete.png")),
    ("error", include_bytes!("../icons/tray/dark-error.png"), include_bytes!("../icons/tray/light-error.png")),
];

/// Windows' taskbar theme: true when it is light (a dark falcon is then used).
fn light_taskbar() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
        let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0".encode_utf16().collect();
        let value: Vec<u16> = "SystemUsesLightTheme\0".encode_utf16().collect();
        let mut data: u32 = 0;
        let mut size = 4u32;
        if RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut data as *mut u32).cast(), &mut size) == 0 {
            return data == 1;
        }
    }
    false
}

fn speed(bytes_per_second: f64) -> String {
    match bytes_per_second {
        b if b >= 1048576.0 => format!("{:.1} MB/s", b / 1048576.0),
        b if b >= 1024.0 => format!("{:.0} KB/s", b / 1024.0),
        b => format!("{b:.0} B/s"),
    }
}

pub fn watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut shown = (String::new(), false, String::new());
        let mut tick = 0u32;
        let mut light = light_taskbar();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tick += 1;
            if tick % 10 == 0 {
                light = light_taskbar();
            }
            let views = app.state::<Shared>().views();
            let now = now_ms();
            let running: Vec<_> = views.iter().filter(|v| v.status.running()).collect();
            let state = if views.iter().any(|v| v.status == Status::Failed && v.last_try_ms.is_some_and(|t| now.saturating_sub(t) < 5 * 60_000)) {
                "error"
            } else if !running.is_empty() {
                "downloading"
            } else if views.iter().any(|v| v.status == Status::Paused) {
                "paused"
            } else if views.iter().any(|v| v.completed_ms.is_some_and(|t| now.saturating_sub(t) < 60_000)) {
                "complete"
            } else {
                "idle"
            };
            let tooltip = if running.is_empty() {
                "Kestrel Download Manager".to_string()
            } else {
                let rate: f64 = running.iter().map(|v| v.bytes_per_second).sum();
                format!("Kestrel — {} download{} • {}", running.len(), if running.len() == 1 { "" } else { "s" }, speed(rate))
            };
            let next = (state.to_string(), light, tooltip);
            if next == shown {
                continue;
            }
            let Some(tray) = app.tray_by_id("flux") else { continue };
            if next.0 != shown.0 || next.1 != shown.1 {
                if let Some((_, dark, bright)) = ICONS.iter().find(|(name, _, _)| *name == state) {
                    if let Ok(image) = Image::from_bytes(if light { bright } else { dark }) {
                        let _ = tray.set_icon(Some(image));
                    }
                }
            }
            let _ = tray.set_tooltip(Some(&next.2));
            shown = next;
        }
    });
}
