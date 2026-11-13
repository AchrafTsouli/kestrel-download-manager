//! Adding the extension by hand ("Load unpacked"): which browsers are installed, opening a
//! browser on its extensions page, and where the extension folder is.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Browser {
    pub id: &'static str,
    pub name: &'static str,
    /// The page to open: "chrome://extensions"…
    pub page: &'static str,
}

/// Each browser: id, name, extensions page, where its program usually is (under these folders).
type Known = (&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]);
const KNOWN: [Known; 6] = [
    ("chrome", "Google Chrome", "chrome://extensions", &[("ProgramFiles", r"Google\Chrome\Application\chrome.exe"), ("ProgramFiles(x86)", r"Google\Chrome\Application\chrome.exe"), ("LOCALAPPDATA", r"Google\Chrome\Application\chrome.exe")]),
    ("edge", "Microsoft Edge", "edge://extensions", &[("ProgramFiles(x86)", r"Microsoft\Edge\Application\msedge.exe"), ("ProgramFiles", r"Microsoft\Edge\Application\msedge.exe")]),
    ("brave", "Brave", "brave://extensions", &[("ProgramFiles", r"BraveSoftware\Brave-Browser\Application\brave.exe"), ("LOCALAPPDATA", r"BraveSoftware\Brave-Browser\Application\brave.exe")]),
    ("opera", "Opera", "opera://extensions", &[("LOCALAPPDATA", r"Programs\Opera\opera.exe"), ("LOCALAPPDATA", r"Programs\Opera GX\opera.exe")]),
    ("vivaldi", "Vivaldi", "vivaldi://extensions", &[("LOCALAPPDATA", r"Vivaldi\Application\vivaldi.exe"), ("ProgramFiles", r"Vivaldi\Application\vivaldi.exe")]),
    // Firefox loads an unsigned add-on as a temporary one, from about:debugging.
    ("firefox", "Mozilla Firefox", "about:debugging#/runtime/this-firefox", &[("ProgramFiles", r"Mozilla Firefox\firefox.exe"), ("ProgramFiles(x86)", r"Mozilla Firefox\firefox.exe"), ("LOCALAPPDATA", r"Mozilla Firefox\firefox.exe")]),
];

fn program(places: &[(&str, &str)]) -> Option<PathBuf> {
    places.iter().filter_map(|(var, rest)| std::env::var_os(var).map(|base| PathBuf::from(base).join(rest))).find(|p| p.is_file())
}

/// The browsers found on this computer, in a fixed order.
pub fn installed() -> Vec<Browser> {
    KNOWN.iter().filter(|(_, _, _, places)| program(places).is_some()).map(|(id, name, page, _)| Browser { id, name, page }).collect()
}

/// Opens `id`'s extensions page (in the browser's open window when it is running).
pub fn open_extensions_page(id: &str) -> Result<(), String> {
    let (_, name, page, places) = KNOWN.iter().find(|(known, ..)| *known == id).ok_or("Unknown browser.")?;
    let exe = program(places).ok_or_else(|| format!("{name} was not found on this computer."))?;
    std::process::Command::new(exe).arg(page).spawn().map(|_| ()).map_err(|e| format!("{name} could not be opened: {e}"))
}

/// The extension folder to choose in "Load unpacked": beside Kestrel when installed; in a
/// development build, the repository's BrowserExtension folder.
pub fn extension_folder() -> Option<PathBuf> {
    folder_named("BrowserExtension")
}

/// Firefox's build of the extension (its own background script and add-on ID), made by the installer.
pub fn firefox_extension_folder() -> Option<PathBuf> {
    folder_named("BrowserExtension-Firefox")
}

fn folder_named(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().skip(1).map(|dir| dir.join(name)).find(|dir| dir.join("manifest.json").is_file())
}
