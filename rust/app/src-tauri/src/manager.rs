//! The download manager: owns every download, runs queues, saves history and tells the windows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flux_engine::{Engine, Event, HostLimits, Job, Limiter, Options, Outcome, Session};
use flux_store::Store;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::model::{Kind, MAIN_QUEUE, Record, Settings, Status, View, category_for};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A transfer in progress (not saved).
struct Live {
    cancel: CancellationToken,
    limiter: Limiter,
    speed: f64,
    connections: usize,
    note: Option<String>,
}

/// What a window or the browser asks to add.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddRequest {
    pub url: String,
    pub file_name: Option<String>,
    pub folder: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    /// "now", "later" (paused) or a queue name.
    pub start: Option<String>,
    pub queue: Option<String>,
    pub session: Option<Session>,
    pub request_id: Option<String>,
    pub expected_sha256: Option<String>,
    pub total: Option<u64>,
}

/// File information for the start dialog, from a quick request to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub url: String,
    pub file_name: String,
    pub total: Option<u64>,
    pub resumable: bool,
    pub content_type: Option<String>,
    pub category: String,
    pub folder: String,
    pub error: Option<String>,
}

pub struct Manager {
    pub app: AppHandle,
    pub engine: Engine,
    store: Mutex<Store>,
    pub settings: RwLock<Settings>,
    records: Mutex<Vec<Record>>,
    live: Mutex<HashMap<String, Live>>,
    /// Queues whose schedule started them (a stopped queue keeps its downloads waiting).
    pub running_queues: Mutex<std::collections::HashSet<String>>,
}

pub type Shared = Arc<Manager>;

impl Manager {
    pub fn open(app: AppHandle, data_dir: &Path) -> Result<Shared, String> {
        let store = Store::open(&data_dir.join("history.db")).map_err(|e| e.to_string())?;
        let settings: Settings = store.get("settings").map_err(|e| e.to_string())?.unwrap_or_default();
        let mut records: Vec<Record> = store.load_downloads::<Record>().map_err(|e| e.to_string())?.into_iter().map(|(_, r)| r).collect();
        // Nothing runs before Flux starts it again; unfinished downloads wait as paused.
        for record in &mut records {
            if record.status.running() || record.status == Status::Queued {
                record.status = Status::Paused;
            }
        }
        let app_for_limits = app.clone();
        let limits = HostLimits::new(settings.host_limits.clone()).on_learned(move |host, limit| {
            let _ = app_for_limits.emit("host-limit", (host.to_string(), limit));
        });
        let engine = Engine::new(Options { max_retries: settings.max_retries, ..Options::default() }, limits);
        engine.global_limiter.set_rate(settings.speed_limit);
        let manager = Arc::new(Manager {
            app,
            engine,
            store: Mutex::new(store),
            settings: RwLock::new(settings),
            records: Mutex::new(records),
            live: Mutex::new(HashMap::new()),
            running_queues: Mutex::new(std::collections::HashSet::new()),
        });
        Ok(manager)
    }

    // ------------------------------------------------------------------ views and saving

