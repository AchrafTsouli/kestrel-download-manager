//! Adding downloads: names, folders, duplicates, and the first save.

use super::*;

impl Manager {

    pub fn folder_for(&self, category: &str) -> PathBuf {
        let settings = self.settings.read();
        if let Some(dir) = settings.category_dirs.get(category).filter(|d| d.is_absolute()) {
            return dir.clone();
        }
        if settings.category_folders && category != "Other" { settings.download_dir.join(category) } else { settings.download_dir.clone() }
    }

    /// A free name in `folder`: neither on disk nor used by another download.
    pub(super) fn unique(&self, folder: &Path, name: &str) -> PathBuf {
        let records = self.records.lock();
        let taken = |path: &Path| path.exists() || kestrel_engine::file::part_path(path).exists() || records.iter().any(|r| r.destination == path);
        let first = folder.join(name);
        if !taken(&first) {
            return first;
        }
        let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        (1..10_000).map(|n| folder.join(format!("{stem} ({n}){ext}"))).find(|p| !taken(p)).unwrap_or(first)
    }

    pub(super) fn name_from_url(url: &Url) -> String {
        let raw = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
        let name = kestrel_engine::sanitize_file_name(&percent_decode(raw));
        if name.contains('.') { name } else { "download.bin".into() }
    }

    /// Asks the server for the file's real name and size (for the start dialog).
    pub async fn probe(&self, url: &str, session: Option<Session>, suggested: Option<String>) -> Probe {
        let parsed = Url::parse(url.trim());
        let fallback = parsed.as_ref().map(Manager::name_from_url).unwrap_or_else(|_| "download.bin".into());
        let mut name = suggested.filter(|s| !s.trim().is_empty()).map(|s| kestrel_engine::sanitize_file_name(&s)).unwrap_or(fallback);
        let mut probe = Probe { url: url.trim().to_string(), file_name: name.clone(), total: None, resumable: false, content_type: None, category: category_for(&name).into(), folder: String::new(), error: None, final_url: None };
        match parsed {
            Ok(parsed) if matches!(parsed.scheme(), "http" | "https" | "ftp" | "ftps" | "sftp") => {
                let mut job = Job::new(parsed, std::env::temp_dir().join("kestrel-probe"));
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
                        probe.final_url = Some(info.final_url).filter(|f| f != url.trim());
                    }
                    Ok(Err(error)) => probe.error = Some(error.message),
                    Err(_) => probe.error = Some("The server did not answer in time; the size will be known when the download starts.".into()),
                }
            }
            _ => probe.error = Some("Enter a full address starting with http://, https://, ftp:// or sftp://.".into()),
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
            url = Url::parse(&kestrel_media::pages::canonicalize(url.as_str()).ok_or("Use a video page or an HLS/DASH stream address.")?).map_err(|e| e.to_string())?;
        }
        // FTP and SFTP: a user name and password in the address move into the download's session,
        // so the address shown in the list carries no password.
        let mut request = request;
        if matches!(url.scheme(), "ftp" | "ftps" | "sftp") && !url.username().is_empty() {
            use base64::Engine as _;
            let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", decode(url.username()), decode(url.password().unwrap_or(""))));
            let _ = url.set_username("");
            let _ = url.set_password(None);
            let mut session = request.session.take().unwrap_or_else(|| Session::for_url(&url));
            session.authorization = Some(format!("Basic {token}"));
            request.session = Some(session);
        }
        if !matches!(url.scheme(), "http" | "https" | "ftp" | "ftps" | "sftp") || !url.username().is_empty() {
            return Err("Only HTTP, HTTPS, FTP and SFTP links without a user name in them are supported.".into());
        }
        if let Some(request_id) = &request.request_id
            && let Some(existing) = self.records.lock().iter().find(|r| r.request_id.as_ref() == Some(request_id)) {
                return Ok(existing.id.clone());
            }
        if let Some(session) = &request.session {
            session.validate(&url).map_err(|e| e.message)?;
        }
        let session = self.with_login(&url, request.session.clone());
        let mut name = request.file_name.as_deref().filter(|n| !n.trim().is_empty()).map(kestrel_engine::sanitize_file_name).unwrap_or_else(|| Manager::name_from_url(&url));
        if let Some(selection) = &request.video {
            // The file is named after the format that will be written.
            let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Video".into());
            name = format!("{stem}.{}", selection.extension);
        }
        let rule = self.rule_for(&url, &name);
        // A rule's own User-Agent for this site.
        let session = match rule.as_ref().map(|r| r.user_agent.trim()).filter(|ua| !ua.is_empty() && !ua.chars().any(char::is_control)) {
            Some(ua) => {
                let mut s = session.unwrap_or_else(|| Session { origin: format!("{}/", url.origin().ascii_serialization()), ..Default::default() });
                s.user_agent = Some(ua.to_string());
                Some(s)
            }
            None => session,
        };
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
        let taken_by_a_download = self.records.lock().iter().any(|r| r.destination == plain && r.status != Status::Completed);
        let destination = match self.settings.read().duplicate_action {
            1 if !taken_by_a_download => plain,
            2 if plain.exists() => return Err(format!("{name} is already in {}; it was not downloaded again.", folder.display())),
            _ => self.unique(&folder, &name),
        };
        let settings = self.settings.read().clone();
        let start = request.start.clone().unwrap_or_else(|| "now".into());
        let queue = request.queue.clone().or_else(|| rule_text(|r| &r.queue)).filter(|q| settings.queues.iter().any(|x| &x.name == q)).unwrap_or_else(|| MAIN_QUEUE.into());
        let id = format!("{:016x}{:04x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64, rand_u16());
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
        self.records.lock().push(record);
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

    /// A torrent is saved under its own name in the chosen folder (it continues there after a pause).
    pub(super) fn add_torrent(self: &Arc<Self>, request: AddRequest, selection: kestrel_torrent::Selection) -> Result<String, String> {
        let url = kestrel_torrent::magnet(&selection.info_hash, &selection.name);
        selection.validate(&url)?;
        if let Some(existing) = self.records.lock().iter().find(|r| matches!(&r.kind, Kind::Torrent { selection: s } if s.info_hash == selection.info_hash) && r.status != Status::Completed) {
            return Err(format!("This torrent is already in the list as {}.", existing.name()));
        }
        let folder = request.folder.as_ref().map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| self.settings.read().download_dir.join("Torrents"));
        std::fs::create_dir_all(&folder).map_err(|e| format!("The folder could not be created: {e}"))?;
        let settings = self.settings.read().clone();
        let start = request.start.clone().unwrap_or_else(|| "now".into());
        let id = format!("{:016x}{:04x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64, rand_u16());
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
        self.records.lock().push(record);
        self.save(&id);
        self.emit(&id);
        if start == "now" { self.run(&id) } else { self.schedule() }
        Ok(id)
    }
}
