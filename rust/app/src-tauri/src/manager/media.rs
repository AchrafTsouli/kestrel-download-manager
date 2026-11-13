//! Torrents (librqbit, inside Kestrel) and videos (yt-dlp, FFmpeg).

use super::*;

impl Manager {

    pub(super) fn run_torrent(self: &Arc<Self>, record: Record, selection: kestrel_torrent::Selection, cancel: CancellationToken) {
        let global = self.settings.read().speed_limit;
        let limit = match (record.speed_limit, global) {
            (0, global) => global,
            (own, 0) => own,
            (own, global) => own.min(global),
        };
        if let Some(live) = self.live.lock().get_mut(&record.id) {
            live.info.note = Some("Finding people who share this torrent…".into());
        }
        self.emit(&record.id);
        let manager = self.clone();
        let id = record.id.clone();
        let folder = record.destination.parent().unwrap_or(Path::new("")).to_path_buf();
        tauri::async_runtime::spawn(async move {
            let events = {
                let manager = manager.clone();
                let id = id.clone();
                let selection = selection.clone();
                move |status: kestrel_torrent::Status| manager.on_torrent_status(&id, &selection, status)
            };
            let result = match manager.ensure_vpn().await {
                Err(error) => Err(error),
                Ok(()) => manager.torrents.download(&selection, &folder, limit, events, cancel).await,
            };
            let result = match result {
                Ok(kestrel_torrent::Outcome::Completed) => manager.scan(&folder.join(&selection.name)).await.map(|_| kestrel_torrent::Outcome::Completed),
                other => other,
            };
            manager.live.lock().remove(&id);
            let completed = matches!(result, Ok(kestrel_torrent::Outcome::Completed));
            let error = result.as_ref().err().cloned();
            manager.update(&id, |r| match &result {
                Ok(kestrel_torrent::Outcome::Completed) => {
                    r.status = Status::Completed;
                    r.written = r.total.unwrap_or(r.written);
                    r.error = None;
                    r.completed_ms = Some(now_ms());
                }
                Ok(kestrel_torrent::Outcome::Paused) => r.status = Status::Paused,
                Err(message) => {
                    r.status = Status::Failed;
                    r.error = Some(message.clone());
                }
            });
            manager.finished(&id, completed, error);
        });
    }

    pub(super) fn on_torrent_status(&self, id: &str, selection: &kestrel_torrent::Selection, status: kestrel_torrent::Status) {
        let seeding = status.state == "seeding";
        let total = if selection.selected_bytes > 0 { selection.selected_bytes } else { status.total };
        self.update(id, |r| {
            if total > 0 { r.total = Some(total); }
            r.written = if total > 0 { status.completed.min(total) } else { status.completed };
            r.status = if seeding { Status::Finishing } else if status.connections > 0 || status.completed > 0 { Status::Downloading } else { Status::Connecting };
        });
        if let Some(live) = self.live.lock().get_mut(id) {
            live.info.speed = status.bytes_per_second;
            live.info.connections = status.connections as usize;
            live.info.note = Some(if seeding {
                format!("Sharing with others for up to {} min", selection.seed_minutes)
            } else if status.connections > 0 {
                format!("{} peers · {} seeders", status.connections, status.seeders)
            } else {
                "Finding people who share this torrent…".into()
            });
            live.info.phase = seeding.then(|| "Sharing the finished files".into());
            live.info.phase_fraction = None;
        }
        self.changed(id);
    }

    // ------------------------------------------------------------------ videos

    /// Where a video download keeps its parts between pauses.
    pub(super) fn video_work(record: &Record) -> PathBuf {
        let parent = record.destination.parent().unwrap_or(Path::new(""));
        let work = parent.join(format!(".kestrel-video-{}", record.id));
        // From before the rename.
        let old = parent.join(format!(".flux-video-{}", record.id));
        if !work.exists() && old.is_dir() {
            let _ = std::fs::rename(&old, &work);
        }
        work
    }

