//! What the windows ask for: pause, resume, queues, remove, rename, download again.

use super::*;

impl Manager {
    /// When a started queue has nothing left: its sound, the file to open, then its "after" action.
    pub(super) fn check_queues_done(self: &Arc<Self>) {
        let settings = self.settings.read().clone();
        for queue in settings.queues.iter() {
            let active = self.records.lock().iter().any(|r| r.queue == queue.name && (r.status == Status::Queued || r.status.running()));
            let started = self.running_queues.lock().contains(&queue.name);
            let had_any = self.records.lock().iter().any(|r| r.queue == queue.name && r.status == Status::Completed);
            if started && !active && had_any {
                self.running_queues.lock().remove(&queue.name);
                if let Some(sound) = settings.sounds.get(3).filter(|s| s.on) { crate::system::play(&sound.file, true); }
                if !queue.open_when_done.trim().is_empty() {
                    let _ = crate::system::open_path(std::path::Path::new(queue.open_when_done.trim()));
                }
                let _ = self.app.emit("queue-finished", (queue.name.clone(), queue.after));
            }
        }
    }

    pub fn pause(&self, ids: &[String]) {
        for id in ids {
            if let Some(live) = self.live.lock().get(id) {
                live.cancel.cancel();
            } else if self.update(id, |r| if r.status == Status::Queued { r.status = Status::Paused }).is_some() {
                self.save(id);
                self.emit(id);
            }
        }
    }
    pub fn pause_all(&self) {
        let ids: Vec<String> = self.records.lock().iter().filter(|r| r.status.running() || r.status == Status::Queued).map(|r| r.id.clone()).collect();
        self.pause(&ids);
    }
    /// Resume = start now (IDM's "Resume"); queued resumes wait for their queue.
    pub fn resume(self: &Arc<Self>, ids: &[String]) {
        for id in ids {
            if self.record(id).is_some_and(|r| r.status.resumable()) {
                self.run(id);
            }
        }
    }
    pub fn resume_all(self: &Arc<Self>) {
        let ids: Vec<String> = self.records.lock().iter().filter(|r| r.status.resumable()).map(|r| r.id.clone()).collect();
        self.resume(&ids);
    }
    pub fn set_queue(self: &Arc<Self>, ids: &[String], queue: &str) {
        for id in ids {
            self.update(id, |r| r.queue = queue.to_string());
            self.save(id);
            self.emit(id);
        }
        self.schedule();
    }
    /// Waits in its queue (instead of starting now).
    pub fn enqueue(self: &Arc<Self>, ids: &[String]) {
        for id in ids {
            self.update(id, |r| if r.status.resumable() { r.status = Status::Queued });
            self.save(id);
            self.emit(id);
        }
        self.schedule();
    }
    pub fn start_queue(self: &Arc<Self>, name: &str) {
        self.running_queues.lock().insert(name.to_string());
        let ids: Vec<String> = self.records.lock().iter().filter(|r| r.queue == name && r.status.resumable()).map(|r| r.id.clone()).collect();
        self.enqueue(&ids);
    }
    /// Lets a queue run what is waiting in it, without restarting its paused downloads.
    pub fn run_waiting(self: &Arc<Self>, name: &str) {
        self.running_queues.lock().insert(name.to_string());
        self.schedule();
    }
    /// Scheduler > Files in the queue: moves a download up (-1) or down (+1) among its queue's files.
    pub fn move_in_queue(self: &Arc<Self>, id: &str, delta: i32) {
        let changed: Vec<String> = {
            let mut records = self.records.lock();
            let Some(queue) = records.iter().find(|r| r.id == id).map(|r| r.queue.clone()) else { return };
            let mut order: Vec<(u64, String)> = records.iter().filter(|r| r.queue == queue && r.status != Status::Completed).map(|r| (r.queue_key(), r.id.clone())).collect();
            order.sort();
            let Some(at) = order.iter().position(|(_, i)| i == id) else { return };
            let to = at as i64 + delta as i64;
            if to < 0 || to >= order.len() as i64 {
                return;
            }
            order.swap(at, to as usize);
            for (place, (_, rid)) in order.iter().enumerate() {
                if let Some(r) = records.iter_mut().find(|r| &r.id == rid) {
                    r.queue_order = place as u64 + 1;
                }
            }
            order.into_iter().map(|(_, i)| i).collect()
        };
        for id in &changed {
            self.save(id);
            self.emit(id);
        }
    }
    pub fn stop_queue(self: &Arc<Self>, name: &str) {
        self.running_queues.lock().remove(name);
        let ids: Vec<String> = self.records.lock().iter().filter(|r| r.queue == name && (r.status.running() || r.status == Status::Queued)).map(|r| r.id.clone()).collect();
        self.pause(&ids);
        self.schedule();
    }
    pub fn set_speed_limit(&self, id: &str, bytes_per_second: u64) {
        self.update(id, |r| r.speed_limit = bytes_per_second);
        if let Some(live) = self.live.lock().get(id) {
            live.limiter.set_rate(bytes_per_second);
        }
        self.save(id);
        self.emit(id);
    }

