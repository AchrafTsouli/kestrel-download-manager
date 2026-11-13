//! The download manager: owns every download, runs queues, saves history and tells the windows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::{Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kestrel_engine::{Engine, Event, HostLimits, Job, Limiter, Options, Outcome, Session};
use kestrel_store::Store;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::model::{Kind, LiveInfo, MAIN_QUEUE, Record, Settings, Status, View, category_for};

mod actions;
mod add;
mod media;
use media::video_tools_directory;
mod rules;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A transfer in progress (not saved).
struct Live {
    cancel: CancellationToken,
    limiter: Limiter,
    info: LiveInfo,
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
    /// A video quality (and clip) chosen in the picker or the browser.
    pub video: Option<kestrel_media::Selection>,
    /// A torrent's chosen files (the address is then its magnet link).
    pub torrent: Option<kestrel_torrent::Selection>,
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
    /// Where the file really is, when the address led there (a redirect or a download page).
    #[serde(default)]
    pub final_url: Option<String>,
}

pub struct Manager {
    pub app: AppHandle,
    pub engine: Engine,
    pub media: kestrel_media::Tools,
    pub torrents: kestrel_torrent::Tools,
    store: Mutex<Store>,
    pub settings: RwLock<Settings>,
    records: Mutex<Vec<Record>>,
    live: Mutex<HashMap<String, Live>>,
    /// Queues whose schedule started them (a stopped queue keeps its downloads waiting).
    pub running_queues: Mutex<std::collections::HashSet<String>>,
    /// Kestrel dialled the VPN and it is still up.
    vpn_up: tokio::sync::Mutex<bool>,
    /// Downloads whose progress changed since the windows were last told (sent together, 4 times a second).
    changed: Mutex<std::collections::HashSet<String>>,
    /// Downloads with a checkpoint not saved yet (saved together, every 2 seconds and on exit).
    dirty: Mutex<std::collections::HashSet<String>>,
}

pub type Shared = Arc<Manager>;

impl Manager {
    pub fn open(app: AppHandle, data_dir: &Path) -> Result<Shared, String> {
        let mut store = Store::open(&data_dir.join("history.db")).map_err(|e| e.to_string())?;
        // The first start after Flux 1.x brings its settings and downloads along (once).
        // (A portable copy starts empty: the installed 1.x's history belongs to the installed Kestrel.)
        let fresh = !kestrel_store::is_portable() && store.get::<Settings>("settings").map_err(|e| e.to_string())?.is_none() && store.get::<bool>("imported-v1").ok().flatten().is_none();
        if fresh {
            if let Some((settings, records)) = crate::migrate::import(&kestrel_store::v1_history_path(), &data_dir.join("torrents")) {
                let list: Vec<(String, Record)> = records.into_iter().map(|r| (r.id.clone(), r)).collect();
                store.save_downloads(&list).map_err(|e| e.to_string())?;
                store.set("settings", &settings).map_err(|e| e.to_string())?;
            }
            store.set("imported-v1", &true).map_err(|e| e.to_string())?;
        }
        let settings: Settings = store.get("settings").map_err(|e| e.to_string())?.unwrap_or_default();
        let mut records: Vec<Record> = store.load_downloads::<Record>().map_err(|e| e.to_string())?.into_iter().map(|(_, r)| r).collect();
        // Nothing runs before Kestrel starts it again; unfinished downloads wait as paused.
        for record in &mut records {
            if record.status.running() || record.status == Status::Queued {
                record.status = Status::Paused;
            }
        }
        let app_for_limits = app.clone();
        let limits = HostLimits::new(settings.host_limits.clone()).on_learned(move |host, limit| {
            let _ = app_for_limits.emit("host-limit", (host.to_string(), limit));
        });
        let engine = Engine::new(Options { max_retries: settings.max_retries, known_hosts: Some(data_dir.join("known_hosts")), ..Options::default() }, limits);
        engine.global_limiter.set_rate(settings.speed_limit_at(&crate::system::local_time().1));
        let manager = Arc::new(Manager {
            app,
            engine,
            media: kestrel_media::Tools::new(video_tools_directory(), kestrel_store::default_directory().join("tools")),
            torrents: kestrel_torrent::Tools::new(kestrel_store::default_directory().join("torrents")),
            store: Mutex::new(store),
            settings: RwLock::new(settings),
            records: Mutex::new(records),
            live: Mutex::new(HashMap::new()),
            running_queues: Mutex::new(std::collections::HashSet::new()),
            vpn_up: tokio::sync::Mutex::new(false),
            changed: Mutex::new(std::collections::HashSet::new()),
            dirty: Mutex::new(std::collections::HashSet::new()),
        });
        Manager::pace(Arc::downgrade(&manager));
        let current = manager.settings.read().clone();
        if let Err(error) = manager.apply_network(&current) {
            // Downloads go direct until the setting is corrected in Options.
            let _ = manager.app.emit("message", format!("Proxy: {error}"));
        }
        Ok(manager)
    }

