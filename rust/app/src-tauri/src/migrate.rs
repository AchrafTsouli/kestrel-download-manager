//! First start after Flux 1.x: its settings and download list come along. 1.x's history is read,
//! never changed (1.x may still be installed beside 2.0 during the beta).
//!
//! Unfinished 1.x downloads kept one part file per connection; they are joined once into 2.0's
//! single `.flux.part` so the download continues where it stopped. Videos keep their parts folder
//! and torrents their saved file list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flux_engine::{Resume, Session, file::{PartFile, part_path}};
use serde_json::{Map, Value};

use crate::model::{AfterQueue, Kind, MAIN_QUEUE, Queue, Record, Settings, Status};

/// 1.x wrote .NET property names (PascalCase); 2.0's types read camelCase.
fn camel(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let mut chars = k.chars();
                    let key = chars.next().map(|c| c.to_ascii_lowercase().to_string() + chars.as_str()).unwrap_or_default();
                    (key, camel(v))
                })
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(camel).collect()),
        other => other.clone(),
    }
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}
fn number(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|n| n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)))
}
fn flag(v: &Value, key: &str, default: bool) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(default)
}
/// A .NET DateTimeOffset ("2026-11-04T12:10:05.1234567+01:00") as Unix milliseconds.
fn millis(v: &Value, key: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(v.get(key)?.as_str()?).ok().map(|t| t.timestamp_millis().max(0) as u64)
}
fn queue_name(name: &str) -> String {
    if name.is_empty() || name == "Default" { MAIN_QUEUE.into() } else { name.into() }
}

pub fn settings(v1: &Value) -> Settings {
    let mut s = Settings::default();
    if let Some(dir) = text(v1, "DownloadDirectory") { s.download_dir = dir.into(); }
    if let Some(n) = number(v1, "DefaultConnections") { s.connections = (n as usize).clamp(1, 32); }
    s.speed_limit = number(v1, "GlobalSpeedLimitBytes").unwrap_or(0);
    s.show_start_dialog = flag(v1, "ShowStartDownloadDialog", s.show_start_dialog);
    s.show_progress_window = flag(v1, "ShowProgressOnStart", s.show_progress_window);
    s.show_complete_dialog = flag(v1, "ShowDownloadCompleteDialog", s.show_complete_dialog);
    s.theme = if flag(v1, "DarkMode", true) { "dark".into() } else { "light".into() };
    if let Some(language) = text(v1, "Language") { s.language = language; }
    s.monitor_clipboard = flag(v1, "MonitorClipboard", s.monitor_clipboard);
    s.play_sounds = flag(v1, "PlayCompletionSound", s.play_sounds);
    if let Some(n) = number(v1, "MaximumRetries") { s.max_retries = (n as u32).min(20); }
    if let Some(proxy) = text(v1, "ProxyAddress") { s.proxy = proxy; }
    if let Some(user) = text(v1, "ProxyUsername") { s.proxy_username = user; }
    if let Some(password) = text(v1, "ProxyPassword") { s.proxy_password = password; }
    // 1.x stored its stock (now dated) Chrome identity here; keep only one the user wrote.
    if let Some(agent) = text(v1, "ManualUserAgent").filter(|a| !a.contains("Chrome/")) { s.user_agent = agent; }
    if let Some(limits) = v1.get("HostConnectionLimits").and_then(Value::as_object) {
        s.host_limits = limits.iter().filter_map(|(host, n)| Some((host.to_ascii_lowercase(), n.as_u64()? as usize))).collect::<HashMap<_, _>>();
    }
    if let Some(feed) = text(v1, "UpdateFeedUrl") { s.update_feed = feed; }
    s.auto_check_updates = flag(v1, "AutoCheckUpdates", s.auto_check_updates);
    s.auto_update_tools = flag(v1, "AutoUpdateVideoTools", s.auto_update_tools);
    if let Some(extensions) = text(v1, "CaptureExtensions") { s.capture_extensions = extensions; }
    s.capture_enabled = flag(v1, "BrowserCaptureEnabled", s.capture_enabled);
    s.video_panel = flag(v1, "BrowserMediaPanelEnabled", s.video_panel);
    if let Some(sites) = text(v1, "ExcludedSites") { s.excluded_sites = sites; }
    if let Some(scanner) = text(v1, "VirusScanner") { s.virus_scanner = scanner; }
    if let Some(args) = text(v1, "VirusScannerArguments") { s.virus_scanner_args = args; }
    if let Some(profile) = text(v1, "VpnProfile") { s.vpn_profile = profile; }
    s.connect_vpn = flag(v1, "ConnectVpnBeforeDownload", false);
    s.disconnect_vpn_when_idle = flag(v1, "DisconnectVpnWhenIdle", false);
    if let Some(logins) = v1.get("SiteLogins").and_then(Value::as_array) {
        s.site_logins = logins.iter().filter_map(|l| serde_json::from_value(camel(l)).ok()).collect();
    }
    if let Some(rules) = v1.get("Rules").and_then(Value::as_array) {
        s.rules = rules.iter().map(|r| crate::model::Rule {
            name: text(r, "Name").unwrap_or_else(|| "Rule".into()),
            enabled: flag(r, "Enabled", true),
            domain: text(r, "DomainPattern").unwrap_or_default(),
            extensions: text(r, "ExtensionPattern").unwrap_or_default(),
            url_pattern: text(r, "UrlRegex").unwrap_or_default(),
            folder: text(r, "TargetFolder").unwrap_or_default(),
            category: text(r, "Category").unwrap_or_default(),
            queue: text(r, "Queue").map(|q| queue_name(&q)).unwrap_or_default(),
            connections: number(r, "Connections").map(|n| n as usize),
            speed_limit: number(r, "SpeedLimitBytes"),
        }).collect();
    }
    if let Some(queues) = v1.get("Queues").and_then(Value::as_array) {
        // Synchronization queues (checking files for changes) have no 2.0 counterpart yet.
        let mut list: Vec<Queue> = queues
            .iter()
            .filter(|q| !flag(q, "Synchronization", false))
            .map(|q| Queue {
                name: queue_name(&text(q, "Name").unwrap_or_default()),
                max_active: number(q, "MaximumActive").map_or(3, |n| (n as usize).clamp(1, 32)),
                schedule_enabled: flag(q, "Enabled", false),
                start_time: text(q, "StartTime"),
                stop_time: text(q, "StopTime"),
                days: q.get("Weekdays").and_then(Value::as_array).map(|d| d.iter().filter_map(|x| x.as_u64().filter(|n| *n < 7).map(|n| n as u8)).collect()).unwrap_or_default(),
                after: if flag(q, "ExitWhenDone", false) { AfterQueue::Exit } else { AfterQueue::Nothing },
                retries: number(q, "Retries").map_or(3, |n| n.min(20) as u32),
            })
            .collect();
        list.dedup_by(|a, b| a.name == b.name);
        if !list.iter().any(|q| q.name == MAIN_QUEUE) {
            list.insert(0, Queue::default());
        }
        s.queues = list;
    }
    s
}

