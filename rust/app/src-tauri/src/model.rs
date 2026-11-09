//! What Flux remembers: downloads, settings and queues. Saved by flux-store; sent to the windows as views.

use std::path::{Path, PathBuf};

use flux_engine::{Resume, Session};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Connecting,
    Downloading,
    Retrying,
    Finishing,
    Paused,
    Completed,
    Failed,
}

impl Status {
    pub fn running(self) -> bool {
        matches!(self, Status::Connecting | Status::Downloading | Status::Retrying | Status::Finishing)
    }
    pub fn resumable(self) -> bool {
        matches!(self, Status::Paused | Status::Failed | Status::Queued)
    }
}

/// What kind of transfer a download is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Kind {
    #[default]
    Http,
    /// A video page fetched with yt-dlp; the selection is the quality (and clip) chosen.
    Video { selection: flux_media::Selection },
    /// A torrent through aria2; the selection names its saved file list and the chosen files.
    Torrent { selection: flux_torrent::Selection },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub url: String,
    pub destination: PathBuf,
    pub category: String,
    pub description: String,
    #[serde(default = "main_queue")]
    pub queue: String,
    pub status: Status,
    #[serde(default)]
    pub kind: Kind,
    pub added_ms: u64,
    #[serde(default)]
    pub last_try_ms: Option<u64>,
    #[serde(default)]
    pub completed_ms: Option<u64>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub written: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default = "eight")]
    pub connections: usize,
    #[serde(default)]
    pub speed_limit: u64,
    #[serde(default)]
    pub session: Option<Session>,
    #[serde(default)]
    pub resume: Resume,
    #[serde(default)]
    pub expected_sha256: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    /// Browser request id, so a repeated hand-over never adds the same download twice.
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub retries: u32,
}

fn main_queue() -> String {
    MAIN_QUEUE.into()
}
fn eight() -> usize {
    8
}
pub const MAIN_QUEUE: &str = "Main download queue";

