//! What the windows call (Tauri commands).

use super::*;


#[tauri::command]
pub(crate) fn list_downloads(manager: Mgr) -> Vec<View> {
    manager.views()
}
#[tauri::command]
pub(crate) fn get_download(manager: Mgr, id: String) -> Option<View> {
    manager.view(&id)
}
#[tauri::command]
pub(crate) fn get_settings(manager: Mgr) -> Settings {
    manager.settings()
}
#[tauri::command]
pub(crate) fn save_settings(app: AppHandle, manager: Mgr, settings: Settings) -> Result<(), String> {
    system::set_autostart(settings.start_with_windows);
    manager.save_settings(settings)?;
    let _ = app.emit("message", "Options saved.");
    Ok(())
}
#[tauri::command]
pub(crate) async fn offer_url(app: AppHandle, url: String, file_name: Option<String>, login: Option<String>, password: Option<String>) -> Result<Option<String>, String> {
    let url = url.trim().to_string();
    let lower = url.to_ascii_lowercase();
    if !(["http://", "https://", "ftp://", "ftps://", "sftp://"].iter().any(|p| lower.starts_with(p)) || kestrel_torrent::magnet_hash(&url).is_some()) {
        return Err("Enter a full address starting with http://, https://, ftp:// or sftp://, or a magnet link.".into());
    }
    // "Use authorization": the login goes with this download only, as HTTP Basic authentication.
    let session = match login.filter(|l| !l.is_empty()) {
        Some(login) => {
            use base64::Engine;
            let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
            let mut session = kestrel_engine::Session::for_url(&parsed);
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{login}:{}", password.unwrap_or_default()));
            session.authorization = Some(format!("Basic {token}"));
            Some(session)
        }
        None => None,
    };
    offer(&app, PendingRequest { url, file_name, session, ..Default::default() }, false)
}
/// The start dialog's "Preview": the files inside a .zip that is still on the server.
#[tauri::command]
pub(crate) async fn zip_preview(windows: State<'_, Windows>, id: String) -> Result<Vec<zippreview::Entry>, String> {
    let request = windows.pending.lock().get(&id).map(|p| p.request.clone()).ok_or("This download was already added or cancelled.")?;
    zippreview::list(&request.url, request.session.as_ref()).await
}
/// The start dialog: the logo of a program that is about to be downloaded, read from the server.
#[tauri::command]
pub(crate) async fn pending_icon(windows: State<'_, Windows>, id: String) -> Result<Option<String>, String> {
    let (request, probe) = windows.pending.lock().get(&id).map(|p| (p.request.clone(), p.probe.clone())).ok_or("This download was already added or cancelled.")?;
    // The address the file is really at (after a redirect or a download page).
    let address = probe.and_then(|p| p.final_url).unwrap_or(request.url);
    use base64::Engine;
    Ok(exeicon::remote(&address, request.session.as_ref()).await.map(|ico| format!("data:image/x-icon;base64,{}", base64::engine::general_purpose::STANDARD.encode(ico))))
}
/// The icon Windows shows for a type of file (WinRAR's for .rar…), by its name: like Explorer and MDM.
#[tauri::command]
pub(crate) async fn type_icon(name: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || fileicon::data_url(std::path::Path::new(&name), false)).await.map_err(|e| e.to_string())
}
/// "Add the browser extension": the browsers found, their extensions page, the extension folder.
#[tauri::command]
pub(crate) fn extension_setup() -> serde_json::Value {
    serde_json::json!({
        "browsers": browsers::installed(),
        "folder": browsers::extension_folder().map(|p| p.to_string_lossy().into_owned()),
        "firefoxManifest": browsers::firefox_extension_folder().map(|p| p.join("manifest.json").to_string_lossy().into_owned()),
    })
}
#[tauri::command]
pub(crate) fn open_extensions_page(browser: String) -> Result<(), String> {
    browsers::open_extensions_page(&browser)
}
/// The file's own icon for the list (a program's logo once it is downloaded), as a data URL.
#[tauri::command]
pub(crate) async fn file_icon(manager: Mgr<'_>, id: String) -> Result<Option<String>, String> {
    let Some(record) = manager.record(&id) else { return Ok(None) };
    let done = record.status == model::Status::Completed && record.destination.exists();
    let is_exe = record.destination.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    // A program not downloaded yet: its logo read from the server (like IDM), kept on disk.
    if !done && is_exe && matches!(record.kind, model::Kind::Http) {
        use base64::Engine;
        let url = |bytes: &[u8]| format!("data:image/x-icon;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
        let saved = kestrel_store::default_directory().join("icons").join(format!("{id}.ico"));
        if let Ok(bytes) = std::fs::read(&saved) {
            return Ok(Some(url(&bytes)));
        }
        static TRIED: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());
        if !TRIED.lock().contains(&id) {
            TRIED.lock().push(id.clone());
            let address = record.resume.final_url.clone().unwrap_or_else(|| record.url.clone());
            if let Some(ico) = exeicon::remote(&address, record.session.as_ref()).await {
                let _ = std::fs::create_dir_all(saved.parent().unwrap());
                let _ = std::fs::write(&saved, &ico);
                return Ok(Some(url(&ico)));
            }
        }
    }
    let path = record.destination.clone();
    tauri::async_runtime::spawn_blocking(move || fileicon::data_url(&path, done)).await.map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) fn move_in_queue(manager: Mgr, id: String, delta: i32) {
    manager.move_in_queue(&id, delta)
}
#[tauri::command]
pub(crate) fn get_pending(windows: State<'_, Windows>, id: String) -> Option<Pending> {
    windows.pending.lock().get(&id).cloned()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartChoice {
    pending_id: String,
    file_name: String,
    folder: String,
    category: String,
    description: String,
    /// "now", "later" or "queue"
    start: String,
    queue: Option<String>,
    /// Remember the folder for this category (1.x's "Remember this path").
    #[serde(default)]
    remember: bool,
    /// The SHA-256 the site publishes: the finished file is checked against it.
    #[serde(default)]
    sha256: Option<String>,
}
#[tauri::command]
pub(crate) async fn confirm_pending(app: AppHandle, manager: Mgr<'_>, windows: State<'_, Windows>, choice: StartChoice) -> Result<String, String> {
    let pending = windows.pending.lock().get(&choice.pending_id).cloned().ok_or("This download was already added or cancelled.")?;
    if choice.remember && std::path::Path::new(&choice.folder).is_absolute() && manager.folder_for(&choice.category) != std::path::Path::new(&choice.folder) {
        let mut settings = manager.settings();
        settings.category_dirs.insert(choice.category.clone(), choice.folder.clone().into());
        manager.save_settings(settings)?;
    }
    let start = if choice.start == "queue" { "queue".to_string() } else { choice.start.clone() };
    let id = manager.add(AddRequest {
        url: pending.request.url,
        file_name: Some(choice.file_name),
        folder: Some(choice.folder),
        category: Some(choice.category),
        description: Some(choice.description),
        start: Some(start.clone()),
        queue: choice.queue,
        session: pending.request.session,
        request_id: pending.request.request_id,
        total: pending.probe.and_then(|p| p.total),
        video: pending.request.video,
        expected_sha256: choice.sha256.map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()),
        ..Default::default()
    })?;
    windows.pending.lock().remove(&choice.pending_id);
    if start == "now" && manager.settings.read().show_progress_window {
        open_progress(&app, &id);
    }
    Ok(id)
}
#[tauri::command]
pub(crate) fn cancel_pending(windows: State<'_, Windows>, id: String) {
    windows.pending.lock().remove(&id);
}