/// A 1.x download as a 2.0 record, without touching files.
pub fn record(job: &Value, torrent_cache: &Path) -> Option<Record> {
    let id = text(job, "Id")?;
    let url = text(job, "Url")?;
    let destination = PathBuf::from(text(job, "Destination")?);
    let status = match job.get("Status").and_then(Value::as_str).unwrap_or("") {
        "Completed" => Status::Completed,
        "Failed" | "Cancelled" | "AuthRequired" | "ExpiredUrl" | "NoDiskSpace" => Status::Failed,
        _ => Status::Paused,
    };
    let kind = if let Some(video) = job.get("Video").filter(|v| v.is_object()) {
        Kind::Video { selection: serde_json::from_value(camel(video)).ok()? }
    } else if let Some(torrent) = job.get("Torrent").filter(|v| v.is_object()) {
        let mut selection: flux_torrent::Selection = serde_json::from_value(camel(torrent)).ok()?;
        selection.metadata_file = flux_torrent::metadata_path(torrent_cache, &selection.info_hash);
        Kind::Torrent { selection }
    } else {
        Kind::Http
    };
    let total = number(job, "TotalBytes");
    let session = job.get("session").filter(|s| s.is_object()).and_then(|s| serde_json::from_value::<Session>(camel(s)).ok());
    Some(Record {
        id,
        url,
        destination,
        category: text(job, "Category").unwrap_or_else(|| "Other".into()),
        description: text(job, "Description").unwrap_or_default(),
        queue: queue_name(&text(job, "Queue").unwrap_or_default()),
        written: if status == Status::Completed { total.unwrap_or(0) } else { 0 },
        status,
        kind,
        added_ms: millis(job, "CreatedAt").unwrap_or_else(crate::manager::now_ms),
        last_try_ms: millis(job, "StartedAt"),
        completed_ms: millis(job, "CompletedAt"),
        total,
        error: text(job, "Error"),
        connections: number(job, "RequestedConnections").map_or(8, |n| (n as usize).clamp(1, 32)),
        speed_limit: number(job, "SpeedLimitBytes").unwrap_or(0),
        session,
        resume: Resume::default(),
        expected_sha256: text(job, "ExpectedSha256"),
        sha256: text(job, "ActualSha256"),
        request_id: text(job, "BrowserRequestId"),
        retries: 0,
    })
}