    fn view_of(&self, record: &Record) -> View {
        let live = self.live.lock().unwrap();
        let (speed, connections, note) = live.get(&record.id).map(|l| (l.speed, l.connections, l.note.clone())).unwrap_or((0.0, 0, None));
        View::of(record, speed, connections, note)
    }
    pub fn views(&self) -> Vec<View> {
        let records = self.records.lock().unwrap().clone();
        records.iter().map(|r| self.view_of(r)).collect()
    }
    pub fn view(&self, id: &str) -> Option<View> {
        let record = self.record(id)?;
        Some(self.view_of(&record))
    }
    pub fn record(&self, id: &str) -> Option<Record> {
        self.records.lock().unwrap().iter().find(|r| r.id == id).cloned()
    }
    fn update(&self, id: &str, change: impl FnOnce(&mut Record)) -> Option<Record> {
        let mut records = self.records.lock().unwrap();
        let record = records.iter_mut().find(|r| r.id == id)?;
        change(record);
        Some(record.clone())
    }
    fn emit(&self, id: &str) {
        if let Some(view) = self.view(id) {
            let _ = self.app.emit("download", view);
        }
    }
    fn save(&self, id: &str) {
        if let Some(record) = self.record(id) {
            if let Err(error) = self.store.lock().unwrap().save_download(id, &record) {
                let _ = self.app.emit("message", format!("Could not save the download list: {error}"));
            }
        }
    }
    fn save_all(&self) {
        let list: Vec<(String, Record)> = self.records.lock().unwrap().iter().map(|r| (r.id.clone(), r.clone())).collect();
        let _ = self.store.lock().unwrap().save_downloads(&list);
    }
    pub fn save_settings(self: &Arc<Self>, mut settings: Settings) -> Result<(), String> {
        settings.connections = settings.connections.clamp(1, 32);
        settings.max_retries = settings.max_retries.min(20);
        if settings.queues.iter().all(|q| q.name != MAIN_QUEUE) {
            settings.queues.insert(0, crate::model::Queue::default());
        }
        self.engine.host_limits.replace(settings.host_limits.clone());
        self.engine.global_limiter.set_rate(settings.speed_limit);
        self.store.lock().unwrap().set("settings", &settings).map_err(|e| e.to_string())?;
        *self.settings.write().unwrap() = settings.clone();
        let _ = self.app.emit("settings", settings);
        self.schedule();
        Ok(())
    }
    pub fn settings(&self) -> Settings {
        let mut settings = self.settings.read().unwrap().clone();
        settings.host_limits = self.engine.host_limits.snapshot();
        settings
    }

    // ------------------------------------------------------------------ adding

    fn folder_for(&self, category: &str) -> PathBuf {
        let settings = self.settings.read().unwrap();
        if settings.category_folders && category != "Other" { settings.download_dir.join(category) } else { settings.download_dir.clone() }
    }

    /// A free name in `folder`: neither on disk nor used by another download.
    fn unique(&self, folder: &Path, name: &str) -> PathBuf {
        let records = self.records.lock().unwrap();
        let taken = |path: &Path| path.exists() || flux_engine::file::part_path(path).exists() || records.iter().any(|r| r.destination == path);
        let first = folder.join(name);
        if !taken(&first) {
            return first;
        }
        let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        (1..10_000).map(|n| folder.join(format!("{stem} ({n}){ext}"))).find(|p| !taken(p)).unwrap_or(first)
    }

    fn name_from_url(url: &Url) -> String {
        let raw = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
        let name = flux_engine::sanitize_file_name(&percent_decode(raw));
        if name.contains('.') { name } else { "download.bin".into() }
    }

    /// Asks the server for the file's real name and size (for the start dialog).
    pub async fn probe(&self, url: &str, session: Option<Session>, suggested: Option<String>) -> Probe {
        let parsed = Url::parse(url.trim());
        let fallback = parsed.as_ref().map(Manager::name_from_url).unwrap_or_else(|_| "download.bin".into());
        let mut name = suggested.filter(|s| !s.trim().is_empty()).map(|s| flux_engine::sanitize_file_name(&s)).unwrap_or(fallback);
        let mut probe = Probe { url: url.trim().to_string(), file_name: name.clone(), total: None, resumable: false, content_type: None, category: category_for(&name).into(), folder: String::new(), error: None };
        match parsed {
            Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => {
                let mut job = Job::new(parsed, std::env::temp_dir().join("flux-probe"));
                job.session = session;
                match tokio::time::timeout(Duration::from_secs(20), self.engine.probe(&mut job)).await {
                    Ok(Ok(info)) => {
                        if let Some(server_name) = info.file_name.filter(|n| !n.is_empty()) {
                            if name == "download.bin" || Path::new(&name).extension().is_none() || probe.file_name == Manager::name_from_url(&job.url) {
                                name = server_name;
                            }
                        }
                        probe.total = info.total;
                        probe.resumable = info.ranged;
                        probe.content_type = info.content_type;
                    }
                    Ok(Err(error)) => probe.error = Some(error.message),
                    Err(_) => probe.error = Some("The server did not answer in time; the size will be known when the download starts.".into()),
                }
            }
            _ => probe.error = Some("Enter a full address starting with http:// or https://.".into()),
        }
        probe.file_name = name.clone();
        probe.category = category_for(&name).into();
        probe.folder = self.folder_for(&probe.category).to_string_lossy().into_owned();
        probe
    }

