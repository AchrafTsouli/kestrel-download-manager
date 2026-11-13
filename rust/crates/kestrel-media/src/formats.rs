//! A video's qualities and a playlist's entries, read from yt-dlp's JSON. Same rules as 1.x
//! (`VideoFormats`); the JSON shapes are the ones the browser extension already uses.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::pages;

/// Presets let yt-dlp choose for each video (playlists); explicit IDs come from a quality list.
pub const PRESET_PREFIX: &str = "apex-";
pub const PRESET_HEIGHTS: &[u32] = &[2160, 1440, 1080, 720, 480, 360];
pub const EXTENSIONS: &[&str] = &["mp4", "webm", "mkv", "m4a"];

/// One quality, as listed and as chosen (with an optional clip).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Selection {
    pub format_id: String,
    pub extension: String,
    pub label: String,
    pub total_bytes: Option<u64>,
    pub approximate: bool,
    pub audio_only: bool,
    pub no_audio: bool,
    /// Only this part of the video, in seconds. Exact re-encodes the cut so it starts on the very second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_end: Option<f64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clip_exact: bool,
    /// The site refused ordinary requests; yt-dlp imitates Chrome's network signature for it.
    pub impersonate: bool,
    /// Subtitle languages to put into the file ("en.*,fr"); none = no subtitles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitles: Option<String>,
    /// The audio track's language ("fr", "en"), when the site offers several; none = the site's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_language: Option<String>,
    /// Cut sponsor segments, self-promotion and reminders out (SponsorBlock's community data).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sponsorblock: bool,
    /// Put the video's chapters into the file.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub chapters: bool,
}

/// "en", "fr.*", "pt-BR", "all", separated by commas: nothing a command line could misread.
pub fn valid_languages(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64 && text.split(',').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.*".contains(&b)) && !l.starts_with('-'))
}

/// One language code for the audio track: "en", "fr", "pt-BR".
pub fn valid_audio_language(text: &str) -> bool {
    (2..=12).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !text.starts_with('-')
}

pub fn valid_format_id(id: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.@-]{0,99}(\+[A-Za-z0-9][A-Za-z0-9_.@-]{0,99})?$").unwrap());
    PATTERN.is_match(id)
}

impl Selection {
    pub fn preset(max_height: Option<u32>, audio_only: bool) -> Selection {
        if audio_only {
            return Selection { format_id: format!("{PRESET_PREFIX}audio"), extension: "m4a".into(), label: "Best audio only".into(), audio_only: true, ..Default::default() };
        }
        match max_height {
            Some(height) => Selection { format_id: format!("{PRESET_PREFIX}{height}"), extension: "mp4".into(), label: format!("Up to {height}p · video + audio"), ..Default::default() },
            None => Selection { format_id: format!("{PRESET_PREFIX}best"), extension: "mp4".into(), label: "Best quality · video + audio".into(), ..Default::default() },
        }
    }
    pub fn is_clip(&self) -> bool {
        self.clip_start.is_some() || self.clip_end.is_some()
    }
    pub fn is_preset(&self) -> bool {
        self.format_id.starts_with(PRESET_PREFIX)
    }
    pub fn preset_height(&self) -> Option<u32> {
        self.format_id.strip_prefix(PRESET_PREFIX).and_then(|h| h.parse().ok())
    }
    /// The name part for a file ("720p", "audio" or the format ID).
    pub fn quality(&self) -> String {
        static HEIGHT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\d+p").unwrap());
        if let Some(height) = HEIGHT.find(&self.label) {
            return height.as_str().into();
        }
        if self.audio_only { "audio".into() } else { self.format_id.clone() }
    }

