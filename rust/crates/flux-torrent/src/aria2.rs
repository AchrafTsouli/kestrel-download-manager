//! aria2 as a child process: it answers only on this computer (127.0.0.1) with a random secret
//! for each start, and stops together with Flux (--stop-with-process). Its .aria2 control files
//! make a pause continue where it stopped.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{Info, PUBLIC_TRACKERS, Selection, magnet_hash, metadata_path, parse};

/// What aria2 reports about a running torrent.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub completed: u64,
    pub total: u64,
    pub bytes_per_second: f64,
    pub connections: u32,
    pub seeders: u32,
    /// "active", "waiting", "paused", "complete", "error", "removed" or "seeding".
    pub state: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Completed,
    Paused,
}

pub struct Tools {
    pub aria2c: PathBuf,
    /// Saved metadata, one `<hash>.torrent` per torrent.
    pub cache: PathBuf,
    /// An HTTP proxy for trackers and web seeds (aria2 cannot send peer traffic through a proxy).
    proxy: std::sync::Mutex<Option<String>>,
}

fn common() -> Vec<String> {
    vec![
        "--enable-dht=true".into(),
        "--bt-enable-lpd=true".into(),
        "--enable-peer-exchange=true".into(),
        format!("--bt-tracker={}", PUBLIC_TRACKERS.join(",")),
        "--bt-tracker-connect-timeout=10".into(),
        "--bt-tracker-timeout=20".into(),
        "--follow-torrent=false".into(),
        "--summary-interval=0".into(),
        "--console-log-level=warn".into(),
        format!("--stop-with-process={}", std::process::id()),
        "--file-allocation=none".into(),
        "--auto-file-renaming=false".into(),
    ]
}

fn last_line(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).next_back().unwrap_or_default().chars().take(400).collect()
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::getrandom(&mut buffer).expect("system random numbers");
    buffer.iter().map(|b| format!("{b:02x}")).collect()
}

impl Tools {
    pub fn new(aria2c: impl Into<PathBuf>, cache: impl Into<PathBuf>) -> Tools {
        Tools { aria2c: aria2c.into(), cache: cache.into(), proxy: std::sync::Mutex::new(None) }
    }

    /// Only HTTP proxies: aria2 has no SOCKS support.
    pub fn set_proxy(&self, proxy: Option<String>) {
        *self.proxy.lock().unwrap() = proxy.filter(|p| p.starts_with("http://") || p.starts_with("https://"));
    }

    fn with_proxy(&self, mut args: Vec<String>) -> Vec<String> {
        if let Some(proxy) = self.proxy.lock().unwrap().clone() {
            args.push(format!("--all-proxy={proxy}"));
        }
        args
    }

    pub fn available(&self) -> bool {
        self.aria2c.exists()
    }

    fn command(&self, args: &[String]) -> Result<tokio::process::Child, String> {
        if !self.available() {
            return Err("The torrent engine (aria2c.exe) is missing. Reinstall Flux to restore it.".into());
        }
        let mut command = Command::new(&self.aria2c);
        command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        command.spawn().map_err(|e| format!("The torrent engine could not start: {e}"))
    }

    /// Reads a magnet link (fetching its file list from peers) or a .torrent file, and keeps its metadata.
    pub async fn load(&self, source: &str, cancel: &CancellationToken) -> Result<Info, String> {
        std::fs::create_dir_all(&self.cache).map_err(|e| e.to_string())?;
        let expected = magnet_hash(source);
        let file = match &expected {
            Some(hash) => {
                let file = metadata_path(&self.cache, hash);
                if !file.exists() {
                    let scratch = self.cache.join(format!("fetch-{}", random_hex(8)));
                    let fetched = self.fetch_metadata(source, hash, &scratch, cancel).await;
                    let copied = fetched.and_then(|f| std::fs::copy(f, &file).map_err(|e| e.to_string()));
                    let _ = std::fs::remove_dir_all(&scratch);
                    copied?;
                }
                file
            }
            None => {
                let data = std::fs::read(source).map_err(|e| format!("The torrent file could not be read: {e}"))?;
                let info = parse(&data)?;
                let file = metadata_path(&self.cache, &info.info_hash);
                if Path::new(source) != file {
                    std::fs::write(&file, &data).map_err(|e| e.to_string())?;
                }
                file
            }
        };
        let info = parse(&std::fs::read(&file).map_err(|e| e.to_string())?)?;
        if expected.is_some_and(|hash| hash != info.info_hash) {
            let _ = std::fs::remove_file(&file);
            return Err("The file list received does not belong to this magnet link.".into());
        }
        Ok(info)
    }

