//! Kestrel's log: `<data>/logs/kestrel.log`, what happened and when, for bug reports (Help > Open log).
//! It never holds web addresses or file paths: they are replaced before writing, so the log keeps
//! the privacy promise the encrypted history makes. At 1 MB it rotates; three files are kept.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;

use parking_lot::Mutex;
use regex::Regex;

const LIMIT: u64 = 1 << 20;
const KEPT: usize = 3;

struct Log {
    file: Option<File>,
    size: u64,
}

static LOG: Mutex<Log> = Mutex::new(Log { file: None, size: 0 });

pub fn folder() -> PathBuf {
    kestrel_store::default_directory().join("logs")
}

pub fn path() -> PathBuf {
    folder().join("kestrel.log")
}

fn rotated(n: usize) -> PathBuf {
    folder().join(format!("kestrel.{n}.log"))
}

/// Replaces web addresses, Windows and Unix paths, so a log can be shared as it is.
pub fn redact(text: &str) -> String {
    static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\b(?:https?|ftp|magnet):\S+"#).expect("pattern"));
    static WINDOWS_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)(?:\b[a-z]:|\\\\)[\\/][^"'\r\n]*"#).expect("pattern"));
    static UNIX_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:^|\s)(/(?:home|Users|tmp|var|mnt|media)/[^"'\r\n]*)"#).expect("pattern"));
    let text = URL.replace_all(text, "<address>");
    let text = WINDOWS_PATH.replace_all(&text, "<path>");
    UNIX_PATH.replace_all(&text, " <path>").into_owned()
}

fn open(log: &mut Log) {
    let _ = std::fs::create_dir_all(folder());
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(path()) {
        log.size = file.metadata().map(|m| m.len()).unwrap_or(0);
        log.file = Some(file);
    }
}

fn rotate(log: &mut Log) {
    log.file = None;
    for n in (1..KEPT).rev() {
        let from = if n == 1 { path() } else { rotated(n - 1) };
        let _ = std::fs::rename(from, rotated(n));
    }
    open(log);
}

fn write(level: &str, message: &str) {
    let line = format!("{} {level} {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), redact(message).replace('\n', " | "));
    let mut log = LOG.lock();
    if log.file.is_none() {
        open(&mut log);
    }
    if log.size + line.len() as u64 > LIMIT {
        rotate(&mut log);
    }
    if let Some(file) = log.file.as_mut()
        && file.write_all(line.as_bytes()).is_ok() {
            log.size += line.len() as u64;
        }
}

pub fn info(message: impl AsRef<str>) {
    write("INFO ", message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    write("WARN ", message.as_ref());
}

pub fn error(message: impl AsRef<str>) {
    write("ERROR", message.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_paths_never_reach_the_log() {
        let text = redact(r#"HTTP 404 for https://example.com/a?token=1 saved to C:\Users\me\Downloads\x.zip and magnet:?xt=urn:btih:abc, cache \\server\share\f and /home/me/file"#);
        assert_eq!(text, "HTTP 404 for <address> saved to <path>");
        let text = redact("Retrying (HTTP 503): server busy; file D:/Videos/clip.mp4");
        assert_eq!(text, "Retrying (HTTP 503): server busy; file <path>");
        assert_eq!(redact("Connection reset by peer"), "Connection reset by peer");
    }
}