/// 1.x's part file for segment `index`.
fn v1_part(job: &Value, destination: &Path, id: &str, index: usize) -> PathBuf {
    match text(job, "TemporaryDirectory") {
        Some(dir) => Path::new(&dir).join(format!("{id}.{index}.part")),
        None => PathBuf::from(format!("{}.{id}.{index}.part", destination.display())),
    }
}

/// Joins an unfinished 1.x file's parts into 2.0's part file and returns where it can continue.
/// The first part is renamed into place (no copy); the others are copied to their offsets.
pub fn join_parts(job: &Value, record: &Record) -> std::io::Result<Option<Resume>> {
    let Some(segments) = job.get("Segments").and_then(Value::as_array).filter(|s| !s.is_empty()) else { return Ok(None) };
    let target = part_path(&record.destination);
    if target.exists() {
        return Ok(None);
    }
    let mut plan = flux_engine::plan::Plan::default();
    let mut parts = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let start = number(segment, "Start").unwrap_or(0);
        let end = number(segment, "End");
        let file = v1_part(job, &record.destination, &record.id, index);
        let on_disk = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        let done = number(segment, "CommittedBytes").unwrap_or(on_disk).min(on_disk);
        plan.segments.push(flux_engine::plan::Segment { start, end, done, active: false });
        parts.push((file, start, done));
    }
    if parts.iter().all(|(_, _, done)| *done == 0) {
        return Ok(None);
    }
    let first_in_place = parts.first().is_some_and(|(file, start, done)| *start == 0 && *done > 0 && std::fs::rename(file, &target).is_ok());
    let part = PartFile::open(&target, record.total, !first_in_place).map_err(|e| std::io::Error::other(e.message))?;
    use std::io::Read;
    for (file, start, done) in parts.iter().skip(usize::from(first_in_place)).filter(|(_, _, d)| *d > 0) {
        let mut input = std::fs::File::open(file)?.take(*done);
        let mut buffer = vec![0u8; 1 << 20];
        let mut offset = *start;
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            part.write_at(offset, &buffer[..read])?;
            offset += read as u64;
        }
    }
    part.sync()?;
    for (file, _, _) in parts.iter().skip(usize::from(first_in_place)) {
        let _ = std::fs::remove_file(file);
    }
    let modified = job.get("LastModified").and_then(Value::as_str).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&chrono::Utc).format("%a, %d %b %Y %H:%M:%S GMT").to_string());
    Ok(Some(Resume { total: record.total, ranged: flag(job, "RangeSupported", false), etag: text(job, "ETag"), last_modified: modified, final_url: text(job, "FinalUrl"), plan }))
}

/// Moves what an unfinished download needs into 2.0's places: the joined part file, the video
/// parts folder, the torrent's saved file list.
pub fn carry_files(job: &Value, record: &mut Record, v1_torrents: &Path) {
    if record.status == Status::Completed {
        return;
    }
    match &record.kind {
        Kind::Http => match join_parts(job, record) {
            Ok(Some(resume)) => {
                record.written = resume.written();
                record.resume = resume;
            }
            // Nothing usable: the download starts again from the beginning.
            _ => record.resume = Resume::default(),
        },
        Kind::Video { .. } => {
            if let Some(work) = text(job, "MediaWorkDirectory").map(PathBuf::from).filter(|w| w.is_dir()) {
                let new = record.destination.parent().unwrap_or(Path::new("")).join(format!(".flux-video-{}", record.id));
                let _ = std::fs::rename(work, new);
            }
        }
        Kind::Torrent { selection } => {
            if let Some(parent) = selection.metadata_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let old = flux_torrent::metadata_path(v1_torrents, &selection.info_hash);
            if !selection.metadata_file.exists() {
                let _ = std::fs::copy(old, &selection.metadata_file);
            }
        }
    }
}

