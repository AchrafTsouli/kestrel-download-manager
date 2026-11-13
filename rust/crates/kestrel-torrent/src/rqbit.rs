//! Torrents inside Kestrel with librqbit (no separate aria2 program): file lists for magnet links,
//! downloads of the chosen files, pause and resume (the files already there are checked and kept),
//! a speed limit, and seeding for the minutes the user chose (none by default).

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Session, SessionOptions};
use tokio_util::sync::CancellationToken;

use crate::{Info, PUBLIC_TRACKERS, Selection, magnet_hash, metadata_path, parse};

/// What a running torrent reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub completed: u64,
    pub total: u64,
    pub bytes_per_second: f64,
    pub connections: u32,
    pub seeders: u32,
    /// "active", "paused", "complete", "error" or "seeding".
    pub state: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Completed,
    Paused,
}

pub struct Tools {
    /// Saved metadata, one `<hash>.torrent` per torrent.
    pub cache: PathBuf,
    /// A SOCKS5 proxy for peers and trackers.
    proxy: parking_lot::Mutex<Option<String>>,
    session: tokio::sync::Mutex<Option<Arc<Session>>>,
}

fn error(e: impl std::fmt::Display) -> String {
    let text = e.to_string();
    text.lines().next().unwrap_or("").chars().take(300).collect()
}

impl Tools {
    pub fn new(cache: impl Into<PathBuf>) -> Tools {
        Tools { cache: cache.into(), proxy: parking_lot::Mutex::new(None), session: tokio::sync::Mutex::new(None) }
    }

    /// Only SOCKS5 proxies carry peer connections; others are not used for torrents.
    pub fn set_proxy(&self, proxy: Option<String>) {
        *self.proxy.lock() = proxy.filter(|p| p.starts_with("socks5://"));
    }

    /// Built in: always there.
    pub fn available(&self) -> bool {
        true
    }

    /// One session for every torrent, started on first use.
    async fn session(&self) -> Result<Arc<Session>, String> {
        let mut slot = self.session.lock().await;
        if let Some(session) = slot.as_ref() {
            return Ok(session.clone());
        }
        std::fs::create_dir_all(&self.cache).map_err(error)?;
        let trackers = PUBLIC_TRACKERS.iter().filter_map(|t| url::Url::parse(t).ok()).collect();
        let options = SessionOptions {
            trackers,
            connect: Some(librqbit::ConnectionOptions { proxy_url: self.proxy.lock().clone(), ..Default::default() }),
            ..Default::default()
        };
        let session = Session::new_with_opts(self.cache.join("downloads"), options).await.map_err(|e| format!("The torrent engine could not start: {}", error(e)))?;
        *slot = Some(session.clone());
        Ok(session)
    }

    /// Reads a magnet link (fetching its file list from peers) or a .torrent file, and keeps its metadata.
    pub async fn load(&self, source: &str, cancel: &CancellationToken) -> Result<Info, String> {
        std::fs::create_dir_all(&self.cache).map_err(error)?;
        let expected = magnet_hash(source);
        let file = match &expected {
            Some(hash) => {
                let file = metadata_path(&self.cache, hash);
                if !file.exists() {
                    let session = self.session().await?;
                    let options = AddTorrentOptions { list_only: true, ..Default::default() };
                    let add = session.add_torrent(AddTorrent::from_url(source), Some(options));
                    let response = tokio::select! {
                        r = tokio::time::timeout(Duration::from_secs(180), add) => r.map_err(|_| "No one shared this torrent's file list within 3 minutes. Try again later, or use a .torrent file.".to_string())?,
                        _ = cancel.cancelled() => return Err(String::new()),
                    };
                    match response.map_err(|e| format!("The torrent's file list could not be read: {}", error(e)))? {
                        AddTorrentResponse::ListOnly(list) => std::fs::write(&file, &list.torrent_bytes).map_err(error)?,
                        _ => return Err("The torrent's file list could not be read.".into()),
                    }
                }
                file
            }
            None => {
                let data = std::fs::read(source).map_err(|e| format!("The torrent file could not be read: {e}"))?;
                let info = parse(&data)?;
                let file = metadata_path(&self.cache, &info.info_hash);
                if Path::new(source) != file {
                    std::fs::write(&file, &data).map_err(error)?;
                }
                file
            }
        };
        let info = parse(&std::fs::read(&file).map_err(error)?)?;
        if expected.is_some_and(|hash| hash != info.info_hash) {
            let _ = std::fs::remove_file(&file);
            return Err("The file list received does not belong to this magnet link.".into());
        }
        Ok(info)
    }

