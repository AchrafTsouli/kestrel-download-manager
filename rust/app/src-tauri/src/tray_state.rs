//! Downloads sent to the background (the progress window's green arrow, like IDM). They share one
//! extra tray icon beside Kestrel's own: its badge tells their state (downloading, paused,
//! complete, error), its tooltip sums them up, and a click opens a small menu listing them; a
//! download picked there gets its progress window back. The icon goes away when none are left.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager as _};

use crate::manager::{Shared, now_ms};
use crate::model::{Status, View};

const TRAY_ID: &str = "background";

const ICONS: [(&str, &[u8], &[u8]); 4] = [
    ("downloading", include_bytes!("../icons/tray/dark-downloading.png"), include_bytes!("../icons/tray/light-downloading.png")),
    ("paused", include_bytes!("../icons/tray/dark-paused.png"), include_bytes!("../icons/tray/light-paused.png")),
    ("complete", include_bytes!("../icons/tray/dark-complete.png"), include_bytes!("../icons/tray/light-complete.png")),
    ("error", include_bytes!("../icons/tray/dark-error.png"), include_bytes!("../icons/tray/light-error.png")),
];

/// Download IDs in the background, with when they finished (to show the check for a moment).
#[derive(Default)]
pub struct Background {
    ids: Mutex<HashSet<String>>,
    finished_at: Mutex<HashMap<String, u64>>,
}

impl Background {
    pub fn add(&self, id: &str) {
        self.ids.lock().unwrap().insert(id.to_string());
    }
    pub fn remove(&self, id: &str) {
        self.ids.lock().unwrap().remove(id);
        self.finished_at.lock().unwrap().remove(id);
    }
}

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

fn size(bytes: f64) -> String {
    match bytes {
        b if b >= 1073741824.0 => format!("{:.2} GB", b / 1073741824.0),
        b if b >= 1048576.0 => format!("{:.1} MB", b / 1048576.0),
        b if b >= 1024.0 => format!("{:.0} KB", b / 1024.0),
        b => format!("{b:.0} B"),
    }
}

fn percent(v: &View) -> Option<f64> {
    v.total.filter(|t| *t > 0).map(|t| (v.written as f64 * 100.0 / t as f64).min(100.0))
}

/// One menu line: "Setup.zip — 45% • 3.1 MB/s".
fn line(v: &View) -> String {
    let name: String = if v.name.chars().count() > 40 { v.name.chars().take(38).collect::<String>() + "…" } else { v.name.clone() };
    let detail = match v.status {
        Status::Completed => "complete".to_string(),
        Status::Failed => "failed".to_string(),
        Status::Paused => format!("paused{}", percent(v).map(|p| format!(" at {p:.0}%")).unwrap_or_default()),
        _ => {
            let rate = if v.bytes_per_second > 0.0 { format!(" • {}/s", size(v.bytes_per_second)) } else { String::new() };
            format!("{}{rate}", percent(v).map(|p| format!("{p:.0}%")).unwrap_or_else(|| size(v.written as f64)))
        }
    };
    format!("{name} — {detail}")
}

/// The menu and its lines by download ID, so the lines can be updated in place: replacing the
/// menu every second would close it while the user is reading it.
fn menu(app: &AppHandle, views: &[View]) -> tauri::Result<(Menu<tauri::Wry>, HashMap<String, MenuItem<tauri::Wry>>)> {
    let menu = Menu::new(app)?;
    let mut items = HashMap::new();
    for v in views {
        let item = MenuItem::with_id(app, format!("bg:{}", v.id), line(v), true, None::<&str>)?;
        menu.append(&item)?;
        items.insert(v.id.clone(), item);
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "bg-show-all", "Show all progress windows", true, None::<&str>)?)?;
    Ok((menu, items))
}

fn reopen(app: &AppHandle, id: &str) {
    app.state::<Background>().remove(id);
    crate::open_progress(app, id);
}

/// Sends a download to the background: its progress window closes and the tray icon shows it.
pub fn send(app: &AppHandle, id: &str) {
    app.state::<Background>().add(id);
    if let Some(window) = app.get_webview_window(&format!("progress-{id}")) {
        let _ = window.close();
    }
}