    pub fn remove(&self, ids: &[String], delete_files: bool) -> Vec<String> {
        let mut removed = Vec::new();
        for id in ids {
            if let Some(live) = self.live.lock().remove(id) {
                live.cancel.cancel();
            }
            let record = {
                let mut records = self.records.lock();
                let Some(index) = records.iter().position(|r| &r.id == id) else { continue };
                records.remove(index)
            };
            let part = kestrel_engine::file::part_path(&record.destination);
            let work = matches!(record.kind, Kind::Video { .. }).then(|| Manager::video_work(&record));
            let torrent = matches!(record.kind, Kind::Torrent { .. });
            // A running transfer releases its file a moment after cancelling.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                if delete_files {
                    // A torrent with several files is a folder.
                    if torrent && record.destination.is_dir() { let _ = std::fs::remove_dir_all(&record.destination); } else { let _ = std::fs::remove_file(&record.destination); }
                }
                if torrent && (record.status != Status::Completed || delete_files) {
                    // Left by aria2, which ran torrents before Kestrel 2.1.
                    let mut control = record.destination.clone().into_os_string();
                    control.push(".aria2");
                    let _ = std::fs::remove_file(control);
                }
                if record.status != Status::Completed || delete_files { let _ = std::fs::remove_file(part); }
                if let Some(work) = work.filter(|_| record.status != Status::Completed || delete_files) { let _ = std::fs::remove_dir_all(work); }
            });
            let _ = self.store.lock().remove_download(id);
            removed.push(id.clone());
        }
        let _ = self.app.emit("removed", removed.clone());
        removed
    }
    pub fn remove_completed(&self) -> Vec<String> {
        let ids: Vec<String> = self.records.lock().iter().filter(|r| r.status == Status::Completed).map(|r| r.id.clone()).collect();
        self.remove(&ids, false)
    }
    /// "Refresh address": an expired link replaced by a new one for the same file. The progress is
    /// kept when the new address gives a file of the same size; otherwise nothing changes.
    pub async fn refresh_address(self: &Arc<Self>, id: &str, address: &str) -> Result<(), String> {
        let record = self.record(id).ok_or("Unknown download.")?;
        if !matches!(record.kind, Kind::Http) {
            return Err("Only a file download's address can be refreshed.".into());
        }
        if self.live.lock().contains_key(id) {
            return Err("Stop the download before changing its address.".into());
        }
        let url = Url::parse(address.trim()).map_err(|_| "Enter a full address that starts with http:// or https://.")?;
        if !matches!(url.scheme(), "http" | "https" | "ftp" | "ftps" | "sftp") {
            return Err("Enter a full address that starts with http:// or https://.".into());
        }
        let mut job = Job::new(url.clone(), record.destination.clone());
        job.session = self.with_login(&url, record.session.clone().filter(|s| kestrel_engine::session::origin_of(&url) == s.origin));
        // The new address is a new server's copy: its validators are its own; the size must match.
        job.resume = record.resume.clone();
        job.resume.etag = None;
        job.resume.last_modified = None;
        let probed = self.engine.probe(&mut job).await.map_err(|e| e.message)?;
        if let (Some(before), Some(now)) = (record.resume.total.or(record.total), probed.total)
            && before != now {
                return Err(format!("This address gives a different file ({} instead of {} bytes). Use Redownload to start it again.", now, before));
            }
        let kept = record.written > 0 && !job.resume.plan.segments.is_empty();
        self.update(id, |r| {
            r.url = url.to_string();
            r.session = job.session.clone();
            r.resume = job.resume.clone();
            r.written = r.resume.written();
            r.total = probed.total.or(r.total);
            r.error = None;
            if r.status == Status::Failed { r.status = Status::Paused; }
        });
        self.save(id);
        self.emit(id);
        crate::applog::info(format!("Download {id}: address refreshed ({})", if kept { "progress kept" } else { "starts again" }));
        Ok(())
    }

    /// Renames several downloads with a pattern: {name} {ext} {n} {date} {site}.
    pub fn rename_many(&self, ids: &[String], pattern: &str) -> Vec<Result<String, String>> {
        let width = ids.len().to_string().len().max(2);
        ids.iter().enumerate().map(|(i, id)| {
            let record = self.record(id).ok_or_else(|| "Unknown download.".to_string())?;
            let file = record.destination.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let path = Path::new(&file);
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let date = chrono::DateTime::from_timestamp_millis(record.added_ms as i64).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string()).unwrap_or_default();
            let site = Url::parse(&record.url).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string())).unwrap_or_default();
            let mut name = pattern.replace("{name}", &stem).replace("{n}", &format!("{:0width$}", i + 1)).replace("{date}", &date).replace("{site}", &site).replace("{ext}", &ext);
            // No extension in the pattern: the file keeps its own.
            if !pattern.contains("{ext}") && !ext.is_empty() {
                name = format!("{name}.{ext}");
            }
            self.rename(id, &name).map(|_| name)
        }).collect()
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<(), String> {
        let record = self.record(id).ok_or("Unknown download.")?;
        if self.live.lock().contains_key(id) {
            return Err("Stop the download before renaming it.".into());
        }
        if matches!(record.kind, Kind::Torrent { .. }) {
            return Err("A torrent keeps the name its files were shared under.".into());
        }
        let name = kestrel_engine::sanitize_file_name(name);
        let folder = record.destination.parent().unwrap_or(Path::new("")).to_path_buf();
        let target = self.unique(&folder, &name);
        if record.destination.exists() {
            std::fs::rename(&record.destination, &target).map_err(|e| e.to_string())?;
        }
        let part = kestrel_engine::file::part_path(&record.destination);
        if part.exists() {
            std::fs::rename(&part, kestrel_engine::file::part_path(&target)).map_err(|e| e.to_string())?;
        }
        self.update(id, |r| { r.destination = target; r.category = category_for(&name).into(); });
        self.save(id);
        self.emit(id);
        Ok(())
    }
    pub fn redownload(self: &Arc<Self>, id: &str) -> Result<String, String> {
        let record = self.record(id).ok_or("Unknown download.")?;
        let name = record.name();
        if let Kind::Torrent { selection } = record.kind {
            let folder = record.destination.parent().map(|p| p.to_string_lossy().into_owned());
            return self.add(AddRequest { url: record.url, folder, start: Some("now".into()), torrent: Some(selection), ..Default::default() });
        }
        let video = match record.kind { Kind::Video { mut selection } => { selection.impersonate = false; Some(selection) } _ => None };
        self.add(AddRequest { url: record.url, file_name: Some(name), session: record.session, start: Some("now".into()), video, ..Default::default() })
    }
}