    /// Downloads the chosen files into `folder` (the torrent's own folder or file is created there).
    pub async fn download(&self, selection: &Selection, folder: &Path, speed_limit: u64, events: impl Fn(Status), cancel: CancellationToken) -> Result<Outcome, String> {
        let metadata = std::fs::read(&selection.metadata_file).map_err(|_| "The torrent's saved file list is missing. Add the torrent again.".to_string())?;
        std::fs::create_dir_all(folder).map_err(|e| format!("The folder could not be created: {e}"))?;
        // A torrent of several files gets its own folder, named after it (one file goes straight in).
        let info = parse(&metadata)?;
        let target = if info.files.len() > 1 { folder.join(&info.name) } else { folder.to_path_buf() };
        // Unchosen files are created too (pieces cross file edges): when the torrent completes, the
        // incomplete ones are removed, as aria2 did. One complete from an earlier download stays.
        let unchosen: Vec<(usize, u64, PathBuf)> = info.files.iter()
            .filter(|f| !selection.files.is_empty() && !selection.files.contains(&f.index))
            .map(|f| (f.index - 1, f.length, if info.files.len() > 1 { target.join(&f.path) } else { folder.join(&f.path) }))
            .collect();
        let session = self.session().await?;
        let limit = NonZeroU32::new(speed_limit.min(u32::MAX as u64) as u32);
        // Saved selections count files from 1 (as before Kestrel 2.1); librqbit counts from 0.
        let only: Vec<usize> = selection.files.iter().filter(|&&i| i > 0).map(|i| i - 1).collect();
        let options = AddTorrentOptions {
            only_files: (!only.is_empty()).then(|| only.clone()),
            output_folder: Some(target.display().to_string()),
            // Files already there (a paused download) are checked and continued, not refused.
            overwrite: true,
            trackers: Some(PUBLIC_TRACKERS.iter().map(|t| t.to_string()).collect()),
            ratelimits: librqbit::limits::LimitsConfig { download_bps: limit, upload_bps: None },
            ..Default::default()
        };
        let handle = match session.add_torrent(AddTorrent::from_bytes(metadata), Some(options)).await.map_err(|e| format!("The torrent could not start: {}", error(e)))? {
            AddTorrentResponse::Added(_, handle) | AddTorrentResponse::AlreadyManaged(_, handle) => handle,
            AddTorrentResponse::ListOnly(_) => return Err("The torrent could not start.".into()),
        };
        if handle.is_paused() {
            session.unpause(&handle).await.map_err(error)?;
        }
        if !only.is_empty() {
            let wanted: HashSet<usize> = only.iter().copied().collect();
            let _ = session.update_only_files(&handle, &wanted).await;
        }
        let id = handle.id();
        let stop = |session: Arc<Session>| async move {
            let _ = session.delete(id.into(), false).await;
        };
        let mut seeding_since: Option<Instant> = None;
        loop {
            let mut stats = handle.stats();
            // Still checking or connecting: "finished" only counts once the size is known.
            stats.finished = stats.finished && stats.total_bytes > 0 && matches!(stats.state, librqbit::TorrentStatsState::Live | librqbit::TorrentStatsState::Paused);
            let live = stats.live.as_ref();
            let state = if let Some(e) = &stats.error { events(Status { completed: stats.progress_bytes, total: stats.total_bytes, bytes_per_second: 0.0, connections: 0, seeders: 0, state: "error".into(), error: Some(e.clone()) }); "error" } else if stats.finished { if selection.seed_minutes > 0 { "seeding" } else { "complete" } } else { "active" };
            if state == "error" {
                stop(session.clone()).await;
                return Err(format!("The torrent stopped: {}", stats.error.clone().unwrap_or_default()));
            }
            events(Status {
                completed: stats.progress_bytes,
                total: stats.total_bytes,
                bytes_per_second: live.map_or(0.0, |l| l.download_speed.as_bytes() as f64),
                connections: live.map_or(0, |l| l.snapshot.peer_stats.live),
                seeders: live.map_or(0, |l| l.snapshot.peer_stats.live_seeders),
                state: state.into(),
                error: None,
            });
            if stats.finished {
                let since = *seeding_since.get_or_insert_with(Instant::now);
                if selection.seed_minutes == 0 || since.elapsed() >= Duration::from_secs(selection.seed_minutes as u64 * 60) {
                    let have = stats.file_progress.clone();
                    stop(session.clone()).await;
                    for (index, length, path) in &unchosen {
                        if have.get(*index).is_none_or(|got| got < length) {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                    return Ok(Outcome::Completed);
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                _ = cancel.cancelled() => {
                    // Stopped and forgotten by the session; the files stay and are checked on resume.
                    stop(session.clone()).await;
                    return Ok(Outcome::Paused);
                }
            }
        }
    }
}