    pub fn validate(&self, page: &str) -> Result<(), String> {
        let format = if self.is_preset() {
            let rest = &self.format_id[PRESET_PREFIX.len()..];
            rest == "best" || rest == "audio" && self.audio_only || self.preset_height().is_some_and(|h| PRESET_HEIGHTS.contains(&h))
        } else {
            valid_format_id(&self.format_id)
        };
        if pages::canonicalize(page).is_none() || !format || !EXTENSIONS.contains(&self.extension.as_str()) || self.label.chars().count() > 200 || self.audio_only && self.no_audio {
            return Err("Invalid video format selection.".into());
        }
        if self.subtitles.as_deref().is_some_and(|l| !valid_languages(l)) {
            return Err("Write subtitle languages like: en, fr, pt-BR (or all).".into());
        }
        if self.audio_language.as_deref().is_some_and(|l| !valid_audio_language(l)) {
            return Err("Write the audio language like: en, fr or pt-BR.".into());
        }
        if self.is_clip() {
            let start = self.clip_start.unwrap_or(0.0);
            let ok_start = self.clip_start.is_none_or(|s| s.is_finite() && (0.0..864_000.0).contains(&s));
            let ok_end = self.clip_end.is_none_or(|e| e.is_finite() && e > 0.0 && e <= 864_000.0);
            if !ok_start || !ok_end || self.clip_end.is_some_and(|e| start >= e) {
                return Err("The clip must end after it starts, within the video.".into());
            }
        } else if self.clip_exact {
            return Err("Exact cutting needs a start or end time.".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub title: String,
    pub formats: Vec<Selection>,
    pub site: Option<String>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    pub url: String,
    pub title: String,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub title: String,
    pub entries: Vec<PlaylistEntry>,
}

/// Either one video's qualities or a playlist's entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub catalog: Option<Catalog>,
    pub playlist: Option<Playlist>,
}

fn text<'a>(item: &'a Value, key: &str) -> &'a str {
    item.get(key).and_then(Value::as_str).unwrap_or("")
}
fn number(item: &Value, key: &str) -> f64 {
    item.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0)
}
fn clip(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}
fn round(value: f64) -> String {
    format!("{}", value.round() as i64)
}

pub fn is_playlist(root: &Value) -> bool {
    matches!(text(root, "_type"), "playlist" | "multi_video")
}