impl Record {
    pub fn name(&self) -> String {
        self.destination.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// What a window shows for one download (no secrets: no session, no resume table).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    pub name: String,
    pub url: String,
    pub path: String,
    pub folder: String,
    pub category: String,
    pub description: String,
    pub queue: String,
    pub status: Status,
    pub kind: &'static str,
    pub written: u64,
    pub total: Option<u64>,
    pub bytes_per_second: f64,
    pub connections: usize,
    pub segments: Vec<SegmentView>,
    pub resumable: bool,
    pub added_ms: u64,
    pub last_try_ms: Option<u64>,
    pub completed_ms: Option<u64>,
    pub error: Option<String>,
    pub note: Option<String>,
    /// Work after the transfer ("Joining video and audio"), with its progress when measurable.
    pub phase: Option<String>,
    pub phase_fraction: Option<f64>,
    /// The size is the site's estimate (videos).
    pub estimated: bool,
    pub exists: bool,
    pub speed_limit: u64,
}

/// What only a running transfer knows.
#[derive(Debug, Clone, Default)]
pub struct LiveInfo {
    pub speed: f64,
    pub connections: usize,
    pub note: Option<String>,
    pub phase: Option<String>,
    pub phase_fraction: Option<f64>,
    pub estimated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentView {
    pub start: u64,
    pub end: Option<u64>,
    pub done: u64,
}

impl View {
    pub fn of(record: &Record, live: &LiveInfo) -> View {
        let exists = record.status != Status::Completed || record.destination.exists();
        View {
            id: record.id.clone(),
            name: record.name(),
            url: record.url.clone(),
            path: record.destination.to_string_lossy().into_owned(),
            folder: record.destination.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            category: record.category.clone(),
            description: record.description.clone(),
            queue: record.queue.clone(),
            status: record.status,
            kind: match record.kind { Kind::Http => "file", Kind::Video { .. } => "video", Kind::Torrent { .. } => "torrent" },
            written: record.written,
            total: record.total,
            bytes_per_second: live.speed,
            connections: live.connections,
            segments: record.resume.plan.segments.iter().map(|s| SegmentView { start: s.start, end: s.end, done: s.done }).collect(),
            resumable: record.resume.ranged,
            added_ms: record.added_ms,
            last_try_ms: record.last_try_ms,
            completed_ms: record.completed_ms,
            error: record.error.clone(),
            note: live.note.clone(),
            phase: live.phase.clone(),
            phase_fraction: live.phase_fraction,
            estimated: live.estimated,
            exists,
            speed_limit: record.speed_limit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AfterQueue {
    #[default]
    Nothing,
    Exit,
    Sleep,
    Hibernate,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Queue {
    pub name: String,
    /// Downloads of this queue running at the same time.
    pub max_active: usize,
    pub schedule_enabled: bool,
    /// "HH:MM", local time.
    pub start_time: Option<String>,
    pub stop_time: Option<String>,
    /// 0 = Sunday … 6 = Saturday; empty = once (today/next occurrence).
    pub days: Vec<u8>,
    pub after: AfterQueue,
    pub retries: u32,
}

impl Default for Queue {
    fn default() -> Self {
        Queue { name: MAIN_QUEUE.into(), max_active: 3, schedule_enabled: false, start_time: None, stop_time: None, days: vec![], after: AfterQueue::Nothing, retries: 3 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub download_dir: PathBuf,
    /// Save each category in its own sub-folder of the download folder.
    pub category_folders: bool,
    pub connections: usize,
    /// Bytes per second for all downloads together; 0 = unlimited.
    pub speed_limit: u64,
    pub show_start_dialog: bool,
    pub show_progress_window: bool,
    pub show_complete_dialog: bool,
    pub theme: String,
    pub language: String,
    pub minimize_to_tray: bool,
    pub start_with_windows: bool,
    pub monitor_clipboard: bool,
    pub play_sounds: bool,
    pub max_retries: u32,
    pub proxy: String,
    pub user_agent: String,
    pub queues: Vec<Queue>,
    pub host_limits: std::collections::HashMap<String, usize>,
    pub update_feed: String,
    pub auto_check_updates: bool,
    pub auto_update_tools: bool,
    pub last_update_check_ms: u64,
    pub capture_extensions: String,
    pub capture_enabled: bool,
    pub video_panel: bool,
    pub excluded_sites: String,
    pub columns: Vec<String>,
}

pub const DEFAULT_EXTENSIONS: &str = "3gp 7z aac ace aif apk arj asf avi bin bz2 deb dmg doc docx epub exe flac flv gz gzip img iso jar lzh m4a m4v mkv mov mp3 mp4 mpeg mpg msi msix ogg ogv opus pdf pkg ppt pptx qt r0* r1* ra rar rm rmvb rpm sit sitx tar tgz torrent ts vob wav webm wma wmv xls xlsx xz z zip zst";

impl Default for Settings {
    fn default() -> Self {
        Settings {
            download_dir: default_download_dir(),
            category_folders: true,
            connections: 8,
            speed_limit: 0,
            show_start_dialog: true,
            show_progress_window: true,
            show_complete_dialog: true,
            theme: "dark".into(),
            language: "en".into(),
            minimize_to_tray: true,
            start_with_windows: false,
            monitor_clipboard: true,
            play_sounds: true,
            max_retries: 5,
            proxy: String::new(),
            user_agent: String::new(),
            queues: vec![Queue::default(), Queue { name: "Synchronization queue".into(), max_active: 1, ..Queue::default() }],
            host_limits: Default::default(),
            update_feed: String::new(),
            auto_check_updates: true,
            auto_update_tools: true,
            last_update_check_ms: 0,
            capture_extensions: DEFAULT_EXTENSIONS.into(),
            capture_enabled: true,
            video_panel: true,
            excluded_sites: String::new(),
            columns: vec![],
        }
    }
}

pub fn default_download_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Downloads")
}

/// The category a file belongs to, from its extension (same groups as Flux 1.x).
pub fn category_for(name: &str) -> &'static str {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "iso" | "cab" | "arj" | "lzh" | "tgz" => "Compressed",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "odt" | "ods" | "epub" | "csv" => "Documents",
        "mp3" | "flac" | "wav" | "m4a" | "aac" | "ogg" | "opus" | "wma" => "Music",
        "exe" | "msi" | "msix" | "appx" | "dmg" | "pkg" | "deb" | "rpm" | "apk" | "bat" | "appimage" => "Programs",
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "m4v" | "ts" | "3gp" | "mpg" | "mpeg" => "Video",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "tif" | "tiff" | "heic" | "ico" => "Images",
        _ => "Other",
    }
}