/// Everything 1.x knew, ready for the 2.0 store. `None` when there is no 1.x history.
pub fn import(v1_history: &Path, torrent_cache: &Path) -> Option<(Settings, Vec<Record>)> {
    if !v1_history.exists() {
        return None;
    }
    let history = flux_store::import_v1(v1_history).ok()?;
    let settings = history.settings.as_ref().map(settings).unwrap_or_default();
    let v1_torrents = v1_history.parent().unwrap_or(Path::new("")).join("torrents");
    let mut records = Vec::new();
    for job in &history.downloads {
        if let Some(mut record) = record(job, torrent_cache) {
            if !settings.queues.iter().any(|q| q.name == record.queue) {
                record.queue = MAIN_QUEUE.into();
            }
            carry_files(job, &mut record, &v1_torrents);
            records.push(record);
        }
    }
    Some((settings, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_and_queues_come_along() {
        let v1 = json!({
            "DownloadDirectory": "D:\\Downloads", "DefaultConnections": 6, "GlobalSpeedLimitBytes": 1000, "DarkMode": false,
            "ShowStartDownloadDialog": false, "HostConnectionLimits": { "Files.Example.test": 2 }, "BrowserMediaPanelEnabled": true,
            "Queues": [
                { "Name": "Default", "MaximumActive": 2, "Enabled": true, "StartTime": "02:00", "Weekdays": [1, 2, 3] },
                { "Name": "Synchronization queue", "Synchronization": true },
                { "Name": "Night", "ExitWhenDone": true }
            ]
        });
        let s = settings(&v1);
        assert_eq!(s.download_dir, PathBuf::from("D:\\Downloads"));
        assert_eq!((s.connections, s.speed_limit, s.theme.as_str(), s.show_start_dialog, s.video_panel), (6, 1000, "light", false, true));
        assert_eq!(s.host_limits.get("files.example.test"), Some(&2));
        assert_eq!(s.queues.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(), [MAIN_QUEUE, "Night"]);
        assert_eq!((s.queues[0].max_active, s.queues[0].schedule_enabled, s.queues[0].days.clone()), (2, true, vec![1, 2, 3]));
        assert_eq!(s.queues[1].after, AfterQueue::Exit);
    }

    #[test]
    fn downloads_keep_their_state_session_and_choices() {
        let cache = std::env::temp_dir();
        let job = json!({
            "Id": "0123456789abcdef0123456789abcdef", "Url": "https://files.test/a.zip", "Destination": "C:\\D\\a.zip", "Status": "Downloading",
            "Category": "Compressed", "Queue": "Default", "TotalBytes": 100, "CreatedAt": "2026-11-04T12:10:05.1234567+01:00", "RequestedConnections": 4,
            "session": { "Origin": "https://files.test/", "Cookies": [{ "Name": "a", "Value": "b", "Domain": "files.test", "Path": "/", "HostOnly": true, "Secure": true }] }
        });
        let r = record(&job, &cache).unwrap();
        assert_eq!((r.status, r.queue.as_str(), r.connections, r.total), (Status::Paused, MAIN_QUEUE, 4, Some(100)));
        assert_eq!(r.added_ms, 1_793_790_605_123);
        assert_eq!(r.session.unwrap().cookies[0].name, "a");

        let video = json!({ "Id": "1", "Url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "Destination": "C:\\D\\v.mp4", "Status": "Completed", "TotalBytes": 5,
            "Video": { "FormatId": "137+140", "Extension": "mp4", "Label": "MP4 · 1080p", "ClipStart": 10.0, "ClipEnd": 20.0, "ClipExact": true } });
        let r = record(&video, &cache).unwrap();
        let Kind::Video { selection } = r.kind else { panic!() };
        assert_eq!((selection.format_id.as_str(), selection.clip_end, selection.clip_exact, r.written), ("137+140", Some(20.0), true, 5));

        let hash = "08ada5a7a6183aae1e09d831df6748d566095a10";
        let torrent = json!({ "Id": "2", "Url": format!("magnet:?xt=urn:btih:{hash}&dn=Sintel"), "Destination": "C:\\D\\Sintel", "Status": "Failed",
            "Torrent": { "InfoHash": hash, "Name": "Sintel", "MetadataFile": "C:\\old\\x.torrent", "Files": [2, 6], "SelectedBytes": 3000 } });
        let r = record(&torrent, &cache).unwrap();
        let Kind::Torrent { selection } = r.kind else { panic!() };
        assert_eq!((selection.files.clone(), selection.metadata_file.clone(), r.status), (vec![2, 6], cache.join(format!("{hash}.torrent")), Status::Failed));
    }

    #[test]
    fn unfinished_parts_are_joined_where_they_belong() {
        let dir = std::env::temp_dir().join(format!("flux-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let destination = dir.join("big.bin");
        let id = "ffffffffffffffffffffffffffffffff";
        // Two connections: 0..=9 (6 bytes arrived) and 10..=19 (4 bytes arrived, 3 confirmed).
        std::fs::write(format!("{}.{id}.0.part", destination.display()), b"abcdef").unwrap();
        std::fs::write(format!("{}.{id}.1.part", destination.display()), b"KLMN").unwrap();
        let job = json!({ "Id": id, "Url": "https://files.test/big.bin", "Destination": destination, "Status": "Paused", "TotalBytes": 20,
            "RangeSupported": true, "ETag": "\"v1\"", "LastModified": "2026-01-02T03:04:05+00:00",
            "Segments": [{ "Start": 0, "End": 9, "CommittedBytes": 6 }, { "Start": 10, "End": 19, "CommittedBytes": 3 }] });
        let mut r = record(&job, &dir).unwrap();
        carry_files(&job, &mut r, &dir);
        assert_eq!(r.written, 9);
        assert_eq!(r.resume.etag.as_deref(), Some("\"v1\""));
        assert_eq!(r.resume.last_modified.as_deref(), Some("Fri, 02 Jan 2026 03:04:05 GMT"));
        let joined = std::fs::read(part_path(&destination)).unwrap();
        assert_eq!(joined.len(), 20);
        assert_eq!(&joined[..6], b"abcdef");
        // Only the confirmed bytes of the second part are trusted.
        assert_eq!(&joined[10..14], b"KLM\0");
        assert!(!Path::new(&format!("{}.{id}.1.part", destination.display())).exists(), "the old parts are gone");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod real {
    /// Reads this computer's Flux 1.x history (read-only) and prints only counts.
    #[test]
    #[ignore]
    fn the_installed_1x_history_imports() {
        let path = flux_store::v1_history_path();
        let history = flux_store::import_v1(&path).expect("1.x history readable");
        let records: Vec<_> = history.downloads.iter().filter_map(|j| super::record(j, &std::env::temp_dir())).collect();
        let settings = history.settings.as_ref().map(super::settings).unwrap_or_default();
        println!("1.x downloads: {}, mapped: {}, queues: {}, settings read: {}", history.downloads.len(), records.len(), settings.queues.len(), history.settings.is_some());
        for status in ["Completed", "Paused", "Failed"] {
            println!("  {status}: {}", records.iter().filter(|r| format!("{:?}", r.status) == status).count());
        }
        println!("  videos: {}, torrents: {}", records.iter().filter(|r| matches!(r.kind, crate::model::Kind::Video { .. })).count(), records.iter().filter(|r| matches!(r.kind, crate::model::Kind::Torrent { .. })).count());
        assert_eq!(records.len(), history.downloads.len(), "every 1.x download maps");
    }
}

#[cfg(test)]
mod seed {
    /// Prepares a separate Flux (FLUX_DATA_DIR) for end-to-end runs: no dialogs, downloads into
    /// FLUX_TEST_DOWNLOADS, and no 1.x import.
    #[test]
    #[ignore]
    fn seed_test_profile() {
        let dir = flux_store::default_directory();
        assert!(std::env::var_os("FLUX_DATA_DIR").is_some(), "set FLUX_DATA_DIR");
        let store = flux_store::Store::open(&dir.join("history.db")).unwrap();
        let mut settings = crate::model::Settings::default();
        settings.download_dir = std::env::var_os("FLUX_TEST_DOWNLOADS").map(std::path::PathBuf::from).expect("set FLUX_TEST_DOWNLOADS");
        settings.show_start_dialog = false;
        settings.show_progress_window = false;
        settings.show_complete_dialog = false;
        settings.auto_check_updates = false;
        settings.auto_update_tools = false;
        settings.monitor_clipboard = false;
        store.set("settings", &settings).unwrap();
        store.set("imported-v1", &true).unwrap();
    }
}
