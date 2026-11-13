//! Work in the background: the scheduler, the clipboard watcher and the tray icon with its menu.

use super::*;


/// Starts and stops queues at their scheduled times (checked every 20 seconds).
pub(crate) fn scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut fired: std::collections::HashSet<String> = Default::default();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            let manager = app.state::<Shared>().inner().clone();
            let (day, now) = system::local_time();
            let stamp = format!("{day}-{now}");
            for queue in manager.settings.read().queues.clone() {
                if !queue.schedule_enabled || !(queue.days.is_empty() || queue.days.contains(&day)) {
                    continue;
                }
                if queue.start_time.as_deref() == Some(now.as_str()) && fired.insert(format!("{stamp}-start-{}", queue.name)) {
                    manager.start_queue(&queue.name);
                    if let Some(sound) = manager.settings.read().sounds.get(2).filter(|s| s.on) { system::play(&sound.file, true); }
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
pub(crate) fn clipboard_monitor(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last = String::new();
        let mut clipboard = arboard::Clipboard::new().ok();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let manager = app.state::<Shared>().inner().clone();
            let settings = manager.settings.read().clone();
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
            if matches!(url.scheme(), "http" | "https") && wanted && !manager.has_url(&text) {
                let _ = offer(&app, PendingRequest { url: text, ..Default::default() }, true);
            }
        }
    });
}

/// What the tray menu shows; it is rebuilt only when this changes (a rebuild closes an open menu).
#[derive(Clone, PartialEq, Default)]
pub(crate) struct TrayLook {
    language: String,
    limiter: bool,
    main_running: bool,
    running: Vec<(String, String)>,
}
pub(crate) fn tray_look(manager: &Shared) -> TrayLook {
    let settings = manager.settings.read();
    TrayLook {
        language: i18n::resolve(&settings.language).to_string(),
        limiter: settings.speed_limit > 0,
        main_running: manager.running_queues.lock().iter().any(|q| q == crate::model::MAIN_QUEUE),
        running: manager.views_where(|r| r.status.running()).into_iter().map(|v| (v.id, v.name)).collect(),
    }
}
/// A download's name as the menu shows it: short, and "&" doubled (menus read it as a shortcut mark).
pub(crate) fn menu_name(name: &str) -> String {
    let mut s: String = name.chars().take(48).collect();
    if name.chars().count() > 48 {
        s.push('…');
    }
    s.replace('&', "&&")
}

/// The tray menu in the interface language, like MDM's.
pub(crate) fn tray_menu(app: &AppHandle, look: &TrayLook) -> tauri::Result<Menu<tauri::Wry>> {
    let language = look.language.as_str();
    let item = |id: &str, text: &'static str, on: bool| MenuItem::with_id(app, id, i18n::tr(language, text), on, None::<&str>);
    let sep = || PredefinedMenuItem::separator(app);
    let downloads = Submenu::new(app, i18n::tr(language, "Downloads in progress"), !look.running.is_empty())?;
    for (id, name) in &look.running {
        downloads.append(&MenuItem::with_id(app, format!("dl:{id}"), menu_name(name), true, None::<&str>)?)?;
    }
    let limiter = Submenu::with_items(app, i18n::tr(language, "Speed Limiter"), true, &[
        &CheckMenuItem::with_id(app, "limit-on", i18n::tr(language, "Turn on"), true, look.limiter, None::<&str>)?,
        &CheckMenuItem::with_id(app, "limit-off", i18n::tr(language, "Turn off"), true, !look.limiter, None::<&str>)?,
    ])?;
    Menu::with_items(app, &[
        &item("show", "Open Kestrel", true)?,
        &sep()?,
        &item("add", "Add new download", true)?,
        &item("batch", "Add batch download", true)?,
        &downloads,
        &sep()?,
        &limiter,
        &item("scheduler", "Scheduler", true)?,
        &item("queue-start", "Start main download queue", !look.main_running)?,
        &item("queue-stop", "Stop main download queue", look.main_running)?,
        &item("pause", "Pause all", true)?,
        &item("resume", "Resume all", true)?,
        &item("options", "Options", true)?,
        &sep()?,
        &item("quit", "Exit", true)?,
    ])
}

pub(crate) fn set_limiter(app: &AppHandle, on: bool) {
    let manager = app.state::<Shared>();
    let mut settings = manager.settings();
    settings.speed_limit = if on { if settings.last_speed_limit > 0 { settings.last_speed_limit } else { 1024 * 1024 } } else { 0 };
    let _ = manager.save_settings(settings);
}

pub(crate) fn tray(app: &AppHandle) -> tauri::Result<()> {
    let first = tray_look(&app.state::<Shared>());
    let menu = tray_menu(app, &first)?;
    // Kept in step with the app (language, limiter, queue, running downloads), rebuilt only on change.
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut shown = first;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let look = tray_look(&handle.state::<Shared>());
            if look == shown {
                continue;
            }
            if let (Some(tray), Ok(menu)) = (handle.tray_by_id("kestrel"), tray_menu(&handle, &look)) {
                let _ = tray.set_menu(Some(menu));
            }
            shown = look;
        }
    });
    TrayIconBuilder::with_id("kestrel")
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip("Kestrel")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "add" => { show_main(app); let _ = app.emit("open-add", ()); }
            "batch" => open_window(app, "batch", "Add batch download", 560.0, 420.0, true),
            "scheduler" => open_window(app, "scheduler", "Scheduler", 700.0, 580.0, true),
            "options" => open_window(app, "options", "Options", 720.0, 600.0, true),
            "limit-on" => set_limiter(app, true),
            "limit-off" => set_limiter(app, false),
            "queue-start" => app.state::<Shared>().start_queue(crate::model::MAIN_QUEUE),
            "queue-stop" => app.state::<Shared>().stop_queue(crate::model::MAIN_QUEUE),
            "pause" => app.state::<Shared>().pause_all(),
            "resume" => app.state::<Shared>().resume_all(),
            "quit" => { app.state::<Shared>().shutdown(); app.exit(0); }
            other => {
                if let Some(id) = other.strip_prefix("dl:") {
                    open_progress(app, id);
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