    pub fn add(self: &Arc<Self>, request: AddRequest) -> Result<String, String> {
        let url = Url::parse(request.url.trim()).map_err(|_| "Enter a full address starting with http:// or https://.".to_string())?;
        if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
            return Err("Only HTTP and HTTPS links without a user name in them are supported.".into());
        }
        if let Some(request_id) = &request.request_id {
            if let Some(existing) = self.records.lock().unwrap().iter().find(|r| r.request_id.as_ref() == Some(request_id)) {
                return Ok(existing.id.clone());
            }
        }
        if let Some(session) = &request.session {
            session.validate(&url).map_err(|e| e.message)?;
        }
        let name = request.file_name.as_deref().filter(|n| !n.trim().is_empty()).map(flux_engine::sanitize_file_name).unwrap_or_else(|| Manager::name_from_url(&url));
        let category = request.category.clone().unwrap_or_else(|| category_for(&name).to_string());
        let folder = request.folder.as_ref().map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| self.folder_for(&category));
        std::fs::create_dir_all(&folder).map_err(|e| format!("The folder could not be created: {e}"))?;
        let destination = self.unique(&folder, &name);
        let settings = self.settings.read().unwrap().clone();
        let start = request.start.clone().unwrap_or_else(|| "now".into());
        let queue = request.queue.clone().filter(|q| settings.queues.iter().any(|x| &x.name == q)).unwrap_or_else(|| MAIN_QUEUE.into());
        let id = format!("{:016x}{:04x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64, rand_u16());
        let record = Record {
            id: id.clone(),
            url: url.to_string(),
            destination,
            category,
            description: request.description.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| url.host_str().unwrap_or_default().trim_start_matches("www.").to_string()),
            queue: queue.clone(),
            status: if start == "later" { Status::Paused } else { Status::Queued },
            kind: Kind::Http,
            added_ms: now_ms(),
            last_try_ms: None,
            completed_ms: None,
            total: request.total,
            written: 0,
            error: None,
            connections: settings.connections,
            speed_limit: 0,
            session: request.session.clone(),
            resume: Default::default(),
            expected_sha256: request.expected_sha256.clone().filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())),
            sha256: None,
            request_id: request.request_id.clone(),
            retries: 0,
        };
        self.records.lock().unwrap().push(record);
        self.save(&id);
        self.emit(&id);
        if start == "now" {
            // Started at once, whatever its queue's limit (like IDM's "Start download").
            self.run(&id);
        } else {
            self.schedule();
        }
        Ok(id)
    }

    // ------------------------------------------------------------------ running

    fn running_in(&self, queue: &str) -> usize {
        let live = self.live.lock().unwrap();
        self.records.lock().unwrap().iter().filter(|r| r.queue == queue && live.contains_key(&r.id)).count()
    }

    /// Starts queued downloads of running queues while their limits allow.
    pub fn schedule(self: &Arc<Self>) {
        let queues = self.settings.read().unwrap().queues.clone();
        let running = self.running_queues.lock().unwrap().clone();
        for queue in queues.iter().filter(|q| running.contains(&q.name)) {
            let waiting: Vec<String> = self.records.lock().unwrap().iter().filter(|r| r.queue == queue.name && r.status == Status::Queued).map(|r| r.id.clone()).collect();
            for id in waiting {
                if self.running_in(&queue.name) >= queue.max_active.max(1) {
                    break;
                }
                self.run(&id);
            }
        }
        let _ = self.app.emit("queues", self.running_queues.lock().unwrap().iter().cloned().collect::<Vec<_>>());
    }

    pub fn run(self: &Arc<Self>, id: &str) {
        if self.live.lock().unwrap().contains_key(id) {
            return;
        }
        let Some(record) = self.update(id, |r| {
            r.status = Status::Connecting;
            r.error = None;
            r.last_try_ms = Some(now_ms());
        }) else { return };
        if !matches!(record.kind, Kind::Http) {
            return; // Video and torrent runners are added in M4/M5.
        }
        let cancel = CancellationToken::new();
        let limiter = Limiter::new(record.speed_limit);
        self.live.lock().unwrap().insert(id.to_string(), Live { cancel: cancel.clone(), limiter: limiter.clone(), speed: 0.0, connections: 0, note: None });
        self.emit(id);
        let mut job = Job {
            url: Url::parse(&record.url).expect("stored address"),
            destination: record.destination.clone(),
            connections: record.connections,
            session: record.session.clone(),
            expected_sha256: record.expected_sha256.clone(),
            limiter,
            resume: record.resume.clone(),
        };
        let manager = self.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn(async move {
            let events = {
                let manager = manager.clone();
                let id = id.clone();
                move |event: Event| manager.on_event(&id, event)
            };
            let result = manager.engine.download(&mut job, events, cancel).await;
            manager.finish(&id, job, result);
        });
    }

    fn on_event(&self, id: &str, event: Event) {
        match event {
            Event::Probed(info) => {
                self.update(id, |r| {
                    r.total = info.total;
                    r.status = Status::Downloading;
                });
            }
            Event::Progress { written, total, bytes_per_second, connections } => {
                self.update(id, |r| {
                    r.written = written;
                    r.total = total.or(r.total);
                    if r.status != Status::Finishing { r.status = Status::Downloading; }
                });
                if let Some(live) = self.live.lock().unwrap().get_mut(id) {
                    live.speed = bytes_per_second;
                    live.connections = connections;
                }
            }
            Event::Checkpoint(resume) => {
                self.update(id, |r| { r.written = resume.written(); r.resume = resume; });
                self.save(id);
            }
            Event::Retrying { delay, error, .. } => {
                self.update(id, |r| { r.status = Status::Retrying; r.error = Some(error.message.clone()); });
                if let Some(live) = self.live.lock().unwrap().get_mut(id) {
                    live.note = Some(format!("{} Retrying in {} s.", error.message, delay.as_secs().max(1)));
                }
            }
            Event::Note(note) => {
                if let Some(live) = self.live.lock().unwrap().get_mut(id) {
                    live.note = Some(note);
                }
            }
        }
        self.emit(id);
    }

    fn finish(self: &Arc<Self>, id: &str, job: Job, result: flux_engine::Result<Outcome>) {
        self.live.lock().unwrap().remove(id);
        let mut completed = false;
        self.update(id, |r| {
            r.resume = job.resume.clone();
            r.session = job.session.clone();
            r.connections = job.connections;
            r.written = job.resume.written();
            match &result {
                Ok(Outcome::Completed { bytes, sha256, .. }) => {
                    r.status = Status::Completed;
                    r.written = *bytes;
                    r.total = Some(*bytes);
                    r.sha256 = sha256.clone();
                    r.error = None;
                    r.completed_ms = Some(now_ms());
                    completed = true;
                }
                Ok(Outcome::Paused) => r.status = Status::Paused,
                Err(error) => {
                    r.status = Status::Failed;
                    r.error = Some(error.message.clone());
                }
            }
        });
        self.save(id);
        self.emit(id);
        if completed {
            let _ = self.app.emit("completed", id.to_string());
        } else if let Err(error) = &result {
            let _ = self.app.emit("failed", (id.to_string(), error.message.clone()));
        }
        self.schedule();
        self.check_queues_done();
    }

    /// When a scheduled queue has nothing left, run its "after" action once.
    fn check_queues_done(self: &Arc<Self>) {
        let settings = self.settings.read().unwrap().clone();
        for queue in settings.queues.iter().filter(|q| q.after != crate::model::AfterQueue::Nothing) {
            let active = self.records.lock().unwrap().iter().any(|r| r.queue == queue.name && (r.status == Status::Queued || r.status.running()));
            let started = self.running_queues.lock().unwrap().contains(&queue.name);
            let had_any = self.records.lock().unwrap().iter().any(|r| r.queue == queue.name && r.status == Status::Completed);
            if started && !active && had_any {
                self.running_queues.lock().unwrap().remove(&queue.name);
                let _ = self.app.emit("queue-finished", (queue.name.clone(), queue.after));
            }
        }
    }

    pub fn pause(&self, ids: &[String]) {
        for id in ids {
            if let Some(live) = self.live.lock().unwrap().get(id) {
                live.cancel.cancel();
            } else if self.update(id, |r| if r.status == Status::Queued { r.status = Status::Paused }).is_some() {
                self.save(id);
                self.emit(id);
            }
        }
    }
    pub fn pause_all(&self) {
        let ids: Vec<String> = self.records.lock().unwrap().iter().filter(|r| r.status.running() || r.status == Status::Queued).map(|r| r.id.clone()).collect();
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
        let ids: Vec<String> = self.records.lock().unwrap().iter().filter(|r| r.status.resumable()).map(|r| r.id.clone()).collect();
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
        self.running_queues.lock().unwrap().insert(name.to_string());
        let ids: Vec<String> = self.records.lock().unwrap().iter().filter(|r| r.queue == name && r.status.resumable()).map(|r| r.id.clone()).collect();
        self.enqueue(&ids);
    }
    pub fn stop_queue(self: &Arc<Self>, name: &str) {
        self.running_queues.lock().unwrap().remove(name);
        let ids: Vec<String> = self.records.lock().unwrap().iter().filter(|r| r.queue == name && (r.status.running() || r.status == Status::Queued)).map(|r| r.id.clone()).collect();
        self.pause(&ids);
        self.schedule();
    }
    pub fn set_speed_limit(&self, id: &str, bytes_per_second: u64) {
        self.update(id, |r| r.speed_limit = bytes_per_second);
        if let Some(live) = self.live.lock().unwrap().get(id) {
            live.limiter.set_rate(bytes_per_second);
        }
        self.save(id);
        self.emit(id);
    }

    pub fn remove(&self, ids: &[String], delete_files: bool) -> Vec<String> {
        let mut removed = Vec::new();
        for id in ids {
            if let Some(live) = self.live.lock().unwrap().remove(id) {
                live.cancel.cancel();
            }
            let record = {
                let mut records = self.records.lock().unwrap();
                let Some(index) = records.iter().position(|r| &r.id == id) else { continue };
                records.remove(index)
            };
            let part = flux_engine::file::part_path(&record.destination);
            // A running transfer releases its file a moment after cancelling.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                if delete_files { let _ = std::fs::remove_file(&record.destination); }
                if record.status != Status::Completed || delete_files { let _ = std::fs::remove_file(part); }
            });
            let _ = self.store.lock().unwrap().remove_download(id);
            removed.push(id.clone());
        }
        let _ = self.app.emit("removed", removed.clone());
        removed
    }
    pub fn remove_completed(&self) -> Vec<String> {
        let ids: Vec<String> = self.records.lock().unwrap().iter().filter(|r| r.status == Status::Completed).map(|r| r.id.clone()).collect();
        self.remove(&ids, false)
    }
    pub fn rename(&self, id: &str, name: &str) -> Result<(), String> {
        let record = self.record(id).ok_or("Unknown download.")?;
        if self.live.lock().unwrap().contains_key(id) {
            return Err("Stop the download before renaming it.".into());
        }
        let name = flux_engine::sanitize_file_name(name);
        let folder = record.destination.parent().unwrap_or(Path::new("")).to_path_buf();
        let target = self.unique(&folder, &name);
        if record.destination.exists() {
            std::fs::rename(&record.destination, &target).map_err(|e| e.to_string())?;
        }
        let part = flux_engine::file::part_path(&record.destination);
        if part.exists() {
            std::fs::rename(&part, flux_engine::file::part_path(&target)).map_err(|e| e.to_string())?;
        }
        self.update(id, |r| { r.destination = target; r.category = category_for(&name).into(); });
        self.save(id);
        self.emit(id);
        Ok(())
    }
    pub fn redownload(self: &Arc<Self>, id: &str) -> Result<String, String> {
        let record = self.record(id).ok_or("Unknown download.")?;
        let name = record.name();
        self.add(AddRequest { url: record.url, file_name: Some(name), session: record.session, start: Some("now".into()), ..Default::default() })
    }
    pub fn active_count(&self) -> usize {
        self.live.lock().unwrap().len()
    }
    pub fn shutdown(&self) {
        for live in self.live.lock().unwrap().values() {
            live.cancel.cancel();
        }
        self.save_all();
    }
}

fn rand_u16() -> u16 {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    (nanos ^ (std::process::id() << 3)) as u16
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