    pub(super) fn run_video(self: &Arc<Self>, record: Record, selection: kestrel_media::Selection, cancel: CancellationToken) {
        let settings = self.settings.read().clone();
        let limit = match (record.speed_limit, settings.speed_limit) {
            (0, global) => global,
            (own, 0) => own,
            (own, global) => own.min(global),
        };
        let mut job = kestrel_media::VideoJob {
            page: record.url.clone(),
            destination: record.destination.clone(),
            work: Manager::video_work(&record),
            selection,
            session: record.session.clone(),
            connections: record.connections,
            retries: settings.max_retries,
            speed_limit: limit,
            expected_sha256: record.expected_sha256.clone(),
        };
        self.emit(&record.id);
        let manager = self.clone();
        let id = record.id.clone();
        tauri::async_runtime::spawn(async move {
            let events = {
                let manager = manager.clone();
                let id = id.clone();
                move |event: kestrel_media::MediaEvent| manager.on_video_event(&id, event)
            };
            let result = match manager.ensure_vpn().await {
                Err(error) => Err(error),
                Ok(()) => manager.media.download(&mut job, events, cancel).await,
            };
            let result = match result {
                Ok(kestrel_media::Outcome::Completed { path, bytes, sha256 }) => manager.scan(&path).await.map(|_| kestrel_media::Outcome::Completed { path, bytes, sha256 }),
                other => other,
            };
            manager.live.lock().remove(&id);
            let mut completed = false;
            let error = result.as_ref().err().cloned();
            manager.update(&id, |r| {
                r.kind = Kind::Video { selection: job.selection.clone() };
                match &result {
                    Ok(kestrel_media::Outcome::Completed { path, bytes, sha256 }) => {
                        r.destination = path.clone();
                        r.status = Status::Completed;
                        r.written = *bytes;
                        r.total = Some(*bytes);
                        r.sha256 = sha256.clone();
                        r.error = None;
                        r.completed_ms = Some(now_ms());
                        completed = true;
                    }
                    Ok(kestrel_media::Outcome::Paused) => r.status = Status::Paused,
                    Err(message) => {
                        r.status = Status::Failed;
                        r.error = Some(message.clone());
                    }
                }
            });
            manager.finished(&id, completed, error);
        });
    }

    pub(super) fn on_video_event(&self, id: &str, event: kestrel_media::MediaEvent) {
        use kestrel_media::MediaEvent;
        match event {
            MediaEvent::Tracks(_) => {}
            MediaEvent::Progress { written, total, estimated, bytes_per_second } => {
                self.modify(id, |r| {
                    r.written = written;
                    r.total = total.or(r.total);
                    r.status = Status::Downloading;
                });
                if let Some(live) = self.live.lock().get_mut(id) {
                    live.info.speed = bytes_per_second;
                    live.info.connections = 1;
                    live.info.estimated = estimated;
                    live.info.phase = None;
                    live.info.phase_fraction = None;
                }
            }
            MediaEvent::Phase { text, fraction } => {
                self.modify(id, |r| r.status = Status::Finishing);
                if let Some(live) = self.live.lock().get_mut(id) {
                    live.info.speed = 0.0;
                    live.info.phase = Some(text);
                    live.info.phase_fraction = fraction;
                }
            }
            MediaEvent::Impersonating => {
                if let Some(live) = self.live.lock().get_mut(id) {
                    live.info.note = Some("The site refused ordinary requests; retrying as Chrome.".into());
                }
            }
        }
        self.changed(id);
    }
}

pub(super) fn program(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// The video tools: bundled beside Kestrel when installed (Linux packages: `../lib/kestrel`); in a
/// development build, the repository's copy or the one Flux 1.x installed; on Linux and macOS,
/// otherwise the system's own yt-dlp and FFmpeg.
pub(super) fn video_tools_directory() -> PathBuf {
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
    let mut candidates = vec![exe.join("video-tools"), exe.join("../lib/kestrel/video-tools"), exe.join("../Resources/video-tools"), exe.join("../../../video-tools")];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Programs").join("Apex Downloader").join("video-tools"));
    }
    if !cfg!(windows) {
        candidates.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
        candidates.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    }
    let complete = |dir: &PathBuf| ["yt-dlp", "ffmpeg", "ffprobe"].iter().all(|n| program(dir, n).exists());
    candidates.iter().find(|d| complete(d)).cloned().unwrap_or_else(|| candidates[0].clone())
}
