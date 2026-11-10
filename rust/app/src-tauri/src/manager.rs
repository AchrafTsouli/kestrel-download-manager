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

use crate::model::{Kind, LiveInfo, MAIN_QUEUE, Record, Settings, Status, View, category_for};

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
    pub video: Option<flux_media::Selection>,
    /// A torrent's chosen files (the address is then its magnet link).
    pub torrent: Option<flux_torrent::Selection>,
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
    pub media: flux_media::Tools,
    pub torrents: flux_torrent::Tools,
    store: Mutex<Store>,
    pub settings: RwLock<Settings>,
    records: Mutex<Vec<Record>>,
    live: Mutex<HashMap<String, Live>>,
    /// Queues whose schedule started them (a stopped queue keeps its downloads waiting).
    pub running_queues: Mutex<std::collections::HashSet<String>>,
    /// Flux dialled the VPN and it is still up.
    vpn_up: tokio::sync::Mutex<bool>,
}

pub type Shared = Arc<Manager>;

impl Manager {
    pub fn open(app: AppHandle, data_dir: &Path) -> Result<Shared, String> {
        let mut store = Store::open(&data_dir.join("history.db")).map_err(|e| e.to_string())?;
        // The first start after Flux 1.x brings its settings and downloads along (once).
        // (A portable copy starts empty: the installed 1.x's history belongs to the installed Flux.)
        let fresh = !flux_store::is_portable() && store.get::<Settings>("settings").map_err(|e| e.to_string())?.is_none() && store.get::<bool>("imported-v1").ok().flatten().is_none();
        if fresh {
            if let Some((settings, records)) = crate::migrate::import(&flux_store::v1_history_path(), &data_dir.join("torrents")) {
                let list: Vec<(String, Record)> = records.into_iter().map(|r| (r.id.clone(), r)).collect();
                store.save_downloads(&list).map_err(|e| e.to_string())?;
                store.set("settings", &settings).map_err(|e| e.to_string())?;
            }
            store.set("imported-v1", &true).map_err(|e| e.to_string())?;
        }
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
            media: flux_media::Tools::new(video_tools_directory(), flux_store::default_directory().join("tools")),
            torrents: flux_torrent::Tools::new(aria2_path(), flux_store::default_directory().join("torrents")),
            store: Mutex::new(store),
            settings: RwLock::new(settings),
            records: Mutex::new(records),
            live: Mutex::new(HashMap::new()),
            running_queues: Mutex::new(std::collections::HashSet::new()),
            vpn_up: tokio::sync::Mutex::new(false),
        });
        let current = manager.settings.read().unwrap().clone();
        if let Err(error) = manager.apply_network(&current) {
            // Downloads go direct until the setting is corrected in Options.
            let _ = manager.app.emit("message", format!("Proxy: {error}"));
        }
        Ok(manager)
    }

    // ------------------------------------------------------------------ views and saving

    fn view_of(&self, record: &Record) -> View {
        let live = self.live.lock().unwrap();
        View::of(record, live.get(&record.id).map(|l| &l.info).unwrap_or(&LiveInfo::default()))
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
        if let Some(record) = self.record(id)
            && let Err(error) = self.store.lock().unwrap().save_download(id, &record) {
                let _ = self.app.emit("message", format!("Could not save the download list: {error}"));
            }
    }
    fn save_all(&self) {
        let list: Vec<(String, Record)> = self.records.lock().unwrap().iter().map(|r| (r.id.clone(), r.clone())).collect();
        let _ = self.store.lock().unwrap().save_downloads(&list);
    }
    /// Proxy and browser identity for the engine, yt-dlp and aria2.
    fn apply_network(&self, settings: &Settings) -> Result<(), String> {
        let proxy = (settings.proxy_mode == 2 && !settings.proxy.trim().is_empty()).then(|| flux_engine::Proxy {
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

    pub fn folder_for(&self, category: &str) -> PathBuf {
        let settings = self.settings.read().unwrap();
        if let Some(dir) = settings.category_dirs.get(category).filter(|d| d.is_absolute()) {
            return dir.clone();
        }
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
                        if let Some(server_name) = info.file_name.filter(|n| !n.is_empty())
                            && (name == "download.bin" || Path::new(&name).extension().is_none() || probe.file_name == Manager::name_from_url(&job.url)) {
                                name = server_name;
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
        // The start dialog shows where a rule will put it.
        if let Some(rule) = Url::parse(url.trim()).ok().and_then(|u| self.rule_for(&u, &name)) {
            if !rule.category.trim().is_empty() {
                probe.category = rule.category.trim().to_string();
                probe.folder = self.folder_for(&probe.category).to_string_lossy().into_owned();
            }
            if Path::new(rule.folder.trim()).is_absolute() {
                probe.folder = rule.folder.trim().to_string();
            }
        }
        probe
    }

    pub fn add(self: &Arc<Self>, request: AddRequest) -> Result<String, String> {
        if let Some(selection) = request.torrent.clone() {
            return self.add_torrent(request, selection);
        }
        let mut url = Url::parse(request.url.trim()).map_err(|_| "Enter a full address starting with http:// or https://.".to_string())?;
        if let Some(selection) = &request.video {
            selection.validate(url.as_str())?;
            // Video jobs keep the page's normal form (one address per YouTube video).
            url = Url::parse(&flux_media::pages::canonicalize(url.as_str()).ok_or("Use a video page or an HLS/DASH stream address.")?).map_err(|e| e.to_string())?;
        }
        if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
            return Err("Only HTTP and HTTPS links without a user name in them are supported.".into());
        }
        if let Some(request_id) = &request.request_id
            && let Some(existing) = self.records.lock().unwrap().iter().find(|r| r.request_id.as_ref() == Some(request_id)) {
                return Ok(existing.id.clone());
            }
        if let Some(session) = &request.session {
            session.validate(&url).map_err(|e| e.message)?;
        }
        let session = self.with_login(&url, request.session.clone());
        let mut name = request.file_name.as_deref().filter(|n| !n.trim().is_empty()).map(flux_engine::sanitize_file_name).unwrap_or_else(|| Manager::name_from_url(&url));
        if let Some(selection) = &request.video {
            // The file is named after the format that will be written.
            let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Video".into());
            name = format!("{stem}.{}", selection.extension);
        }
        let rule = self.rule_for(&url, &name);
        let rule_text = |pick: fn(&crate::model::Rule) -> &String| rule.as_ref().map(|r| pick(r).trim().to_string()).filter(|t| !t.is_empty());
        let category = request.category.clone().or_else(|| rule_text(|r| &r.category)).unwrap_or_else(|| match &request.video {
            Some(selection) if selection.audio_only => "Music".to_string(),
            Some(_) => "Video".to_string(),
            None => category_for(&name).to_string(),
        });
        let folder = request.folder.as_ref().map(PathBuf::from).filter(|p| p.is_absolute())
            .or_else(|| rule_text(|r| &r.folder).map(PathBuf::from).filter(|p| p.is_absolute()))
            .unwrap_or_else(|| self.folder_for(&category));
        std::fs::create_dir_all(&folder).map_err(|e| format!("The folder could not be created: {e}"))?;
        // A file of that name already there: a numbered copy, overwrite it, or leave it.
        let plain = folder.join(&name);
        let taken_by_a_download = self.records.lock().unwrap().iter().any(|r| r.destination == plain && r.status != Status::Completed);
        let destination = match self.settings.read().unwrap().duplicate_action {
            1 if !taken_by_a_download => plain,
            2 if plain.exists() => return Err(format!("{name} is already in {}; it was not downloaded again.", folder.display())),
            _ => self.unique(&folder, &name),
        };
        let settings = self.settings.read().unwrap().clone();
        let start = request.start.clone().unwrap_or_else(|| "now".into());
        let queue = request.queue.clone().or_else(|| rule_text(|r| &r.queue)).filter(|q| settings.queues.iter().any(|x| &x.name == q)).unwrap_or_else(|| MAIN_QUEUE.into());
        let id = format!("{:016x}{:04x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64, rand_u16());
        let record = Record {
            id: id.clone(),
            url: url.to_string(),
            destination,
            category,
            description: request.description.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| url.host_str().unwrap_or_default().trim_start_matches("www.").to_string()),
            queue: queue.clone(),
            status: if start == "later" { Status::Paused } else { Status::Queued },
            kind: match &request.video { Some(selection) => Kind::Video { selection: selection.clone() }, None => Kind::Http },
            added_ms: now_ms(),
            queue_order: 0,
            last_try_ms: None,
            completed_ms: None,
            total: request.total.or(request.video.as_ref().and_then(|v| v.total_bytes)),
            written: 0,
            error: None,
            connections: rule.as_ref().and_then(|r| r.connections).map_or(settings.connections, |n| n.clamp(1, 32)),
            speed_limit: rule.as_ref().and_then(|r| r.speed_limit).unwrap_or(0),
            session,
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

    /// A torrent is saved under its own name in the chosen folder (aria2 continues there after a pause).
    fn add_torrent(self: &Arc<Self>, request: AddRequest, selection: flux_torrent::Selection) -> Result<String, String> {
        let url = flux_torrent::magnet(&selection.info_hash, &selection.name);
        selection.validate(&url)?;
        if let Some(existing) = self.records.lock().unwrap().iter().find(|r| matches!(&r.kind, Kind::Torrent { selection: s } if s.info_hash == selection.info_hash) && r.status != Status::Completed) {
            return Err(format!("This torrent is already in the list as {}.", existing.name()));
        }
        let folder = request.folder.as_ref().map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| self.settings.read().unwrap().download_dir.join("Torrents"));
        std::fs::create_dir_all(&folder).map_err(|e| format!("The folder could not be created: {e}"))?;
        let settings = self.settings.read().unwrap().clone();
        let start = request.start.clone().unwrap_or_else(|| "now".into());
        let id = format!("{:016x}{:04x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64, rand_u16());
        let record = Record {
            id: id.clone(),
            url,
            destination: folder.join(&selection.name),
            category: "Other".into(),
            description: request.description.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| "Torrent".into()),
            queue: request.queue.clone().filter(|q| settings.queues.iter().any(|x| &x.name == q)).unwrap_or_else(|| MAIN_QUEUE.into()),
            status: if start == "later" { Status::Paused } else { Status::Queued },
            total: Some(selection.selected_bytes).filter(|b| *b > 0),
            kind: Kind::Torrent { selection },
            added_ms: now_ms(),
            queue_order: 0,
            last_try_ms: None,
            completed_ms: None,
            written: 0,
            error: None,
            connections: 0,
            speed_limit: 0,
            session: None,
            resume: Default::default(),
            expected_sha256: None,
            sha256: None,
            request_id: request.request_id.clone(),
            retries: 0,
        };
        self.records.lock().unwrap().push(record);
        self.save(&id);
        self.emit(&id);
        if start == "now" { self.run(&id) } else { self.schedule() }
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
            let mut waiting: Vec<(u64, String)> = self.records.lock().unwrap().iter().filter(|r| r.queue == queue.name && r.status == Status::Queued).map(|r| (r.queue_key(), r.id.clone())).collect();
            waiting.sort();
            let waiting: Vec<String> = waiting.into_iter().map(|(_, id)| id).collect();
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
        let cancel = CancellationToken::new();
        let limiter = Limiter::new(record.speed_limit);
        self.live.lock().unwrap().insert(id.to_string(), Live { cancel: cancel.clone(), limiter: limiter.clone(), info: LiveInfo::default() });
        match &record.kind {
            Kind::Http => {}
            Kind::Video { selection } => return self.run_video(record.clone(), selection.clone(), cancel),
            Kind::Torrent { selection } => return self.run_torrent(record.clone(), selection.clone(), cancel),
        }
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
            let result = match manager.ensure_vpn().await {
                Err(error) => Err(flux_engine::Error::protocol(error)),
                Ok(()) => manager.engine.download(&mut job, events, cancel).await,
            };
            let result = match result {
                Ok(Outcome::Completed { path, bytes, sha256 }) => match manager.scan(&path).await {
                    Ok(()) => {
                        manager.server_date(&path, job.resume.last_modified.as_deref());
                        manager.unpack(&id, &path).await;
                        Ok(Outcome::Completed { path, bytes, sha256 })
                    }
                    Err(error) => Err(flux_engine::Error::protocol(error)),
                },
                other => other,
            };
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
                    live.info.speed = bytes_per_second;
                    live.info.connections = connections;
                }
            }
            Event::Checkpoint(resume) => {
                self.update(id, |r| { r.written = resume.written(); r.resume = resume; });
                self.save(id);
            }
            Event::Retrying { delay, error, .. } => {
                self.update(id, |r| { r.status = Status::Retrying; r.error = Some(error.message.clone()); });
                if let Some(live) = self.live.lock().unwrap().get_mut(id) {
                    live.info.note = Some(format!("{} Retrying in {} s.", error.message, delay.as_secs().max(1)));
                }
            }
            Event::Note(note) => {
                if let Some(live) = self.live.lock().unwrap().get_mut(id) {
                    live.info.note = Some(note);
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
        self.finished(id, completed, result.as_ref().err().map(|e| e.message.clone()));
    }

    /// After any transfer ends: save, tell the windows, start what waits.
    fn finished(self: &Arc<Self>, id: &str, completed: bool, error: Option<String>) {
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

    // ------------------------------------------------------------------ logins, rules, scanner, VPN

    /// The first enabled rule that matches the address and file name.
    pub fn rule_for(&self, url: &Url, file_name: &str) -> Option<crate::model::Rule> {
        self.settings.read().unwrap().rules.iter().find(|r| r.matches(url, file_name)).cloned()
    }

    /// Adds a saved site login to the request, unless the browser sent credentials of its own.
    pub fn with_login(&self, url: &Url, session: Option<Session>) -> Option<Session> {
        use base64::Engine as _;
        if session.as_ref().is_some_and(|s| s.authorization.is_some()) {
            return session;
        }
        let settings = self.settings.read().unwrap();
        let Some(login) = settings.site_logins.iter().find(|l| l.matches(url)) else { return session };
        let mut session = session.unwrap_or_else(|| Session::for_url(url));
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", login.username, login.password));
        session.authorization = Some(format!("Basic {token}"));
        Some(session)
    }

    /// Runs the chosen virus scanner on a finished file (or torrent folder). A non-zero exit code
    /// blocks it: the file is renamed with ".blocked" so it is not opened by accident.
    async fn scan(&self, path: &Path) -> Result<(), String> {
        let (scanner, args) = {
            let s = self.settings.read().unwrap();
            (s.virus_scanner.trim().to_string(), s.virus_scanner_args.clone())
        };
        if scanner.is_empty() {
            return Ok(());
        }
        let file = path.to_string_lossy().into_owned();
        let arguments: Vec<String> = split_arguments(&args)?.into_iter().map(|a| a.replace("{file}", &file)).collect();
        let mut command = tokio::process::Command::new(&scanner);
        command.args(&arguments).kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let status = tokio::time::timeout(Duration::from_secs(30 * 60), command.status()).await
            .map_err(|_| "The virus scanner did not finish within 30 minutes.".to_string())?
            .map_err(|e| format!("The virus scanner could not start: {e}"))?;
        if status.success() {
            return Ok(());
        }
        let mut blocked = path.as_os_str().to_owned();
        blocked.push(".blocked");
        let _ = std::fs::rename(path, &blocked);
        Err(format!("The virus scanner reported a problem (exit code {}). The file was kept as {}.", status.code().unwrap_or(-1), Path::new(&blocked).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
    }

    /// Gives the finished file the server's date (Options > Save to), when the server sent one.
    fn server_date(&self, path: &Path, last_modified: Option<&str>) {
        if !self.settings.read().unwrap().set_creation_date {
            return;
        }
        let Some(date) = last_modified.and_then(|d| chrono::DateTime::parse_from_rfc2822(d.trim()).ok()) else { return };
        let time = std::time::UNIX_EPOCH + Duration::from_secs(date.timestamp().max(0) as u64);
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = file.set_times(std::fs::FileTimes::new().set_modified(time).set_accessed(time));
        }
    }

    /// Unpacks a finished archive when enabled in Options. Its result is told in the main window;
    /// a failure leaves the archive as it is and does not fail the download.
    async fn unpack(&self, id: &str, path: &Path) {
        let (wanted, delete, target) = {
            let s = self.settings.read().unwrap();
            (s.extract_archives, s.delete_extracted_archives, s.archive_target)
        };
        if !wanted || !crate::extract::is_archive(path) {
            return;
        }
        if let Some(live) = self.live.lock().unwrap().get_mut(id) {
            live.info.phase = Some("Unpacking the archive".into());
            live.info.phase_fraction = None;
        }
        self.update(id, |r| r.status = Status::Finishing);
        self.emit(id);
        let archive = path.to_path_buf();
        let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let message = match tokio::task::spawn_blocking(move || crate::extract::extract(&archive, target)).await {
            Ok(Ok((folder, what))) => {
                if delete {
                    crate::extract::recycle(path);
                }
                format!("{name}: {what} unpacked into {}.", folder.display())
            }
            Ok(Err(error)) => format!("{name} was not unpacked: {error}"),
            Err(error) => format!("{name} was not unpacked: {error}"),
        };
        let _ = self.app.emit("message", message);
    }

    /// Dials the chosen Windows VPN before a download starts (once; it stays up).
    async fn ensure_vpn(&self) -> Result<(), String> {
        let (enabled, profile) = {
            let s = self.settings.read().unwrap();
            (s.connect_vpn, s.vpn_profile.trim().to_string())
        };
        if !enabled || profile.is_empty() {
            return Ok(());
        }
        let mut up = self.vpn_up.lock().await;
        if *up {
            return Ok(());
        }
        rasdial(&profile, false).await?;
        *up = true;
        Ok(())
    }

    fn disconnect_vpn_if_idle(self: &Arc<Self>) {
        let (wanted, profile) = {
            let s = self.settings.read().unwrap();
            (s.disconnect_vpn_when_idle, s.vpn_profile.trim().to_string())
        };
        if !wanted || profile.is_empty() || !self.live.lock().unwrap().is_empty() {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut up = manager.vpn_up.lock().await;
            if *up && manager.live.lock().unwrap().is_empty() && rasdial(&profile, true).await.is_ok() {
                *up = false;
            }
        });
    }

    // ------------------------------------------------------------------ torrents

    fn run_torrent(self: &Arc<Self>, record: Record, selection: flux_torrent::Selection, cancel: CancellationToken) {
        let global = self.settings.read().unwrap().speed_limit;
        let limit = match (record.speed_limit, global) {
            (0, global) => global,
            (own, 0) => own,
            (own, global) => own.min(global),
        };
        if let Some(live) = self.live.lock().unwrap().get_mut(&record.id) {
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
                move |status: flux_torrent::Status| manager.on_torrent_status(&id, &selection, status)
            };
            let result = match manager.ensure_vpn().await {
                Err(error) => Err(error),
                Ok(()) => manager.torrents.download(&selection, &folder, limit, events, cancel).await,
            };
            let result = match result {
                Ok(flux_torrent::Outcome::Completed) => manager.scan(&folder.join(&selection.name)).await.map(|_| flux_torrent::Outcome::Completed),
                other => other,
            };
            manager.live.lock().unwrap().remove(&id);
            let completed = matches!(result, Ok(flux_torrent::Outcome::Completed));
            let error = result.as_ref().err().cloned();
            manager.update(&id, |r| match &result {
                Ok(flux_torrent::Outcome::Completed) => {
                    r.status = Status::Completed;
                    r.written = r.total.unwrap_or(r.written);
                    r.error = None;
                    r.completed_ms = Some(now_ms());
                }
                Ok(flux_torrent::Outcome::Paused) => r.status = Status::Paused,
                Err(message) => {
                    r.status = Status::Failed;
                    r.error = Some(message.clone());
                }
            });
            manager.finished(&id, completed, error);
        });
    }

    fn on_torrent_status(&self, id: &str, selection: &flux_torrent::Selection, status: flux_torrent::Status) {
        let seeding = status.state == "seeding";
        let total = if selection.selected_bytes > 0 { selection.selected_bytes } else { status.total };
        self.update(id, |r| {
            if total > 0 { r.total = Some(total); }
            r.written = if total > 0 { status.completed.min(total) } else { status.completed };
            r.status = if seeding { Status::Finishing } else if status.connections > 0 || status.completed > 0 { Status::Downloading } else { Status::Connecting };
        });
        if let Some(live) = self.live.lock().unwrap().get_mut(id) {
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
        self.emit(id);
    }

    // ------------------------------------------------------------------ videos

    /// Where a video download keeps its parts between pauses.
    fn video_work(record: &Record) -> PathBuf {
        record.destination.parent().unwrap_or(Path::new("")).join(format!(".flux-video-{}", record.id))
    }

    fn run_video(self: &Arc<Self>, record: Record, selection: flux_media::Selection, cancel: CancellationToken) {
        let settings = self.settings.read().unwrap().clone();
        let limit = match (record.speed_limit, settings.speed_limit) {
            (0, global) => global,
            (own, 0) => own,
            (own, global) => own.min(global),
        };
        let mut job = flux_media::VideoJob {
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
                move |event: flux_media::MediaEvent| manager.on_video_event(&id, event)
            };
            let result = match manager.ensure_vpn().await {
                Err(error) => Err(error),
                Ok(()) => manager.media.download(&mut job, events, cancel).await,
            };
            let result = match result {
                Ok(flux_media::Outcome::Completed { path, bytes, sha256 }) => manager.scan(&path).await.map(|_| flux_media::Outcome::Completed { path, bytes, sha256 }),
                other => other,
            };
            manager.live.lock().unwrap().remove(&id);
            let mut completed = false;
            let error = result.as_ref().err().cloned();
            manager.update(&id, |r| {
                r.kind = Kind::Video { selection: job.selection.clone() };
                match &result {
                    Ok(flux_media::Outcome::Completed { path, bytes, sha256 }) => {
                        r.destination = path.clone();
                        r.status = Status::Completed;
                        r.written = *bytes;
                        r.total = Some(*bytes);
                        r.sha256 = sha256.clone();
                        r.error = None;
                        r.completed_ms = Some(now_ms());
                        completed = true;
                    }
                    Ok(flux_media::Outcome::Paused) => r.status = Status::Paused,
                    Err(message) => {
                        r.status = Status::Failed;
                        r.error = Some(message.clone());
                    }
                }
            });
            manager.finished(&id, completed, error);
        });
    }

    fn on_video_event(&self, id: &str, event: flux_media::MediaEvent) {
        use flux_media::MediaEvent;
        match event {
            MediaEvent::Tracks(_) => {}
            MediaEvent::Progress { written, total, estimated, bytes_per_second } => {
                self.update(id, |r| {
                    r.written = written;
                    r.total = total.or(r.total);
                    r.status = Status::Downloading;
                });
                if let Some(live) = self.live.lock().unwrap().get_mut(id) {
                    live.info.speed = bytes_per_second;
                    live.info.connections = 1;
                    live.info.estimated = estimated;
                    live.info.phase = None;
                    live.info.phase_fraction = None;
                }
            }
            MediaEvent::Phase { text, fraction } => {
                self.update(id, |r| r.status = Status::Finishing);
                if let Some(live) = self.live.lock().unwrap().get_mut(id) {
                    live.info.speed = 0.0;
                    live.info.phase = Some(text);
                    live.info.phase_fraction = fraction;
                }
            }
            MediaEvent::Impersonating => {
                if let Some(live) = self.live.lock().unwrap().get_mut(id) {
                    live.info.note = Some("The site refused ordinary requests; retrying as Chrome.".into());
                }
            }
        }
        self.emit(id);
    }

    /// When a started queue has nothing left: its sound, the file to open, then its "after" action.
    fn check_queues_done(self: &Arc<Self>) {
        let settings = self.settings.read().unwrap().clone();
        for queue in settings.queues.iter() {
            let active = self.records.lock().unwrap().iter().any(|r| r.queue == queue.name && (r.status == Status::Queued || r.status.running()));
            let started = self.running_queues.lock().unwrap().contains(&queue.name);
            let had_any = self.records.lock().unwrap().iter().any(|r| r.queue == queue.name && r.status == Status::Completed);
            if started && !active && had_any {
                self.running_queues.lock().unwrap().remove(&queue.name);
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
    /// Lets a queue run what is waiting in it, without restarting its paused downloads.
    pub fn run_waiting(self: &Arc<Self>, name: &str) {
        self.running_queues.lock().unwrap().insert(name.to_string());
        self.schedule();
    }
    /// Scheduler > Files in the queue: moves a download up (-1) or down (+1) among its queue's files.
    pub fn move_in_queue(self: &Arc<Self>, id: &str, delta: i32) {
        let changed: Vec<String> = {
            let mut records = self.records.lock().unwrap();
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
                    let mut control = record.destination.clone().into_os_string();
                    control.push(".aria2");
                    let _ = std::fs::remove_file(control);
                }
                if record.status != Status::Completed || delete_files { let _ = std::fs::remove_file(part); }
                if let Some(work) = work.filter(|_| record.status != Status::Completed || delete_files) { let _ = std::fs::remove_dir_all(work); }
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
        if matches!(record.kind, Kind::Torrent { .. }) {
            return Err("A torrent keeps the name its files were shared under.".into());
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
        if let Kind::Torrent { selection } = record.kind {
            let folder = record.destination.parent().map(|p| p.to_string_lossy().into_owned());
            return self.add(AddRequest { url: record.url, folder, start: Some("now".into()), torrent: Some(selection), ..Default::default() });
        }
        let video = match record.kind { Kind::Video { mut selection } => { selection.impersonate = false; Some(selection) } _ => None };
        self.add(AddRequest { url: record.url, file_name: Some(name), session: record.session, start: Some("now".into()), video, ..Default::default() })
    }
    pub fn shutdown(&self) {
        for live in self.live.lock().unwrap().values() {
            live.cancel.cancel();
        }
        self.save_all();
    }
}

fn program(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// The video tools: bundled beside Flux when installed (Linux packages: `../lib/flux`); in a
/// development build, the repository's copy or the one Flux 1.x installed; on Linux and macOS,
/// otherwise the system's own yt-dlp and FFmpeg.
fn video_tools_directory() -> PathBuf {
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
    let mut candidates = vec![exe.join("video-tools"), exe.join("../lib/flux/video-tools"), exe.join("../Resources/video-tools"), exe.join("../../../video-tools")];
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

/// aria2 for torrents: with the video tools, or the system's own.
fn aria2_path() -> PathBuf {
    let bundled = program(&video_tools_directory(), "aria2c");
    if bundled.exists() || cfg!(windows) {
        return bundled;
    }
    let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    path.iter().chain(&["/opt/homebrew/bin".into(), "/usr/local/bin".into(), "/usr/bin".into()]).map(|d| program(d, "aria2c")).find(|p| p.exists()).unwrap_or(bundled)
}

/// Windows' own VPN dialler, with the profile's saved credentials.
#[cfg(not(windows))]
async fn rasdial(_profile: &str, _disconnect: bool) -> Result<(), String> {
    Err("Connecting a VPN from Kestrel is available on Windows only; connect it from your system's network settings.".into())
}
#[cfg(windows)]
async fn rasdial(profile: &str, disconnect: bool) -> Result<(), String> {
    let mut command = tokio::process::Command::new("rasdial.exe");
    command.arg(profile).kill_on_drop(true);
    if disconnect {
        command.arg("/disconnect");
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let status = tokio::time::timeout(Duration::from_secs(90), command.status()).await
        .map_err(|_| "The VPN did not connect within 90 seconds.".to_string())?
        .map_err(|e| format!("Windows VPN could not start: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("Windows VPN returned error {}. Check the profile \"{profile}\" and its saved sign-in.", status.code().unwrap_or(-1))) }
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