    async fn fetch_metadata(&self, magnet: &str, hash: &str, folder: &Path, cancel: &CancellationToken) -> Result<PathBuf, String> {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let mut args = common();
        args.extend(["--bt-metadata-only=true".into(), "--bt-save-metadata=true".into(), "--seed-time=0".into(), format!("--dir={}", folder.display()), magnet.into()]);
        let mut child = self.command(&self.with_proxy(args))?;
        let mut stderr = child.stderr.take().unwrap();
        let errors = tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            text
        });
        tokio::select! {
            _ = cancel.cancelled() => { let _ = child.kill().await; return Err(String::new()); }
            _ = child.wait() => {}
        }
        let file = folder.join(format!("{hash}.torrent"));
        if !file.exists() {
            return Err(format!("No one shared this torrent's file list. Try again later, or use a .torrent file. {}", last_line(&errors.await.unwrap_or_default())).trim().to_string());
        }
        Ok(file)
    }

    /// Downloads the chosen files into `folder` (the torrent's own folder or file is created there).
    pub async fn download(&self, selection: &Selection, folder: &Path, speed_limit: u64, events: impl Fn(Status), cancel: CancellationToken) -> Result<Outcome, String> {
        if !selection.metadata_file.exists() {
            return Err("The torrent's saved file list is missing. Add the torrent again.".into());
        }
        std::fs::create_dir_all(folder).map_err(|e| format!("The folder could not be created: {e}"))?;
        let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_err(|e| e.to_string())?.port();
        let (secret, gid) = (random_hex(24), random_hex(8));
        let mut args = common();
        args.extend([
            "--enable-rpc=true".into(),
            "--rpc-listen-all=false".into(),
            format!("--rpc-listen-port={port}"),
            format!("--rpc-secret={secret}"),
            format!("--gid={gid}"),
            format!("--dir={}", folder.display()),
            "--continue=true".into(),
            "--check-integrity=false".into(),
            "--allow-overwrite=false".into(),
            "--auto-save-interval=10".into(),
            format!("--seed-time={}", selection.seed_minutes),
            "--bt-save-metadata=false".into(),
        ]);
        // Pieces span file boundaries, so edges of unchosen files arrive too; aria2 deletes them on completion.
        if !selection.files.is_empty() {
            let list: Vec<String> = selection.files.iter().map(usize::to_string).collect();
            args.extend([format!("--select-file={}", list.join(",")), "--bt-remove-unselected-file=true".into()]);
        }
        if speed_limit > 0 {
            args.push(format!("--max-overall-download-limit={speed_limit}"));
        }
        args.push(selection.metadata_file.display().to_string());
        let mut child = self.command(&self.with_proxy(args))?;
        let mut stderr = child.stderr.take().unwrap();
        let errors = tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            text
        });
        let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).no_proxy().build().map_err(|e| e.to_string())?;
        let endpoint = format!("http://127.0.0.1:{port}/jsonrpc");
        let call = |method: &'static str, params: Vec<Value>| {
            let mut all = vec![json!(format!("token:{secret}"))];
            all.extend(params);
            let request = client.post(&endpoint).json(&json!({ "jsonrpc": "2.0", "id": "flux", "method": method, "params": all }));
            async move { request.send().await.ok()?.json::<Value>().await.ok()?.get("result").cloned() }
        };

        let started = Instant::now();
        let mut last: Option<Status> = None;
        let result = loop {
            if cancel.is_cancelled() {
                break Ok(Outcome::Paused);
            }
            if let Ok(Some(_)) = child.try_wait() {
                break Err(format!("The torrent engine stopped: {}", last_line(&errors.await.unwrap_or_default())));
            }
            let fields = json!(["status", "totalLength", "completedLength", "downloadSpeed", "connections", "numSeeders", "errorMessage", "seeder"]);
            match call("aria2.tellStatus", vec![json!(gid), fields]).await {
                Some(r) => {
                    let number = |name: &str| r[name].as_str().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                    let state = r["status"].as_str().unwrap_or_default().to_string();
                    let seeding = r["seeder"] == "true" && state == "active";
                    let status = Status {
                        completed: number("completedLength"),
                        total: number("totalLength"),
                        bytes_per_second: number("downloadSpeed") as f64,
                        connections: number("connections") as u32,
                        seeders: number("numSeeders") as u32,
                        state: if seeding { "seeding".into() } else { state.clone() },
                        error: r["errorMessage"].as_str().filter(|e| !e.is_empty()).map(String::from),
                    };
                    events(status.clone());
                    last = Some(status);
                    if matches!(state.as_str(), "complete" | "error" | "removed") {
                        break Ok(Outcome::Completed);
                    }
                }
                None if started.elapsed() > Duration::from_secs(20) => break Err("The torrent engine did not answer.".into()),
                None => {}
            }
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        };
        // Ask aria2 to save its state and stop; force it after 10 seconds.
        let _ = call(if cancel.is_cancelled() { "aria2.forceShutdown" } else { "aria2.shutdown" }, vec![]).await;
        if tokio::time::timeout(Duration::from_secs(10), child.wait()).await.is_err() {
            let _ = child.kill().await;
        }
        match result? {
            Outcome::Paused => Ok(Outcome::Paused),
            Outcome::Completed => match last {
                Some(s) if s.state == "error" => Err(s.error.map_or("The torrent could not be downloaded.".into(), |e| format!("Torrent: {e}"))),
                Some(s) if s.state == "complete" || s.state == "seeding" => {
                    if folder.join(&selection.name).exists() { Ok(Outcome::Completed) } else { Err("The torrent finished but its files were not found.".into()) }
                }
                _ => Err("The torrent engine stopped before the download finished.".into()),
            },
        }
    }
}
