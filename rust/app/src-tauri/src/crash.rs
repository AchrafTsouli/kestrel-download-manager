//! Crash reports, opt-in each time. When Flux stops on an internal error it writes what happened
//! (version, system, the error and where it happened: no file names, addresses or settings) to
//! `<data>/crashes`. On the next start the main window offers to open it as a GitHub issue, which
//! the user reads and sends from the browser, or to dismiss it.

use std::path::{Path, PathBuf};

use serde::Serialize;

fn folder() -> PathBuf {
    flux_store::default_directory().join("crashes")
}

/// Records internal errors (Rust panics) before the default handling runs.
pub fn install(version: String) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into());
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let report = format!(
            "Flux {version}\nSystem: {} {}\nThread: {thread}\nWhere: {place}\nError: {}\n",
            std::env::consts::OS,
            std::env::consts::ARCH,
            // Errors can quote a path or an address; keep only the first line and cut it short.
            message.lines().next().unwrap_or("").chars().take(300).collect::<String>()
        );
        let dir = folder();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(format!("crash-{stamp}.txt")), report));
        previous(info);
    }));
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub id: String,
    pub text: String,
}

fn valid_id(id: &str) -> bool {
    id.starts_with("crash-") && id.ends_with(".txt") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

/// Reports not yet sent or dismissed (newest first, at most 5).
pub fn pending() -> Vec<Report> {
    let Ok(entries) = std::fs::read_dir(folder()) else { return Vec::new() };
    let mut reports: Vec<Report> = entries
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            valid_id(&id).then(|| Report { text: std::fs::read_to_string(e.path()).unwrap_or_default(), id })
        })
        .collect();
    reports.sort_by(|a, b| b.id.cmp(&a.id));
    reports.truncate(5);
    reports
}

/// Marks a report as handled (kept as `.seen` for the user's own reference).
pub fn dismiss(id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("Unknown report.".into());
    }
    let path: &Path = &folder().join(id);
    std::fs::rename(path, path.with_extension("seen")).map_err(|e| e.to_string())
}

fn encode(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// A new GitHub issue filled in with the report, for the user to read and send.
pub fn issue_url(id: &str) -> Result<String, String> {
    if !valid_id(id) {
        return Err("Unknown report.".into());
    }
    let text = std::fs::read_to_string(folder().join(id)).map_err(|e| e.to_string())?;
    let error = text.lines().find_map(|l| l.strip_prefix("Error: ")).unwrap_or("crash");
    let title = format!("Crash: {}", error.chars().take(80).collect::<String>());
    let body = format!("Flux closed unexpectedly.\n\nWhat I was doing:\n\n\n---\n```\n{text}```\n");
    Ok(format!("https://github.com/AchrafTsouli/flux-downloader/issues/new?title={}&body={}", encode(&title), encode(&body)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn report_ids_are_checked_and_urls_encoded() {
        assert!(super::valid_id("crash-1762000000.txt"));
        assert!(!super::valid_id("../crash-1.txt") && !super::valid_id("crash-1.exe"));
        assert_eq!(super::encode("a b&c/é"), "a%20b%26c%2F%C3%A9");
    }
}