/// Unknown codecs (common outside YouTube, e.g. HLS variants) count as present; "none" means absent.
/// audio_ext/video_ext are "none" on combined formats too, so only the codec fields signal absence.
fn known(item: &Value, codec: &str) -> bool {
    let value = text(item, codec);
    !value.is_empty() && value != "none"
}
fn is_video(item: &Value) -> bool {
    text(item, "vcodec") != "none"
        && (known(item, "vcodec") || number(item, "height") > 0.0 || number(item, "width") > 0.0 || text(item, "video_ext") != "none" && !known(item, "acodec"))
        && text(item, "resolution") != "audio only"
}
fn is_audio(item: &Value) -> bool {
    text(item, "acodec") != "none" && (known(item, "acodec") || !is_video(item) || !known(item, "vcodec"))
}
fn usable(item: &Value) -> bool {
    let url = text(item, "url").to_ascii_lowercase();
    item.get("has_drm").and_then(Value::as_bool) != Some(true)
        && valid_format_id(text(item, "format_id"))
        && (url.starts_with("https://") || url.starts_with("http://"))
        && !matches!(text(item, "protocol"), "rtmp" | "rtmpe" | "rtsp" | "mms" | "f4m" | "ism")
}
/// Size in bytes and whether it is an estimate.
fn size(item: &Value) -> (Option<u64>, bool) {
    let (exact, estimate) = (number(item, "filesize"), number(item, "filesize_approx"));
    let value = if exact > 0.0 { exact } else { estimate };
    if value > 0.0 && value < u64::MAX as f64 { (Some(value as u64), exact <= 0.0) } else { (None, false) }
}
fn codec(item: &Value) -> String {
    match text(item, "vcodec").split('.').next().unwrap_or("") {
        "" | "none" => String::new(),
        "avc1" | "avc3" | "h264" => "H.264".into(),
        "hvc1" | "hev1" | "h265" => "H.265".into(),
        "av01" => "AV1".into(),
        "vp09" | "vp9" => "VP9".into(),
        other => other.to_uppercase(),
    }
}
fn descending(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

/// One video's qualities, best first, with one audio-only choice at the end.
pub fn parse(json: &str) -> Result<Catalog, String> {
    let document: Value = serde_json::from_str(json).map_err(|e| format!("The video tool returned unreadable data: {e}"))?;
    let mut root = &document;
    // Pages with several videos (a post with clips) return a playlist; use its first video.
    if is_playlist(root)
        && let Some(first) = root.get("entries").and_then(Value::as_array).and_then(|e| e.first()).filter(|e| e.get("formats").is_some()) {
            root = first;
        }
    if root.get("is_live").and_then(Value::as_bool) == Some(true) {
        return Err("Live broadcasts are not supported yet. Retry when the broadcast has finished.".into());
    }
    let formats = root.get("formats").and_then(Value::as_array).ok_or("No video qualities were returned.")?;
    let all: Vec<&Value> = formats.iter().filter(|item| item.is_object() && usable(item)).collect();
    let mut audio: Vec<&Value> = all.iter().copied().filter(|item| is_audio(item) && !is_video(item)).collect();
    audio.sort_by(|a, b| {
        descending(number(a, "language_preference"), number(b, "language_preference"))
            .then(descending(number(a, "abr"), number(b, "abr")))
            .then(descending(number(a, "tbr"), number(b, "tbr")))
    });
    let mut videos: Vec<&Value> = all.iter().copied().filter(|item| is_video(item)).collect();
    videos.sort_by(|a, b| {
        descending(number(a, "height"), number(b, "height")).then(descending(number(a, "fps"), number(b, "fps"))).then(descending(number(a, "tbr"), number(b, "tbr")))
    });

    let mut result: Vec<Selection> = Vec::new();
    let mut qualities = HashSet::new();
    for video in videos {
        let mut ext = text(video, "ext").to_string();
        if ext != "mp4" && ext != "webm" {
            continue;
        }
        let mut id = text(video, "format_id").to_string();
        let (mut total, mut approximate) = size(video);
        let mut no_audio = false;
        if !is_audio(video) {
            let pair = if ext == "mp4" { "m4a" } else { "webm" };
            let sound = audio.iter().find(|a| text(a, "ext") == pair).or_else(|| audio.iter().find(|a| matches!(text(a, "ext"), "m4a" | "mp4" | "webm")));
            match sound {
                None => no_audio = true,
                Some(sound) => {
                    if text(sound, "ext") != pair {
                        ext = "mkv".into();
                    }
                    id = format!("{id}+{}", text(sound, "format_id"));
                    let (audio_total, audio_estimate) = size(sound);
                    total = match (total, audio_total) {
                        (Some(v), Some(a)) if v > 0 && a > 0 => v.checked_add(a),
                        _ => None,
                    };
                    approximate |= audio_estimate;
                }
            }
        }
        if id.len() > 201 || !valid_format_id(&id) {
            continue;
        }
        let (height, fps) = (number(video, "height"), number(video, "fps"));
        let codec = codec(video);
        let quality = if height > 0.0 {
            format!("{}p", height as u64)
        } else if !text(video, "format_note").is_empty() {
            clip(text(video, "format_note"), 40)
        } else if number(video, "tbr") > 0.0 {
            format!("{} kbps", round(number(video, "tbr")))
        } else {
            id.clone()
        };
        if !qualities.insert(format!("{ext}|{quality}|{fps}|{codec}|{no_audio}")) {
            continue;
        }
        let fps_text = if fps > 30.0 { format!(" · {} fps", trim_number(fps)) } else { String::new() };
        let codec_text = if codec.is_empty() { String::new() } else { format!(" · {codec}") };
        let label = format!("{} · {quality}{fps_text}{codec_text} · {}", ext.to_uppercase(), if no_audio { "video only" } else { "video + audio" });
        if result.iter().all(|item| item.format_id != id) {
            result.push(Selection { format_id: id, label, extension: ext, total_bytes: total, approximate, no_audio, ..Default::default() });
        }
        if result.len() >= 50 {
            break;
        }
    }
    // One audio-only choice, in a container the bundled tools can write without re-encoding.
    if let Some(best) = audio.iter().find(|a| text(a, "ext") == "m4a").or_else(|| audio.iter().find(|a| text(a, "ext") == "webm")) {
        let (total, approximate) = size(best);
        let rate = if number(best, "abr") > 0.0 { number(best, "abr") } else { number(best, "tbr") };
        let ext = text(best, "ext").to_string();
        let rate_text = if rate > 0.0 { format!(" · {} kbps", round(rate)) } else { String::new() };
        result.push(Selection { format_id: text(best, "format_id").into(), label: format!("{} · audio only{rate_text}", ext.to_uppercase()), extension: ext, total_bytes: total, approximate, audio_only: true, ..Default::default() });
    }
    if result.is_empty() {
        return Err("No unencrypted video is available for this page.".into());
    }
    let title = clip(text(root, "title"), 200);
    let site = clip(text(root, "extractor_key"), 60);
    let duration = number(root, "duration");
    Ok(Catalog {
        title: if title.is_empty() { "Video".into() } else { title },
        formats: result,
        site: (!site.is_empty()).then_some(site),
        duration_seconds: (duration > 0.0).then_some(duration),
    })
}

fn trim_number(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A playlist's videos (private and deleted ones left out), at most `maximum`.
pub fn parse_playlist(json: &str, maximum: usize) -> Result<Playlist, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("The video tool returned unreadable data: {e}"))?;
    let entries = root.get("entries").and_then(Value::as_array).filter(|_| is_playlist(&root)).ok_or("No playlist entries were returned.")?;
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries.iter().filter(|e| e.is_object()) {
        let title = clip(text(entry, "title"), 200);
        if title == "[Private video]" || title == "[Deleted video]" {
            continue;
        }
        let url = [text(entry, "url"), text(entry, "webpage_url")].into_iter().filter_map(pages::canonicalize).find(|u| pages::playlist(u).is_none());
        let Some(url) = url else { continue };
        if !seen.insert(url.clone()) {
            continue;
        }
        let duration = number(entry, "duration");
        let title = if title.is_empty() { format!("Video {}", result.len() + 1) } else { title };
        result.push(PlaylistEntry { url, title, duration_seconds: (duration > 0.0).then_some(duration) });
        if result.len() >= maximum {
            break;
        }
    }
    if result.is_empty() {
        return Err("The playlist has no downloadable videos.".into());
    }
    let name = clip(text(&root, "title"), 200);
    Ok(Playlist { title: if name.is_empty() { "Playlist".into() } else { name }, entries: result })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn format(id: &str, ext: &str, vcodec: &str, acodec: &str, height: u32, size: u64) -> Value {
        json!({ "format_id": id, "ext": ext, "vcodec": vcodec, "acodec": acodec, "height": height, "filesize": size, "url": "https://media.test/x", "protocol": "https" })
    }

    #[test]
    fn video_only_qualities_are_paired_with_matching_audio() {
        let page = json!({ "title": "Talk", "extractor_key": "Youtube", "duration": 90.0, "formats": [
            format("140", "m4a", "none", "mp4a.40.2", 0, 1_000),
            format("251", "webm", "none", "opus", 0, 900),
            format("137", "mp4", "avc1.640028", "none", 1080, 50_000),
            format("248", "webm", "vp9", "none", 1080, 40_000),
            format("18", "mp4", "avc1.42001E", "mp4a.40.2", 360, 5_000),
        ]});
        let catalog = parse(&page.to_string()).unwrap();
        let ids: Vec<&str> = catalog.formats.iter().map(|f| f.format_id.as_str()).collect();
        assert_eq!(ids, ["137+140", "248+251", "18", "140"]);
        assert_eq!(catalog.formats[0].label, "MP4 · 1080p · H.264 · video + audio");
        assert_eq!(catalog.formats[0].total_bytes, Some(51_000));
        assert_eq!(catalog.formats[1].extension, "webm");
        assert!(catalog.formats[3].audio_only);
        assert_eq!(catalog.duration_seconds, Some(90.0));
        assert_eq!(catalog.site.as_deref(), Some("Youtube"));
    }

    #[test]
    fn unknown_codecs_count_as_present_and_drm_is_skipped() {
        // HLS variants often carry no codec names at all.
        let page = json!({ "title": "", "formats": [
            { "format_id": "hls-720", "ext": "mp4", "height": 720, "url": "https://media.test/720.m3u8", "protocol": "m3u8_native", "tbr": 2500 },
            { "format_id": "drm", "ext": "mp4", "height": 1080, "url": "https://media.test/d", "has_drm": true },
            { "format_id": "rtmp", "ext": "mp4", "height": 1080, "url": "https://media.test/r", "protocol": "rtmp" },
        ]});
        let catalog = parse(&page.to_string()).unwrap();
        assert_eq!(catalog.title, "Video");
        assert_eq!(catalog.formats.len(), 1);
        assert_eq!(catalog.formats[0].label, "MP4 · 720p · video + audio");
        assert!(!catalog.formats[0].no_audio);
    }

    #[test]
    fn a_post_with_several_videos_uses_the_first_and_live_is_refused() {
        let post = json!({ "_type": "playlist", "entries": [{ "title": "Clip", "formats": [format("18", "mp4", "avc1", "mp4a", 360, 10)] }] });
        assert_eq!(parse(&post.to_string()).unwrap().title, "Clip");
        let live = json!({ "is_live": true, "formats": [] });
        assert!(parse(&live.to_string()).unwrap_err().contains("Live"));
        assert!(parse(&json!({ "formats": [] }).to_string()).is_err());
    }

    #[test]
    fn webm_video_with_only_m4a_audio_becomes_mkv() {
        let page = json!({ "formats": [format("140", "m4a", "none", "mp4a", 0, 1), format("248", "webm", "vp9", "none", 1080, 1)] });
        let catalog = parse(&page.to_string()).unwrap();
        assert_eq!((catalog.formats[0].format_id.as_str(), catalog.formats[0].extension.as_str()), ("248+140", "mkv"));
    }

    #[test]
    fn playlists_skip_private_duplicate_and_nested_entries() {
        let list = json!({ "_type": "playlist", "title": "Course", "entries": [
            { "url": "https://www.youtube.com/watch?v=aaaaaaaaaaa", "title": "One", "duration": 60 },
            { "url": "https://youtu.be/aaaaaaaaaaa", "title": "Again" },
            { "url": "https://www.youtube.com/watch?v=bbbbbbbbbbb", "title": "[Private video]" },
            { "url": "https://www.youtube.com/playlist?list=PLx", "title": "Nested" },
            { "url": "https://www.youtube.com/watch?v=ccccccccccc", "title": "" },
        ]});
        let playlist = parse_playlist(&list.to_string(), 1000).unwrap();
        assert_eq!(playlist.title, "Course");
        assert_eq!(playlist.entries.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["One", "Video 2"]);
        assert_eq!(parse_playlist(&list.to_string(), 1).unwrap().entries.len(), 1);
    }

    #[test]
    fn selections_are_validated() {
        let page = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        assert!(Selection::preset(Some(720), false).validate(page).is_ok());
        assert!(Selection::preset(None, true).validate(page).is_ok());
        assert!(Selection { format_id: "apex-999".into(), extension: "mp4".into(), ..Default::default() }.validate(page).is_err());
        assert!(Selection { format_id: "137+140".into(), extension: "mp4".into(), ..Default::default() }.validate(page).is_ok());
        assert!(Selection { format_id: "--exec".into(), extension: "mp4".into(), ..Default::default() }.validate(page).is_err());
        assert!(Selection { format_id: "18".into(), extension: "exe".into(), ..Default::default() }.validate(page).is_err());
        let clip = |start, end, exact| Selection { format_id: "18".into(), extension: "mp4".into(), clip_start: start, clip_end: end, clip_exact: exact, ..Default::default() };
        assert!(clip(Some(10.0), Some(20.0), true).validate(page).is_ok());
        assert!(clip(Some(20.0), Some(10.0), false).validate(page).is_err());
        assert!(clip(None, None, true).validate(page).is_err());
        assert!(clip(Some(f64::NAN), None, false).validate(page).is_err());
        assert_eq!(Selection { label: "MP4 · 720p · H.264".into(), ..Default::default() }.quality(), "720p");
        let subs = |l: &str| Selection { format_id: "18".into(), extension: "mp4".into(), subtitles: Some(l.into()), ..Default::default() };
        assert!(subs("en.*,fr,pt-BR").validate(page).is_ok());
        assert!(subs("--exec").validate(page).is_err());
        assert!(subs("en fr").validate(page).is_err());
    }
}