pub fn watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut shown = (String::new(), false, String::new(), String::new());
        // The menu's downloads (in order) and its lines with the text they show.
        let mut listed: Vec<String> = Vec::new();
        let mut items: HashMap<String, (MenuItem<tauri::Wry>, String)> = HashMap::new();
        let mut tick = 0u32;
        let mut light = light_taskbar();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tick += 1;
            if tick % 10 == 0 {
                light = light_taskbar();
            }
            let background = app.state::<Background>();
            let now = now_ms();
            let views: Vec<View> = {
                let wanted = background.ids.lock().unwrap().clone();
                if wanted.is_empty() && shown.0.is_empty() {
                    continue;
                }
                let found = app.state::<Shared>().views_where(|r| wanted.contains(&r.id));
                // Downloads removed from the list leave the background too.
                background.ids.lock().unwrap().retain(|id| found.iter().any(|v| &v.id == id));
                found
            };
            // A finished download shows its check for a few seconds, then leaves the icon.
            {
                let mut finished = background.finished_at.lock().unwrap();
                for v in views.iter().filter(|v| v.status == Status::Completed) {
                    finished.entry(v.id.clone()).or_insert(now);
                }
                let done: Vec<String> = finished.iter().filter(|(_, t)| now.saturating_sub(**t) > 8_000).map(|(id, _)| id.clone()).collect();
                drop(finished);
                for id in done {
                    background.remove(&id);
                }
            }
            let views: Vec<View> = views.into_iter().filter(|v| background.ids.lock().unwrap().contains(&v.id)).collect();

            if views.is_empty() {
                if !shown.0.is_empty() {
                    let _ = app.remove_tray_by_id(TRAY_ID);
                    shown = Default::default();
                    listed.clear();
                    items.clear();
                }
                continue;
            }
            let running: Vec<&View> = views.iter().filter(|v| v.status.running()).collect();
            let state = if views.iter().any(|v| v.status == Status::Failed) {
                "error"
            } else if !running.is_empty() {
                "downloading"
            } else if views.iter().any(|v| v.status == Status::Paused || v.status == Status::Queued) {
                "paused"
            } else {
                "complete"
            };
            let (written, total) = views.iter().filter_map(|v| v.total.map(|t| (v.written.min(t), t))).fold((0u64, 0u64), |a, b| (a.0 + b.0, a.1 + b.1));
            let rate: f64 = running.iter().map(|v| v.bytes_per_second).sum();
            let mut tooltip = format!("Kestrel — {} download{}", views.len(), if views.len() == 1 { "" } else { "s" });
            if total > 0 {
                tooltip.push_str(&format!(" • {:.0}%", written as f64 * 100.0 / total as f64));
            }
            if rate > 0.0 {
                tooltip.push_str(&format!(" • {}/s", size(rate)));
            }
            let lines = views.iter().map(line).collect::<Vec<_>>().join("\n");
            let next = (state.to_string(), light, tooltip, lines);
            if next == shown {
                continue;
            }
            let icon = ICONS.iter().find(|(name, _, _)| *name == state).and_then(|(_, dark, bright)| Image::from_bytes(if light { bright } else { dark }).ok());
            let ids: Vec<String> = views.iter().map(|v| v.id.clone()).collect();
            // A new menu only when downloads join or leave; otherwise its lines change in place.
            let new_menu = if ids != listed {
                match menu(&app, &views) {
                    Ok((menu, lines)) => {
                        items = lines.into_iter().map(|(id, item)| { let text = views.iter().find(|v| v.id == id).map(line).unwrap_or_default(); (id, (item, text)) }).collect();
                        listed = ids;
                        Some(menu)
                    }
                    Err(_) => continue,
                }
            } else {
                for v in &views {
                    if let Some((item, text)) = items.get_mut(&v.id) {
                        let now_text = line(v);
                        if *text != now_text {
                            let _ = item.set_text(&now_text);
                            *text = now_text;
                        }
                    }
                }
                None
            };
            match app.tray_by_id(TRAY_ID) {
                Some(tray) => {
                    if next.0 != shown.0 || next.1 != shown.1 {
                        let _ = tray.set_icon(icon);
                    }
                    let _ = tray.set_tooltip(Some(&next.2));
                    if let Some(menu) = new_menu {
                        let _ = tray.set_menu(Some(menu));
                    }
                }
                None => {
                    let Some(menu) = new_menu.or_else(|| menu(&app, &views).ok().map(|(m, _)| m)) else { continue };
                    let mut builder = TrayIconBuilder::with_id(TRAY_ID).tooltip(&next.2).menu(&menu).show_menu_on_left_click(true).on_menu_event(|app, event| {
                        let id = event.id.as_ref();
                        if let Some(id) = id.strip_prefix("bg:") {
                            reopen(app, id);
                        } else if id == "bg-show-all" {
                            let ids: Vec<String> = app.state::<Background>().ids.lock().unwrap().iter().cloned().collect();
                            for id in ids {
                                reopen(app, &id);
                            }
                        }
                    });
                    if let Some(icon) = icon {
                        builder = builder.icon(icon);
                    }
                    let _ = builder.build(&app);
                }
            }
            shown = next;
        }
    });
}