    // ------------------------------------------------------------------ views and saving

    fn view_of(&self, record: &Record) -> View {
        let live = self.live.lock();
        View::of(record, live.get(&record.id).map(|l| &l.info).unwrap_or(&LiveInfo::default()))
    }
    pub fn views(&self) -> Vec<View> {
        self.views_where(|_| true)
    }
    /// The views of the downloads that pass `keep`, without copying the others (the list can hold
    /// thousands, and the tray and the taskbar look every second).
    pub fn views_where(&self, keep: impl Fn(&Record) -> bool) -> Vec<View> {
        // The live part is small (running transfers only); copied first, so the two locks are never held together.
        let live: HashMap<String, LiveInfo> = self.live.lock().iter().map(|(id, l)| (id.clone(), l.info.clone())).collect();
        let none = LiveInfo::default();
        self.records.lock().iter().filter(|r| keep(r)).map(|r| View::of(r, live.get(&r.id).unwrap_or(&none))).collect()
    }
    pub fn has_url(&self, url: &str) -> bool {
        self.records.lock().iter().any(|r| r.url == url)
    }
    pub fn ids_with_url(&self, url: &str) -> Vec<String> {
        self.records.lock().iter().filter(|r| r.url == url).map(|r| r.id.clone()).collect()
    }
    pub fn view(&self, id: &str) -> Option<View> {
        let record = self.record(id)?;
        Some(self.view_of(&record))
    }
    pub fn record(&self, id: &str) -> Option<Record> {
        self.records.lock().iter().find(|r| r.id == id).cloned()
    }
    fn update(&self, id: &str, change: impl FnOnce(&mut Record)) -> Option<Record> {
        let mut records = self.records.lock();
        let record = records.iter_mut().find(|r| r.id == id)?;
        change(record);
        Some(record.clone())
    }
    /// [`Manager::update`] without the copy (progress, several times a second).
    fn modify(&self, id: &str, change: impl FnOnce(&mut Record)) {
        if let Some(record) = self.records.lock().iter_mut().find(|r| r.id == id) {
            change(record);
        }
    }
    /// Progress for the windows, sent with the others at the next tick.
    fn changed(&self, id: &str) {
        self.changed.lock().insert(id.to_string());
    }
    /// Every 250 ms: one "downloads" event with what changed. Every 2 s: the checkpoints, in one transaction.
    fn pace(manager: std::sync::Weak<Manager>) {
        std::thread::spawn(move || {
            let mut tick = 0u32;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let Some(manager) = manager.upgrade() else { return };
                tick = tick.wrapping_add(1);
                let ids: Vec<String> = std::mem::take(&mut *manager.changed.lock()).into_iter().collect();
                if !ids.is_empty() {
                    let views = manager.views_where(|r| ids.contains(&r.id));
                    let _ = manager.app.emit("downloads", views);
                }
                if tick.is_multiple_of(8) {
                    manager.save_dirty();
                }
            }
        });
    }
    fn save_dirty(&self) {
        let ids: Vec<String> = std::mem::take(&mut *self.dirty.lock()).into_iter().collect();
        if ids.is_empty() {
            return;
        }
        let list: Vec<(String, Record)> = self.records.lock().iter().filter(|r| ids.contains(&r.id)).map(|r| (r.id.clone(), r.clone())).collect();
        if let Err(error) = self.store.lock().save_many(&list) {
            crate::applog::error(format!("Saving {} downloads failed: {error}", list.len()));
            let _ = self.app.emit("message", format!("Could not save the download list: {error}"));
        }
    }
    fn emit(&self, id: &str) {
        if let Some(view) = self.view(id) {
            let _ = self.app.emit("download", view);
        }
    }
    fn save(&self, id: &str) {
        self.dirty.lock().remove(id);
        if let Some(record) = self.record(id)
            && let Err(error) = self.store.lock().save_download(id, &record) {
                crate::applog::error(format!("Saving download {id} failed: {error}"));
                let _ = self.app.emit("message", format!("Could not save the download list: {error}"));
            }
    }
    /// Proxy and browser identity for the engine, yt-dlp and torrents.
    fn apply_network(&self, settings: &Settings) -> Result<(), String> {
        let proxy = (settings.proxy_mode == 2 && !settings.proxy.trim().is_empty()).then(|| kestrel_engine::Proxy {
            url: settings.proxy.trim().to_string(),
            username: settings.proxy_username.clone(),
            password: settings.proxy_password.clone(),
        });
        self.engine.set_network_with(proxy.as_ref(), Some(settings.user_agent.as_str()), settings.proxy_mode == 1)?;
        let with_credentials = match &proxy {
            Some(p) => Some(p.with_credentials()?),
            None => None,
        };
        self.media.set_proxy(with_credentials.clone());
        self.torrents.set_proxy(with_credentials);
        Ok(())
    }

    pub fn save_settings(self: &Arc<Self>, mut settings: Settings) -> Result<(), String> {
        self.apply_network(&settings)?;
        settings.connections = settings.connections.clamp(1, 32);
        settings.max_retries = settings.max_retries.min(20);
        if settings.queues.iter().all(|q| q.name != MAIN_QUEUE) {
            settings.queues.insert(0, crate::model::Queue::default());
        }
        self.engine.host_limits.replace(settings.host_limits.clone());
        if settings.speed_limit > 0 {
            settings.last_speed_limit = settings.speed_limit;
        }
        self.engine.global_limiter.set_rate(settings.speed_limit_at(&crate::system::local_time().1));
        self.store.lock().set("settings", &settings).map_err(|e| e.to_string())?;
        *self.settings.write() = settings.clone();
        let _ = self.app.emit("settings", settings);
        self.schedule();
        Ok(())
    }
    pub fn settings(&self) -> Settings {
        let mut settings = self.settings.read().clone();
        settings.host_limits = self.engine.host_limits.snapshot();
        settings
    }

    // ------------------------------------------------------------------ running

    fn running_in(&self, queue: &str) -> usize {
        let live = self.live.lock();
        self.records.lock().iter().filter(|r| r.queue == queue && live.contains_key(&r.id)).count()
    }

    /// Starts queued downloads of running queues while their limits allow.
    pub fn schedule(self: &Arc<Self>) {
        let queues = self.settings.read().queues.clone();
        let running = self.running_queues.lock().clone();
        for queue in queues.iter().filter(|q| running.contains(&q.name)) {
            let mut waiting: Vec<(u64, String)> = self.records.lock().iter().filter(|r| r.queue == queue.name && r.status == Status::Queued).map(|r| (r.queue_key(), r.id.clone())).collect();
            waiting.sort();
            let waiting: Vec<String> = waiting.into_iter().map(|(_, id)| id).collect();
            for id in waiting {
                if self.running_in(&queue.name) >= queue.max_active.max(1) {
                    break;
                }
                self.run(&id);
            }
        }
        let _ = self.app.emit("queues", self.running_queues.lock().iter().cloned().collect::<Vec<_>>());
    }

    pub fn run(self: &Arc<Self>, id: &str) {
        if self.live.lock().contains_key(id) {
            return;
        }
        let Some(record) = self.update(id, |r| {
            r.status = Status::Connecting;
            r.error = None;
            r.last_try_ms = Some(now_ms());
        }) else { return };
        // Only an HTTP download needs its address parsed here; a damaged one fails that download only.
        let url = match (&record.kind, Url::parse(&record.url)) {
            (Kind::Http, Err(_)) => {
                self.modify(id, |r| {
                    r.status = Status::Failed;
                    r.error = Some("This download's address is not valid.".into());
                });
                self.finished(id, false, Some("This download's address is not valid.".into()));
                return;
            }
            (_, url) => url.ok(),
        };
        let kind = match record.kind { Kind::Http => "file", Kind::Video { .. } => "video", Kind::Torrent { .. } => "torrent" };
        crate::applog::info(format!("Download {id} started ({kind}, {} connections)", record.connections));
        let cancel = CancellationToken::new();
        let limiter = Limiter::new(record.speed_limit);
        self.live.lock().insert(id.to_string(), Live { cancel: cancel.clone(), limiter: limiter.clone(), info: LiveInfo::default() });
        match &record.kind {
            Kind::Http => {}
            Kind::Video { selection } => return self.run_video(record.clone(), selection.clone(), cancel),
            Kind::Torrent { selection } => return self.run_torrent(record.clone(), selection.clone(), cancel),
        }
        self.emit(id);
        let mut job = Job {
            url: url.expect("checked above"),
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
            let result = match manager.ensure_vpn().await {
                Err(error) => Err(kestrel_engine::Error::protocol(error)),
                Ok(()) => manager.engine.download(&mut job, events, cancel).await,
            };
            let result = match result {
                Ok(Outcome::Completed { path, bytes, sha256 }) => match manager.scan(&path).await {
                    Ok(()) => {
                        manager.server_date(&path, job.resume.last_modified.as_deref());
                        manager.unpack(&id, &path).await;
                        Ok(Outcome::Completed { path, bytes, sha256 })
                    }
                    Err(error) => Err(kestrel_engine::Error::protocol(error)),
                },
                other => other,
            };
            manager.finish(&id, job, result);
        });
    }

    fn on_event(&self, id: &str, event: Event) {
        match event {
            Event::Probed(info) => {
                self.modify(id, |r| {
                    r.total = info.total;
                    r.status = Status::Downloading;
                });
            }
            Event::Progress { written, total, bytes_per_second, connections } => {
                self.modify(id, |r| {
                    r.written = written;
                    r.total = total.or(r.total);
                    if r.status != Status::Finishing { r.status = Status::Downloading; }
                });
                if let Some(live) = self.live.lock().get_mut(id) {
                    live.info.speed = bytes_per_second;
                    live.info.connections = connections;
                }
            }
            Event::Checkpoint(resume) => {
                self.modify(id, |r| { r.written = resume.written(); r.resume = resume; });
                self.dirty.lock().insert(id.to_string());
            }
            Event::Retrying { delay, error, .. } => {
                crate::applog::warn(format!("Download {id}: {} Retrying in {} s.", error.message, delay.as_secs().max(1)));
                self.modify(id, |r| { r.status = Status::Retrying; r.error = Some(error.message.clone()); });
                if let Some(live) = self.live.lock().get_mut(id) {
                    live.info.note = Some(format!("{} Retrying in {} s.", error.message, delay.as_secs().max(1)));
                }
            }
            Event::Note(note) => {
                if let Some(live) = self.live.lock().get_mut(id) {
                    live.info.note = Some(note);
                }
            }
        }
        self.changed(id);
    }

    fn finish(self: &Arc<Self>, id: &str, job: Job, result: kestrel_engine::Result<Outcome>) {
        self.live.lock().remove(id);
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
        self.finished(id, completed, result.as_ref().err().map(|e| e.message.clone()));
    }

    /// After any transfer ends: save, tell the windows, start what waits.
    fn finished(self: &Arc<Self>, id: &str, completed: bool, error: Option<String>) {
        match (&error, completed) {
            (_, true) => crate::applog::info(format!("Download {id} completed")),
            (Some(error), _) => crate::applog::warn(format!("Download {id} failed: {error}")),
            (None, _) => crate::applog::info(format!("Download {id} stopped")),
        }
        self.save(id);
        self.emit(id);
        self.disconnect_vpn_if_idle();
        if completed {
            let _ = self.app.emit("completed", id.to_string());
        } else if let Some(error) = error {
            let _ = self.app.emit("failed", (id.to_string(), error));
        }
        self.schedule();
        self.check_queues_done();
    }

    /// Night mode: the scheduler calls this every few seconds so the limit follows the clock.
    pub fn apply_speed_schedule(&self) {
        let limit = self.settings.read().speed_limit_at(&crate::system::local_time().1);
        if self.engine.global_limiter.rate() != limit {
            self.engine.global_limiter.set_rate(limit);
        }
    }
    pub fn shutdown(&self) {
        // Every other record was saved when it changed: only the running ones and unsaved checkpoints remain.
        let running: Vec<String> = {
            let live = self.live.lock();
            live.values().for_each(|l| l.cancel.cancel());
            live.keys().cloned().collect()
        };
        crate::applog::info(format!("Kestrel closing ({} downloads were running)", running.len()));
        self.dirty.lock().extend(running);
        self.save_dirty();
    }
}

/// Splits a command line into arguments ("quoted parts" stay whole).
pub fn split_arguments(text: &str) -> Result<Vec<String>, String> {
    let (mut out, mut token, mut quoted) = (Vec::new(), String::new(), false);
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    out.push(std::mem::take(&mut token));
                }
            }
            c => token.push(c),
        }
    }
    if quoted {
        return Err("The scanner arguments have an unmatched quote.".into());
    }
    if !token.is_empty() {
        out.push(token);
    }
    Ok(out)
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
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