/// The video picker's list: one video's qualities or a playlist's entries.
#[tauri::command]
pub(crate) async fn probe_video(manager: Mgr<'_>, windows: State<'_, Windows>, id: String) -> Result<kestrel_media::Probe, String> {
    let request = windows.pending.lock().get(&id).map(|p| p.request.clone()).ok_or("This download was already added or cancelled.")?;
    let session = request.session.clone().filter(|s| url::Url::parse(&request.url).is_ok_and(|u| s.validate(&u).is_ok()));
    let cancel = tokio_util::sync::CancellationToken::new();
    let found = match tokio::time::timeout(std::time::Duration::from_secs(150), manager.media.probe(&request.url, session.as_ref(), &cancel)).await {
        Ok(Ok(found)) => found,
        Ok(Err(error)) => return Err(if error.is_empty() { "Cancelled.".into() } else { error }),
        Err(_) => { cancel.cancel(); return Err("The video site did not answer in time.".into()); }
    };
    if let Some(pending) = windows.pending.lock().get_mut(&id) {
        pending.video = Some(found.clone());
    }
    Ok(found)
}
#[tauri::command]
pub(crate) async fn video_exact_cut(manager: Mgr<'_>) -> Result<bool, String> {
    Ok(manager.media.supports_exact_cut().await)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VideoChoice {
    pending_id: String,
    /// One video: the chosen quality from the list.
    format_id: Option<String>,
    clip_start: Option<f64>,
    clip_end: Option<f64>,
    #[serde(default)]
    clip_exact: bool,
    /// A playlist: the videos to fetch, a height limit (none = best) or audio only.
    #[serde(default)]
    entries: Vec<String>,
    max_height: Option<u32>,
    #[serde(default)]
    audio_only: bool,
    #[serde(default)]
    subfolder: bool,
    /// Subtitle languages ("en,fr"), saved beside the video.
    subtitles: Option<String>,
    /// The audio track's language, SponsorBlock, chapters.
    audio_language: Option<String>,
    #[serde(default)]
    sponsorblock: bool,
    #[serde(default)]
    chapters: bool,
    /// "now" or "later"
    start: String,
}
impl VideoChoice {
    fn apply(&self, selection: &mut kestrel_media::Selection) {
        selection.audio_language = self.audio_language.clone().map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
        selection.sponsorblock = self.sponsorblock;
        selection.chapters = self.chapters && !selection.audio_only;
    }
}
pub(crate) fn clock_name(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 { format!("{}.{:02}.{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}.{:02}", s / 60, s % 60) }
}
#[tauri::command]
pub(crate) async fn confirm_video(app: AppHandle, manager: Mgr<'_>, windows: State<'_, Windows>, choice: VideoChoice) -> Result<String, String> {
    let pending = windows.pending.lock().get(&choice.pending_id).cloned().ok_or("This download was already added or cancelled.")?;
    let found = pending.video.clone().ok_or("The quality list is not ready yet.")?;
    if let Some(playlist) = found.playlist {
        let mut preset = kestrel_media::Selection::preset(choice.max_height, choice.audio_only);
        preset.subtitles = choice.subtitles.clone().filter(|_| !choice.audio_only);
        choice.apply(&mut preset);
        let category = if choice.audio_only { "Music" } else { "Video" };
        let mut folder = manager.folder_for(category);
        if choice.subfolder {
            folder = folder.join(kestrel_engine::sanitize_file_name(&playlist.title).trim_end_matches('.'));
        }
        let mut added = 0;
        let mut last_error = None;
        for entry in playlist.entries.iter().filter(|e| choice.entries.contains(&e.url)) {
            let request = AddRequest {
                url: entry.url.clone(),
                file_name: Some(format!("{}.{}", entry.title, preset.extension)),
                folder: Some(folder.to_string_lossy().into_owned()),
                category: Some(category.into()),
                description: Some(playlist.title.clone()),
                start: Some(if choice.start == "now" { "queue".into() } else { "later".into() }),
                video: Some(preset.clone()),
                ..Default::default()
            };
            match manager.add(request) {
                Ok(_) => added += 1,
                Err(error) => last_error = Some(format!("{}: {error}", entry.title)),
            }
        }
        windows.pending.lock().remove(&choice.pending_id);
        // Many videos at once go through the main queue rather than all starting together.
        if choice.start == "now" && added > 0 {
            manager.run_waiting(model::MAIN_QUEUE);
        }
        let _ = app.emit("message", format!("{}: added {added} of {} videos{}.", playlist.title, choice.entries.len(), if choice.start == "now" { "" } else { " for later" }));
        return match (added, last_error) {
            (0, Some(error)) => Err(error),
            _ => Ok(String::new()),
        };
    }
    let catalog = found.catalog.ok_or("No video qualities were found.")?;
    let format_id = choice.format_id.clone().ok_or("Choose a quality.")?;
    let mut selection = catalog.formats.iter().find(|f| f.format_id == format_id).cloned().ok_or("This quality is no longer available. Reload the quality list.")?;
    selection.clip_start = choice.clip_start.filter(|s| *s > 0.0);
    selection.clip_end = choice.clip_end;
    selection.clip_exact = choice.clip_exact && selection.is_clip();
    selection.subtitles = choice.subtitles.clone().filter(|_| !selection.audio_only && !selection.is_clip());
    choice.apply(&mut selection);
    selection.validate(&pending.request.url)?;
    let part = if selection.is_clip() { format!(" (part {}-{})", clock_name(selection.clip_start.unwrap_or(0.0)), selection.clip_end.map_or("end".to_string(), clock_name)) } else { String::new() };
    let name = kestrel_engine::sanitize_file_name(&format!("{}{part} ({}).{}", catalog.title, selection.quality(), selection.extension));
    let mut request = pending.request.clone();
    request.file_name = Some(name);
    request.video = Some(selection);
    if choice.start == "later" {
        windows.pending.lock().remove(&choice.pending_id);
        return manager.add(AddRequest { url: request.url, file_name: request.file_name, session: request.session, request_id: request.request_id, video: request.video, start: Some("later".into()), ..Default::default() });
    }
    windows.pending.lock().remove(&choice.pending_id);
    Ok(offer(&app, request, false)?.unwrap_or_default())
}
/// Reads a torrent's file list (from peers, for a magnet link; at most 3 minutes).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TorrentOffer {
    info: kestrel_torrent::Info,
    folder: String,
}
#[tauri::command]
pub(crate) async fn load_torrent(manager: Mgr<'_>, windows: State<'_, Windows>, id: String) -> Result<TorrentOffer, String> {
    let source = windows.pending.lock().get(&id).map(|p| p.request.url.clone()).ok_or("This download was already added or cancelled.")?;
    let cancel = tokio_util::sync::CancellationToken::new();
    let info = match tokio::time::timeout(std::time::Duration::from_secs(180), manager.torrents.load(&source, &cancel)).await {
        Ok(result) => result?,
        Err(_) => {
            cancel.cancel();
            return Err("No one shared this torrent's file list within 3 minutes. Try again later, or use a .torrent file.".into());
        }
    };
    if let Some(pending) = windows.pending.lock().get_mut(&id) {
        pending.torrent = Some(info.clone());
    }
    let folder = manager.settings.read().download_dir.join("Torrents").to_string_lossy().into_owned();
    Ok(TorrentOffer { info, folder })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TorrentChoice {
    pending_id: String,
    /// 1-based indexes of the chosen files.
    files: Vec<usize>,
    folder: String,
    #[serde(default)]
    seed_minutes: u32,
    /// "now" or "later"
    start: String,
}
#[tauri::command]
pub(crate) async fn confirm_torrent(app: AppHandle, manager: Mgr<'_>, windows: State<'_, Windows>, choice: TorrentChoice) -> Result<String, String> {
    let pending = windows.pending.lock().get(&choice.pending_id).cloned().ok_or("This download was already added or cancelled.")?;
    let info = pending.torrent.ok_or("The torrent's file list is not ready yet.")?;
    let chosen: Vec<&kestrel_torrent::TorrentFile> = info.files.iter().filter(|f| choice.files.contains(&f.index)).collect();
    if chosen.is_empty() {
        return Err("Choose at least one file.".into());
    }
    if !std::path::Path::new(&choice.folder).is_absolute() {
        return Err("Choose a folder to save the torrent in.".into());
    }
    let selection = kestrel_torrent::Selection {
        info_hash: info.info_hash.clone(),
        name: info.name.clone(),
        metadata_file: kestrel_torrent::metadata_path(&manager.torrents.cache, &info.info_hash),
        // Every file chosen: no list, so the whole torrent is taken.
        files: if chosen.len() == info.files.len() { Vec::new() } else { chosen.iter().map(|f| f.index).collect() },
        selected_bytes: chosen.iter().map(|f| f.length).sum(),
        seed_minutes: choice.seed_minutes.min(1440),
    };
    let id = manager.add(AddRequest { url: kestrel_torrent::magnet(&info.info_hash, &info.name), folder: Some(choice.folder), start: Some(choice.start.clone()), request_id: pending.request.request_id, torrent: Some(selection), ..Default::default() })?;
    windows.pending.lock().remove(&choice.pending_id);
    if choice.start == "now" && manager.settings.read().show_progress_window {
        open_progress(&app, &id);
    }
    Ok(id)
}
#[tauri::command]
pub(crate) async fn open_torrent_file(app: AppHandle, path: String) -> Result<(), String> {
    if !is_torrent_source(&path) {
        return Err("Choose a .torrent file.".into());
    }
    offer(&app, PendingRequest { url: path, ..Default::default() }, false).map(|_| ())
}

/// The page is not a video after all: download the address as a file.
#[tauri::command]
pub(crate) async fn video_as_file(app: AppHandle, windows: State<'_, Windows>, id: String) -> Result<(), String> {
    let pending = windows.pending.lock().remove(&id).ok_or("This download was already added or cancelled.")?;
    let mut request = pending.request;
    request.as_file = true;
    request.video = None;
    offer(&app, request, true).map(|_| ())
}
/// Tasks > Export: the addresses, one per line, or as an IDM export file (.ef2).
#[tauri::command]
pub(crate) fn write_link_list(path: String, urls: Vec<String>, idm: bool) -> Result<(), String> {
    let text: String = if idm { urls.iter().map(|u| format!("<\r\n{u}\r\n>\r\n")).collect() } else { urls.iter().map(|u| format!("{u}\r\n")).collect() };
    std::fs::write(&path, text).map_err(|e| e.to_string())
}
/// Tasks > Import: the download links found in a text or IDM export file.
#[tauri::command]
pub(crate) fn read_link_list(path: String) -> Result<Vec<String>, String> {
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > 16 * 1024 * 1024 {
        return Err("That file is too large to be a list of links.".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(links_in(&String::from_utf8_lossy(&bytes)))
}
pub(crate) fn links_in(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    text.split_whitespace()
        .filter(|w| { let l = w.to_ascii_lowercase(); ["http://", "https://", "ftp://", "ftps://", "sftp://", "magnet:?"].iter().any(|p| l.starts_with(p)) })
        .filter(|w| seen.insert(w.to_string()))
        .map(str::to_string)
        .collect()
}
#[tauri::command]
pub(crate) fn add_downloads(manager: Mgr, urls: Vec<String>, start: String) -> Vec<Result<String, String>> {
    urls.into_iter().map(|url| manager.add(AddRequest { url, start: Some(start.clone()), ..Default::default() })).collect()
}
#[tauri::command]
pub(crate) fn pause_downloads(manager: Mgr, ids: Vec<String>) {
    manager.pause(&ids)
}
#[tauri::command]
pub(crate) fn resume_downloads(manager: Mgr, ids: Vec<String>) {
    manager.resume(&ids)
}
#[tauri::command]
pub(crate) fn pause_all(manager: Mgr) {
    manager.pause_all()
}
#[tauri::command]
pub(crate) fn resume_all(manager: Mgr) {
    manager.resume_all()
}
#[tauri::command]
pub(crate) fn remove_downloads(manager: Mgr, ids: Vec<String>, delete_files: bool) -> Vec<String> {
    manager.remove(&ids, delete_files)
}
#[tauri::command]
pub(crate) fn remove_completed(manager: Mgr) -> Vec<String> {
    manager.remove_completed()
}
#[tauri::command]
pub(crate) fn rename_download(manager: Mgr, id: String, name: String) -> Result<(), String> {
    manager.rename(&id, &name)
}
#[tauri::command]
pub(crate) fn redownload(manager: Mgr, id: String) -> Result<String, String> {
    manager.redownload(&id)
}
#[tauri::command]
pub(crate) fn set_speed_limit(manager: Mgr, id: String, bytes_per_second: u64) {
    manager.set_speed_limit(&id, bytes_per_second)
}
#[tauri::command]
pub(crate) fn set_queue(manager: Mgr, ids: Vec<String>, queue: String) {
    manager.set_queue(&ids, &queue)
}
#[tauri::command]
pub(crate) fn enqueue(manager: Mgr, ids: Vec<String>) {
    manager.enqueue(&ids)
}
#[tauri::command]
pub(crate) fn start_queue(manager: Mgr, name: String) {
    manager.start_queue(&name)
}
#[tauri::command]
pub(crate) fn run_waiting(manager: Mgr, name: String) {
    manager.run_waiting(&name)
}
#[tauri::command]
pub(crate) fn stop_queue(manager: Mgr, name: String) {
    manager.stop_queue(&name)
}
#[tauri::command]
pub(crate) fn running_queues(manager: Mgr) -> Vec<String> {
    manager.running_queues.lock().iter().cloned().collect()
}
#[tauri::command]
pub(crate) fn open_file(manager: Mgr, id: String) -> Result<(), String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    if !record.destination.exists() {
        return Err("The file is no longer there.".into());
    }
    system::open_path(&record.destination)
}
#[tauri::command]
pub(crate) fn show_in_folder(manager: Mgr, id: String) -> Result<(), String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    system::reveal(&record.destination)
}
#[tauri::command]
pub(crate) fn open_folder(path: String) -> Result<(), String> {
    system::open_path(std::path::Path::new(&path))
}
#[tauri::command]
pub(crate) async fn show_progress(app: AppHandle, id: String) -> Result<(), String> {
    open_progress(&app, &id);
    Ok(())
}
#[tauri::command]
pub(crate) async fn show_window(app: AppHandle, name: String) -> Result<(), String> {
    match name.as_str() {
        "options" => open_window(&app, "options", "Options", 720.0, 600.0, true),
        "scheduler" => open_window(&app, "scheduler", "Scheduler", 700.0, 580.0, true),
        "batch" => open_window(&app, "batch", "Add batch download", 560.0, 420.0, true),
        "grabber" => open_window(&app, "grabber", "Site grabber", 640.0, 540.0, true),
        "extension" => open_window(&app, "extension", "Add the browser extension", 600.0, 470.0, false),
        _ => show_main(&app),
    }
    Ok(())
}
#[tauri::command]
pub(crate) fn power_action(app: AppHandle, action: AfterQueue) -> Result<(), String> {
    if action == AfterQueue::Exit {
        app.state::<Shared>().shutdown();
        app.exit(0);
        return Ok(());
    }
    app.state::<Shared>().shutdown();
    system::power(action)
}
#[tauri::command]
pub(crate) fn exit_app(app: AppHandle) {
    app.state::<Shared>().shutdown();
    app.exit(0);
}
/// Registers this copy's browser bridge for Chrome, Edge and Brave (the installer does it too).
#[tauri::command]
pub(crate) async fn register_browsers() -> Result<String, String> {
    let host = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name(format!("kestrel-nmhost{}", std::env::consts::EXE_SUFFIX));
    if !host.exists() {
        return Err("kestrel-nmhost.exe is missing beside Kestrel.".into());
    }
    let mut command = std::process::Command::new(host);
    command.arg("--register");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(if output.status.success() { &output.stdout } else { &output.stderr }).trim().to_string();
    if output.status.success() { Ok(text) } else { Err(text) }
}
/// Options > General > Test browser integration: is each browser connected to this Kestrel?
#[derive(Serialize)]
pub(crate) struct BrowserCheck {
    browser: String,
    ok: bool,
    detail: String,
}
#[tauri::command]
pub(crate) fn test_browsers() -> Vec<BrowserCheck> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let host = std::env::current_exe().map(|e| e.with_file_name("kestrel-nmhost.exe")).unwrap_or_default();
        [("Google Chrome", r"Google\Chrome"), ("Microsoft Edge", r"Microsoft\Edge"), ("Brave", r"BraveSoftware\Brave-Browser"), ("Mozilla Firefox", "Mozilla")]
            .into_iter()
            .map(|(name, key)| {
                let key = format!(r"HKCU\Software\{key}\NativeMessagingHosts\com.swiftfetch.bridge");
                let manifest = std::process::Command::new("reg").args(["query", &key, "/ve"]).creation_flags(0x0800_0000).output().ok()
                    .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().find(|l| l.contains("REG_SZ")).and_then(|l| l.split("REG_SZ").nth(1)).map(|v| std::path::PathBuf::from(v.trim())));
                let (ok, detail) = match manifest {
                    None => (false, "Not connected: use Connect browsers.".to_string()),
                    Some(m) if !m.is_file() => (false, "Connected to a copy that is gone: use Connect browsers.".to_string()),
                    Some(m) => {
                        let path = std::fs::read_to_string(&m).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["path"].as_str().map(std::path::PathBuf::from));
                        match path {
                            Some(p) if p == host => (true, "Connected to this Kestrel.".to_string()),
                            Some(p) if p.is_file() => (true, format!("Connected to another copy: {}", p.display())),
                            _ => (false, "Its bridge program is missing: use Connect browsers.".to_string()),
                        }
                    }
                };
                BrowserCheck { browser: name.into(), ok, detail }
            })
            .collect()
    }
    #[cfg(not(windows))]
    Vec::new()
}
/// Site grabber: links to files of the given types on one site.
#[tauri::command]
pub(crate) async fn grab_site(manager: Mgr<'_>, url: String, extensions: String) -> Result<Vec<String>, String> {
    let root = url::Url::parse(url.trim()).map_err(|_| "Enter a full address starting with http:// or https://.".to_string())?;
    let session = manager.with_login(&root, None);
    manager.engine.grab(&root, &extensions, session.as_ref()).await
}
#[tauri::command]
pub(crate) async fn check_updates(app: AppHandle) -> Result<updates::UpdateInfo, String> {
    updates::check(&app).await.inspect_err(|e| applog::warn(format!("Update check failed: {e}")))
}
#[tauri::command]
pub(crate) async fn install_update(app: AppHandle) -> Result<(), String> {
    applog::info("Installing an update");
    updates::install(&app).await.inspect_err(|e| applog::error(format!("Update failed: {e}")))
}
/// Help > Open log.
#[tauri::command]
pub(crate) fn open_log() -> Result<(), String> {
    if !applog::path().exists() {
        applog::info("Log opened");
    }
    system::open_path(&applog::path())
}
/// What the windows need to drag a finished file out of Kestrel (to Explorer, a mail, a chat).
#[derive(Serialize)]
pub(crate) struct DragItem {
    path: String,
    icon: String,
}
#[tauri::command]
pub(crate) fn drag_item(manager: Mgr, id: String) -> Result<DragItem, String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    if record.status != model::Status::Completed || !record.destination.exists() {
        return Err("Only a finished file that is still there can be dragged.".into());
    }
    Ok(DragItem { path: record.destination.to_string_lossy().into_owned(), icon: desktop::drag_icon()?.to_string_lossy().into_owned() })
}
#[tauri::command]
pub(crate) fn crash_reports() -> Vec<crash::Report> {
    crash::pending()
}
#[tauri::command]
pub(crate) fn dismiss_crash(id: String) -> Result<(), String> {
    crash::dismiss(&id)
}
/// Opens the report as a new GitHub issue in the browser (the user reads it and sends it there).
#[tauri::command]
pub(crate) fn report_crash(id: String) -> Result<(), String> {
    let url = crash::issue_url(&id)?;
    system::open_path(std::path::Path::new(&url))?;
    crash::dismiss(&id)
}
/// The progress window's green arrow: the download goes on in the background, in the tray.
#[tauri::command]
pub(crate) async fn to_background(app: AppHandle, id: String) {
    tray_state::send(&app, &id);
}
#[tauri::command]
pub(crate) fn set_after_complete(windows: State<'_, Windows>, id: String, action: AfterQueue) {
    let mut after = windows.after.lock();
    if action == AfterQueue::Nothing { after.remove(&id); } else { after.insert(id, action); }
}
#[tauri::command]
pub(crate) fn get_after_complete(windows: State<'_, Windows>, id: String) -> AfterQueue {
    windows.after.lock().get(&id).copied().unwrap_or_default()
}
/// Windows' "Open with" chooser for a finished file.
#[tauri::command]
pub(crate) fn open_with(manager: Mgr, id: String) -> Result<(), String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    if !record.destination.exists() {
        return Err("The file is no longer there.".into());
    }
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32.exe").arg("shell32.dll,OpenAs_RunDLL").arg(&record.destination).spawn().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(windows))]
    system::open_path(&record.destination)
}
#[tauri::command]
pub(crate) fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}
/// Context menu > Refresh address.
#[tauri::command]
pub(crate) async fn refresh_address(manager: Mgr<'_>, id: String, url: String) -> Result<(), String> {
    manager.inner().clone().refresh_address(&id, &url).await
}
/// Several downloads renamed with a pattern ({name} {ext} {n} {date} {site}).
#[tauri::command]
pub(crate) fn rename_with_pattern(manager: Mgr, ids: Vec<String>, pattern: String) -> Vec<Result<String, String>> {
    if pattern.trim().is_empty() {
        return vec![Err("Write a name pattern.".into())];
    }
    manager.rename_many(&ids, pattern.trim())
}
/// The finished file's checksum ("md5", "sha1" or "sha256"), lowercase hex.
#[tauri::command]
pub(crate) async fn file_checksum(manager: Mgr<'_>, id: String, algorithm: String) -> Result<String, String> {
    let record = manager.record(&id).ok_or("Unknown download.")?;
    if record.status != model::Status::Completed || !record.destination.is_file() {
        return Err("The file is not there (not finished, moved or deleted).".into());
    }
    tokio::task::spawn_blocking(move || checksum_of(&record.destination, &algorithm)).await.map_err(|e| e.to_string())?
}
pub(crate) fn checksum_of(path: &std::path::Path, algorithm: &str) -> Result<String, String> {
    use sha2::Digest;
    use std::io::Read;
    fn run<D: sha2::Digest>(mut digest: D, path: &std::path::Path) -> Result<String, String> {
        let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 { break; }
            digest.update(&buffer[..n]);
        }
        Ok(digest.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }
    match algorithm {
        "md5" => run(md5::Md5::new(), path),
        "sha1" => run(sha1::Sha1::new(), path),
        "sha256" => run(sha2::Sha256::new(), path),
        _ => Err("Unknown checksum type.".into()),
    }
}
/// File > Export settings: everything except passwords.
#[tauri::command]
pub(crate) fn export_settings(manager: Mgr, path: String) -> Result<(), String> {
    let settings = manager.settings.read().for_export();
    let json = serde_json::json!({ "kestrelSettings": 1, "settings": settings });
    std::fs::write(&path, serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
/// File > Import settings. Passwords are not in exports: the ones saved here are kept.
#[tauri::command]
pub(crate) fn import_settings(app: AppHandle, manager: Mgr, path: String) -> Result<(), String> {
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| "This is not a Kestrel settings file.".to_string())?;
    if value.get("kestrelSettings").is_none() {
        return Err("This is not a Kestrel settings file.".into());
    }
    let mut imported: Settings = serde_json::from_value(value["settings"].clone()).map_err(|e| format!("The settings file is damaged: {e}"))?;
    let current = manager.settings.read().clone();
    if imported.proxy_password.is_empty() && imported.proxy == current.proxy {
        imported.proxy_password = current.proxy_password.clone();
    }
    for login in &mut imported.site_logins {
        if login.password.is_empty()
            && let Some(saved) = current.site_logins.iter().find(|l| l.origin == login.origin && l.username == login.username) {
                login.password = saved.password.clone();
            }
    }
    imported.last_update_check_ms = current.last_update_check_ms;
    imported.extension_guide_shown = current.extension_guide_shown;
    system::set_autostart(imported.start_with_windows);
    manager.save_settings(imported)?;
    let _ = app.emit("message", "Settings imported.");
    Ok(())
}

#[cfg(test)]
mod checksum_tests {
    #[test]
    fn known_checksums() {
        let path = std::env::temp_dir().join(format!("kestrel-sum-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(super::checksum_of(&path, "md5").unwrap(), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(super::checksum_of(&path, "sha1").unwrap(), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(super::checksum_of(&path, "sha256").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_file(path);
    }
}
