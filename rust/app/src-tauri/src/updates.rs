//! Flux's own updates, through the signed feed (flux-update): a check once a day when enabled, or
//! from Help > Check for updates; installing pauses everything, saves, and lets the installer run
//! once Flux has closed (it starts the new Flux when done).

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager as _};

use crate::manager::{Shared, now_ms};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub version: String,
    pub available: bool,
    /// Can be installed automatically; otherwise only its download page can be opened.
    pub signed: bool,
    pub url: String,
}

fn current(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

pub async fn check(app: &AppHandle) -> Result<UpdateInfo, String> {
    let manager = app.state::<Shared>().inner().clone();
    let feed = manager.settings.read().unwrap().update_feed.clone();
    let release = flux_update::check(&feed).await?;
    let mut settings = manager.settings();
    settings.last_update_check_ms = now_ms();
    let _ = manager.save_settings(settings);
    let current = current(app);
    Ok(UpdateInfo { available: flux_update::is_newer(&release.version, &current), current, version: release.version, signed: release.signed, url: release.url })
}

/// Downloads the newer signed installer (checked against its signed checksum), then closes Flux
/// and runs it silently.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let manager = app.state::<Shared>().inner().clone();
    let feed = manager.settings.read().unwrap().update_feed.clone();
    let release = flux_update::check(&feed).await?;
    if !flux_update::is_newer(&release.version, &current(app)) {
        return Err(format!("Flux {} is up to date.", current(app)));
    }
    // The signed installer is for Windows; elsewhere the release page offers the packages.
    #[cfg(not(windows))]
    {
        let _ = crate::system::open_path(std::path::Path::new("https://github.com/AchrafTsouli/flux-downloader/releases/latest"));
        return Ok(());
    }
    #[cfg_attr(not(windows), allow(unreachable_code))]
    let _ = app.emit("message", format!("Downloading Flux {}…", release.version));
    let installer = flux_update::download_installer(&release).await?;
    manager.shutdown();
    // The installer waits for Flux to close (it holds the app's mutex), so start it a moment later.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let command = format!("/d /c ping -n 4 127.0.0.1 >nul & \"{}\" /SILENT /SUPPRESSMSGBOXES /NORESTART", installer.display());
        std::process::Command::new("cmd.exe").raw_arg(command).creation_flags(0x0800_0000).spawn().map_err(|e| format!("The installer could not start: {e}"))?;
    }
    app.exit(0);
    Ok(())
}

/// Once a day when enabled in Options: tell the main window a newer Flux exists.
pub fn check_daily(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let (enabled, last) = {
                let settings = app.state::<Shared>().settings.read().unwrap().clone();
                (settings.auto_check_updates, settings.last_update_check_ms)
            };
            if enabled && now_ms().saturating_sub(last) > 24 * 3600 * 1000 {
                if let Ok(info) = check(&app).await {
                    if info.available {
                        let _ = app.emit("update-available", info);
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}
