//! Operating-system actions: open and reveal files, start with the system, sounds, power actions,
//! local time. Windows, Linux and macOS.

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
    if path.exists() {
        Command::new("explorer").arg(format!("/select,{}", path.display())).spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if path.exists() {
        Command::new("open").arg("-R").arg(path).spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }
    open_path(path.parent().unwrap_or(path))
}

#[cfg(unix)]
fn home() -> std::path::PathBuf {
    std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default()
}

/// The Run value's name: the one Flux 1.x and its installer use, so there is only ever one.
#[cfg(windows)]
const RUN_VALUE: &str = "ApexDownloader";

/// Starts Flux in the tray when the user signs in (Windows Run key of the current user).
pub fn set_autostart(enabled: bool) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let result = if enabled {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            Command::new("reg").args(["add", key, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &format!("\"{exe}\" --background"), "/f"]).creation_flags(0x0800_0000).status()
        } else {
            Command::new("reg").args(["delete", key, "/v", RUN_VALUE, "/f"]).creation_flags(0x0800_0000).status()
        };
        let _ = result;
    }
    #[cfg(target_os = "macos")]
    {
        let plist = home().join("Library/LaunchAgents/com.achraftsouli.flux.plist");
        if enabled {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let text = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.achraftsouli.flux</string>
<key>ProgramArguments</key><array><string>{exe}</string><string>--background</string></array>
<key>RunAtLoad</key><true/>
</dict></plist>
"#);
            let _ = std::fs::create_dir_all(plist.parent().unwrap()).and_then(|_| std::fs::write(&plist, text));
        } else {
            let _ = std::fs::remove_file(plist);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home().join(".config"));
        let entry = config.join("autostart/flux.desktop");
        if enabled {
            let exe = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from).or_else(|| std::env::current_exe().ok()).map(|p| p.display().to_string()).unwrap_or_default();
            let text = format!("[Desktop Entry]\nType=Application\nName=Kestrel\nExec=\"{exe}\" --background\nX-GNOME-Autostart-enabled=true\n");
            let _ = std::fs::create_dir_all(entry.parent().unwrap()).and_then(|_| std::fs::write(&entry, text));
        } else {
            let _ = std::fs::remove_file(entry);
        }
    }
}

/// Keeps the browser extension connected: when a browser has no Flux bridge registered, or the
/// registration points to a file that is gone (an uninstalled Flux 1.x), this Flux's bridge is
/// registered. A working registration of another copy is left alone.
pub fn ensure_browser_bridge() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let registered = |browser: &str| -> Option<std::path::PathBuf> {
            let key = format!(r"HKCU\Software\{browser}\NativeMessagingHosts\com.swiftfetch.bridge");
            let out = Command::new("reg").args(["query", &key, "/ve"]).creation_flags(0x0800_0000).output().ok()?;
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            let value = text.lines().find(|l| l.contains("REG_SZ"))?.split("REG_SZ").nth(1)?.trim().to_string();
            Some(value.into())
        };
        let browsers = [r"Google\Chrome", r"Microsoft\Edge", r"BraveSoftware\Brave-Browser"];
        if browsers.iter().all(|b| registered(b).is_some_and(|manifest| manifest.is_file())) {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let host = exe.with_file_name("flux-nmhost.exe");
        if host.is_file() {
            let _ = Command::new(host).arg("--register").creation_flags(0x0800_0000).status();
        }
    }
}

/// Holds the mutex the installer checks (`AppMutex`), so it asks to close a running Flux instead
/// of replacing files in use. Same name as Flux 1.x.
pub fn hold_running_mutex() {
    #[cfg(windows)]
    unsafe {
        let name: Vec<u16> = "Local\\ApexDownloader.Running".encode_utf16().chain([0]).collect();
        // Kept for the life of the process; Windows releases it on exit.
        let _ = windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, name.as_ptr());
    }
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
    #[cfg(target_os = "macos")]
    {
        let command = match action {
            AfterQueue::Shutdown => vec!["osascript", "-e", "tell application \"System Events\" to shut down"],
            AfterQueue::Sleep | AfterQueue::Hibernate => vec!["pmset", "sleepnow"],
            AfterQueue::Nothing | AfterQueue::Exit => return Ok(()),
        };
        Command::new(command[0]).args(&command[1..]).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let verb = match action {
            AfterQueue::Shutdown => "poweroff",
            AfterQueue::Sleep => "suspend",
            AfterQueue::Hibernate => "hibernate",
            AfterQueue::Nothing | AfterQueue::Exit => return Ok(()),
        };
        Command::new("systemctl").arg(verb).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn sound(success: bool) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONASTERISK, MB_ICONHAND};
        MessageBeep(if success { MB_ICONASTERISK } else { MB_ICONHAND });
    }
    #[cfg(target_os = "macos")]
    let _ = Command::new("afplay").arg(if success { "/System/Library/Sounds/Glass.aiff" } else { "/System/Library/Sounds/Basso.aiff" }).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("canberra-gtk-play").args(["-i", if success { "complete" } else { "dialog-error" }]).spawn();
}

/// (weekday 0 = Sunday, "HH:MM") in local time.
pub fn local_time() -> (u8, String) {
    let now = Local::now();
    (now.weekday().num_days_from_sunday() as u8, format!("{:02}:{:02}", now.hour(), now.minute()))
}
