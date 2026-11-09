//! Operating-system actions: open and reveal files, start with Windows, sounds, power actions, local time.

use std::path::Path;
use std::process::Command;

use chrono::{Datelike, Local, Timelike};

use crate::model::AfterQueue;

pub fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // "start" through cmd opens the file with its default program; the empty title protects paths with spaces.
        Command::new("cmd").args(["/C", "start", ""]).arg(path).creation_flags(0x0800_0000).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    Command::new("open").arg(path).spawn().map_err(|e| e.to_string())?;
    #[cfg(all(unix, not(target_os = "macos")))]
    Command::new("xdg-open").arg(path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Shows the file selected in its folder (or opens the folder when the file is gone).
pub fn reveal(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        if path.exists() {
            Command::new("explorer").arg(format!("/select,{}", path.display())).spawn().map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    open_path(path.parent().unwrap_or(path))
}

/// Starts Flux in the tray when the user signs in (Windows Run key of the current user).
pub fn set_autostart(enabled: bool) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let result = if enabled {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            Command::new("reg").args(["add", key, "/v", "Flux", "/t", "REG_SZ", "/d", &format!("\"{exe}\" --background"), "/f"]).creation_flags(0x0800_0000).status()
        } else {
            Command::new("reg").args(["delete", key, "/v", "Flux", "/f"]).creation_flags(0x0800_0000).status()
        };
        let _ = result;
    }
    #[cfg(not(windows))]
    let _ = enabled;
}

pub fn power(action: AfterQueue) -> Result<(), String> {
    #[cfg(windows)]
    {
        let args: &[&str] = match action {
            AfterQueue::Shutdown => &["/s", "/t", "0"],
            AfterQueue::Hibernate => &["/h"],
            AfterQueue::Sleep => {
                Command::new("rundll32.exe").args(["powrprof.dll,SetSuspendState", "0,1,0"]).spawn().map_err(|e| e.to_string())?;
                return Ok(());
            }
            AfterQueue::Nothing | AfterQueue::Exit => return Ok(()),
        };
        Command::new("shutdown").args(args).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    let _ = action;
    Ok(())
}

pub fn sound(success: bool) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONASTERISK, MB_ICONHAND};
        MessageBeep(if success { MB_ICONASTERISK } else { MB_ICONHAND });
    }
    #[cfg(not(windows))]
    let _ = success;
}

/// (weekday 0 = Sunday, "HH:MM") in local time.
pub fn local_time() -> (u8, String) {
    let now = Local::now();
    (now.weekday().num_days_from_sunday() as u8, format!("{:02}:{:02}", now.hour(), now.minute()))
}
