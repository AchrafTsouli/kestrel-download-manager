//! Runs yt-dlp, FFmpeg and ffprobe: quality lists, playlists, downloads, clips. Same behaviour as
//! 1.x (`VideoTools`): impersonation retry on refusals, exact cut with Windows' H.264 encoder,
//! output checked with ffprobe and named after its real container.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use flux_engine::Session;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::formats::{self, Catalog, Playlist, Probe, Selection};
use crate::pages;
use crate::update;

const OUTPUT_EXTENSIONS: &[&str] = &["mp4", "webm", "mkv", "m4a", "mov", "mp3", "ogg", "opus"];
const IMPERSONATION: &[&str] = &["--impersonate", "chrome", "--extractor-args", "generic:impersonate"];

/// What a running video download reports.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    /// The format IDs being fetched (known once yt-dlp has chosen, for presets).
    Tracks(Vec<String>),
    Progress { written: u64, total: Option<u64>, estimated: bool, bytes_per_second: f64 },
    /// After the transfer: joining, cutting, copying. `fraction` when it can be measured.
    Phase { text: String, fraction: Option<f64> },
    /// The site refused; retrying with Chrome's network signature (saved with the selection).
    Impersonating,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Completed { path: PathBuf, bytes: u64, sha256: Option<String> },
    Paused,
}

/// A video download: the page, where it goes and how.
#[derive(Debug, Clone)]
pub struct VideoJob {
    pub page: String,
    /// The intended file; renamed when the real container differs.
    pub destination: PathBuf,
    /// Parts kept between pauses (`.flux-video-<id>` beside the destination).
    pub work: PathBuf,
    pub selection: Selection,
    pub session: Option<Session>,
    pub connections: usize,
    pub retries: u32,
    /// Bytes per second; 0 = unlimited.
    pub speed_limit: u64,
    pub expected_sha256: Option<String>,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// The tools folder (yt-dlp, FFmpeg, ffprobe, QuickJS) and the folder holding a newer yt-dlp.
pub struct Tools {
    pub directory: PathBuf,
    pub update_directory: PathBuf,
    exact_cut: Mutex<Option<bool>>,
    /// The proxy chosen in Flux's options (with its credentials), passed to yt-dlp.
    proxy: Mutex<Option<String>>,
}

impl Tools {
    pub fn new(directory: impl Into<PathBuf>, update_directory: impl Into<PathBuf>) -> Tools {
        Tools { directory: directory.into(), update_directory: update_directory.into(), exact_cut: Mutex::new(None), proxy: Mutex::new(None) }
    }

    pub fn set_proxy(&self, proxy: Option<String>) {
        *self.proxy.lock().unwrap() = proxy;
    }

    pub fn available(&self) -> bool {
        ["yt-dlp.exe", "ffmpeg.exe", "ffprobe.exe"].iter().all(|n| self.directory.join(n).exists())
            && (self.directory.join("qjs.exe").exists() || self.directory.join("node.exe").exists())
    }

    fn path(&self, tool: &str) -> Result<PathBuf, String> {
        if !self.available() {
            return Err("The video tools are missing. Reinstall Flux to restore them.".into());
        }
        // A verified newer yt-dlp replaces the aging bundled copy; FFmpeg and QuickJS stay bundled.
        if tool == "yt-dlp.exe"
            && let Some(updated) = update::preferred(&self.directory, &self.update_directory) {
                return Ok(updated);
            }
        Ok(self.directory.join(tool))
    }

    fn javascript_runtime(&self) -> String {
        let qjs = self.directory.join("qjs.exe");
        if qjs.exists() { format!("quickjs:{}", qjs.display()) } else { format!("node:{}", self.directory.join("node.exe").display()) }
    }

    fn common(&self, cookies: Option<&Path>, session: Option<&Session>, playlist: bool) -> Vec<String> {
        let mut args: Vec<String> = [if playlist { "--yes-playlist" } else { "--no-playlist" }, "--ignore-config", "--no-cache-dir", "--output-na-placeholder", "null", "--no-js-runtimes", "--js-runtimes"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.push(self.javascript_runtime());
        args.push("--ffmpeg-location".into());
        args.push(self.directory.display().to_string());
        if let Some(proxy) = self.proxy.lock().unwrap().clone() {
            args.extend(["--proxy".into(), proxy]);
        }
        if let Some(cookies) = cookies {
            args.extend(["--cookies".into(), cookies.display().to_string()]);
        }
        // Streams and many non-YouTube sites check the embedding page and browser identity.
        if let Some(referer) = session.and_then(|s| s.referer.as_deref()).filter(|r| !r.is_empty()) {
            args.extend(["--referer".into(), referer.to_string()]);
        }
        if let Some(agent) = session.and_then(|s| s.user_agent.as_deref()).filter(|a| !a.is_empty()) {
            args.extend(["--user-agent".into(), agent.to_string()]);
        }
        args
    }

    /// Runs a tool, calling `line` for each output line; killed when `cancel` fires.
    async fn run(&self, tool: &str, args: &[String], mut line: impl FnMut(&str), cancel: &CancellationToken) -> Result<Run, String> {
        let mut command = Command::new(self.path(tool)?);
        command.args(args).current_dir(&self.directory).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        command.env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // no console window
        let mut child = command.spawn().map_err(|e| format!("Could not start the video tool: {e}"))?;
        let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut stderr = BufReader::new(child.stderr.take().unwrap()).lines();
        let (mut out, mut err) = (String::new(), String::new());
        let (mut out_open, mut err_open) = (true, true);
        while out_open || err_open {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    return Err(String::new());
                }
                next = stdout.next_line(), if out_open => match next {
                    Ok(Some(text)) => {
                        line(&text);
                        if !text.starts_with("FLUXPROGRESS:") {
                            if out.len() + text.len() > 16 * 1024 * 1024 {
                                let _ = child.kill().await;
                                return Err("Video tool output exceeded its size limit.".into());
                            }
                            out.push_str(&text);
                            out.push('\n');
                        }
                    }
                    _ => out_open = false,
                },
                next = stderr.next_line(), if err_open => match next {
                    Ok(Some(text)) => {
                        line(&text);
                        // Keep the end: the last error line is the useful one.
                        if err.len() + text.len() > 128 * 1024 {
                            err.clear();
                        }
                        err.push_str(&text);
                        err.push('\n');
                    }
                    _ => err_open = false,
                },
            }
        }
        let status = tokio::select! {
            _ = cancel.cancelled() => { let _ = child.kill().await; return Err(String::new()); }
            status = child.wait() => status.map_err(|e| e.to_string())?,
        };
        Ok(Run { code: status.code(), stdout: out, stderr: err })
    }

    /// True when the bundled FFmpeg can re-encode a cut (Windows' Media Foundation H.264 encoder).
    pub async fn supports_exact_cut(&self) -> bool {
        if let Some(known) = *self.exact_cut.lock().unwrap() {
            return known;
        }
        let args = ["-hide_banner".to_string(), "-encoders".to_string()];
        let known = matches!(self.run("ffmpeg.exe", &args, |_| {}, &CancellationToken::new()).await, Ok(run) if run.code == Some(0) && run.stdout.contains(" h264_mf "));
        *self.exact_cut.lock().unwrap() = Some(known);
        known
    }

    /// One video's qualities. Refused requests are retried with Chrome's network signature; the
    /// qualities then carry `impersonate` so the download does the same.
    pub async fn formats(&self, page: &str, session: Option<&Session>, cancel: &CancellationToken) -> Result<Catalog, String> {
        let canonical = pages::canonicalize(page).ok_or("Use a video page or an HLS/DASH stream address.")?;
        let cookies = CookieFile::write(session, page)?;
        let arguments = |impersonate: bool| {
            let mut args = self.common(cookies.path(), session, false);
            if impersonate {
                args.extend(IMPERSONATION.iter().map(|s| s.to_string()));
            }
            args.extend(["--skip-download", "--dump-single-json", "--no-check-formats", "--playlist-items", "1", "--", &canonical].map(String::from));
            args
        };
        let mut run = self.run("yt-dlp.exe", &arguments(false), |_| {}, cancel).await?;
        let mut impersonated = false;
        if run.code != Some(0) && blocked(&run.stderr) {
            run = self.run("yt-dlp.exe", &arguments(true), |_| {}, cancel).await?;
            impersonated = true;
        }
        if run.code != Some(0) {
            return Err(failure(&run.stderr));
        }
        let mut catalog = formats::parse(&run.stdout)?;
        if impersonated {
            catalog.formats.iter_mut().for_each(|f| f.impersonate = true);
        }
        Ok(catalog)
    }

    pub async fn playlist(&self, page: &str, session: Option<&Session>, maximum: usize, cancel: &CancellationToken) -> Result<Playlist, String> {
        let list = pages::playlist(page).or_else(|| pages::canonicalize(page)).ok_or("Use a playlist or channel address.")?;
        let cookies = CookieFile::write(session, page)?;
        let mut args = self.common(cookies.path(), session, true);
        args.extend(["--flat-playlist".into(), "--dump-single-json".into(), "--playlist-items".into(), format!("1:{}", maximum.clamp(1, 5000)), "--".into(), list]);
        let run = self.run("yt-dlp.exe", &args, |_| {}, cancel).await?;
        if run.code != Some(0) {
            return Err(failure(&run.stderr));
        }
        formats::parse_playlist(&run.stdout, maximum)
    }

    /// Playlist and channel addresses list their entries; anything else lists one video's qualities.
    pub async fn probe(&self, page: &str, session: Option<&Session>, cancel: &CancellationToken) -> Result<Probe, String> {
        if pages::playlist(page).is_some() {
            return Ok(Probe { catalog: None, playlist: Some(self.playlist(page, session, 1000, cancel).await?) });
        }
        Ok(Probe { catalog: Some(self.formats(page, session, cancel).await?), playlist: None })
    }

    /// Downloads one video. A pause (cancel) keeps the parts in `job.work` for `--continue`.
    pub async fn download(&self, job: &mut VideoJob, events: impl Fn(MediaEvent), cancel: CancellationToken) -> Result<Outcome, String> {
        job.selection.validate(&job.page)?;
        let canonical = pages::canonicalize(&job.page).ok_or("Use a video page or an HLS/DASH stream address.")?;
        let work = job.work.clone();
        std::fs::create_dir_all(&work).map_err(|e| format!("The video work folder could not be created: {e}"))?;
        if let Some(folder) = job.destination.parent() {
            std::fs::create_dir_all(folder).map_err(|e| format!("The folder could not be created: {e}"))?;
        }
        let cookies = CookieFile::write(job.session.as_ref(), &job.page)?;
        let retries = job.retries.min(10).to_string();
        let mut args = self.common(cookies.path(), job.session.as_ref(), false);
        args.extend(
            ["--newline", "--progress", "--no-simulate", "--continue", "--no-overwrites", "--no-mtime", "--windows-filenames", "--abort-on-unavailable-fragments", "--playlist-items", "1", "--retries", &retries, "--fragment-retries", &retries, "--concurrent-fragments"]
                .map(String::from),
        );
        args.push(job.connections.clamp(1, 16).to_string());
        if job.selection.impersonate {
            args.extend(IMPERSONATION.iter().map(|s| s.to_string()));
        }
        args.extend(format_arguments(&job.selection));
        args.extend(clip_arguments(&job.selection));
        args.extend(subtitle_arguments(&job.selection));
        args.extend([
            "--output".into(),
            work.join("result.%(ext)s").display().to_string(),
            "--print".into(),
            "before_dl:FLUXFORMAT:%(format_id)j".into(),
            "--print".into(),
            "after_move:FLUXFILE:%(filepath)j".into(),
            "--progress-template".into(),
            "download:FLUXPROGRESS:{\"format\":%(info.format_id)j,\"bytes\":%(progress.downloaded_bytes)j,\"total\":%(progress.total_bytes)j,\"estimate\":%(progress.total_bytes_estimate)j,\"speed\":%(progress.speed)j}".into(),
        ]);
        if job.speed_limit > 0 {
            args.extend(["--limit-rate".into(), job.speed_limit.to_string()]);
        }
        args.extend(["--".into(), canonical]);

        let mut tracker = Tracker::new(if job.selection.is_preset() { Vec::new() } else { job.selection.format_id.split('+').map(String::from).collect() });
        let mut result_file: Option<PathBuf> = None;
        let mut run = {
            let mut line = |text: &str| tracker.line(text, &mut result_file, &events);
            self.run("yt-dlp.exe", &args, &mut line, &cancel).await
        };
        if matches!(&run, Err(e) if e.is_empty()) {
            return Ok(Outcome::Paused);
        }
        if matches!(&run, Ok(r) if r.code != Some(0) && !job.selection.impersonate && blocked(&r.stderr)) {
            job.selection.impersonate = true;
            events(MediaEvent::Impersonating);
            let at = args.iter().position(|a| a == "--format").unwrap_or(args.len() - 2);
            args.splice(at..at, IMPERSONATION.iter().map(|s| s.to_string()));
            let mut line = |text: &str| tracker.line(text, &mut result_file, &events);
            run = self.run("yt-dlp.exe", &args, &mut line, &cancel).await;
            if matches!(&run, Err(e) if e.is_empty()) {
                return Ok(Outcome::Paused);
            }
        }
        let run = run?;
        let result_file = result_file.filter(|file| file.parent() == Some(work.as_path()) && file.file_name().is_some_and(|n| n.to_string_lossy().starts_with("result.")) && file.exists());
        // A missing subtitle language (or a site refusing one) must not cost the video itself.
        let subtitles_only_failed = !subtitle_arguments(&job.selection).is_empty() && result_file.is_some();
        if run.code != Some(0) && !subtitles_only_failed {
            return Err(failure(&run.stderr));
        }
        let mut result = result_file.ok_or("The video tool did not produce a completed media file.")?;
        if job.selection.clip_exact {
            result = match self.exact_cut(&result, &work, job.selection.audio_only, &events, &cancel).await {
                Err(e) if e.is_empty() => return Ok(Outcome::Paused),
                other => other?,
            };
        }
        self.verify(&result, &job.selection, tracker.ids.len(), &cancel).await?;

        // yt-dlp can pick another container (presets, remuxed streams); name the file after its real format.
        let actual = result.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let current = job.destination.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if OUTPUT_EXTENSIONS.contains(&actual.as_str()) && actual != current {
            job.destination = free_path(&job.destination.with_extension(&actual));
        } else if job.destination.exists() {
            job.destination = free_path(&job.destination);
        }
        let staging = flux_engine::file::part_path(&job.destination);
        if std::fs::rename(&result, &staging).is_err() {
            // Another drive: copy with progress for the second bar.
            events(MediaEvent::Phase { text: "Copying to the destination drive".into(), fraction: Some(0.0) });
            if let Err(error) = copy_with_progress(&result, &staging, &events, &cancel).await {
                let _ = std::fs::remove_file(&staging);
                return if error.is_empty() { Ok(Outcome::Paused) } else { Err(error) };
            }
            let _ = std::fs::remove_file(&result);
        }
        let sha256 = match &job.expected_sha256 {
            Some(expected) => {
                let actual = hash_file(&staging).await.map_err(|e| e.to_string())?;
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err("The assembled video does not match the expected SHA-256 checksum.".into());
                }
                Some(actual)
            }
            None => None,
        };
        std::fs::rename(&staging, &job.destination).map_err(|e| format!("The finished video could not be saved: {e}"))?;
        // The work folder holds only this download's parts.
        move_subtitles(&work, &job.destination);
        let _ = std::fs::remove_dir_all(&work);
        let bytes = std::fs::metadata(&job.destination).map(|m| m.len()).unwrap_or(0);
        Ok(Outcome::Completed { path: job.destination.clone(), bytes, sha256 })
    }

    /// Re-encodes a fast (keyframe) cut so it starts on the exact frame: the fast cut carries the
    /// frames before the start hidden by an edit list, and decoding honours it.
    async fn exact_cut(&self, input: &Path, work: &Path, audio_only: bool, events: &impl Fn(MediaEvent), cancel: &CancellationToken) -> Result<PathBuf, String> {
        let probe_args = ["-v", "error", "-show_entries", "stream=codec_type,codec_name,bit_rate:format=duration,bit_rate", "-of", "json"].map(String::from);
        let mut info_args = probe_args.to_vec();
        info_args.push(input.display().to_string());
        let info = self.run("ffprobe.exe", &info_args, |_| {}, cancel).await?;
        if info.code != Some(0) {
            return Err(format!("The downloaded part could not be read: {}", failure(&info.stderr)));
        }
        let json: Value = serde_json::from_str(&info.stdout).map_err(|e| e.to_string())?;
        let streams = json["streams"].as_array().cloned().unwrap_or_default();
        let video = streams.iter().find(|s| s["codec_type"] == "video");
        if !audio_only && video.is_some_and(|v| v["codec_name"] == "av1") {
            return Err("This quality uses AV1, which can't be cut to the exact second. Choose an H.264 or VP9 quality, or use Fast cut.".into());
        }
        let duration: f64 = json["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
        let rate = |v: &Value| v["bit_rate"].as_str().and_then(|b| b.parse::<u64>().ok());
        let bitrate = video.and_then(rate).or_else(|| rate(&json["format"])).unwrap_or(0);
        let target = ((bitrate as f64 * 1.15) as u64).clamp(800_000, 60_000_000);
        let output = work.join(if audio_only { "result-exact.m4a" } else { "result-exact.mp4" });
        let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y", "-i"].map(String::from).to_vec();
        args.push(input.display().to_string());
        if audio_only {
            args.extend(["-vn", "-map", "0:a:0", "-c:a", "aac", "-b:a", "192k"].map(String::from));
        } else {
            args.extend(["-map", "0:v:0", "-map", "0:a?", "-c:v", "h264_mf", "-b:v"].map(String::from));
            args.push(target.to_string());
            args.extend(["-pix_fmt", "nv12", "-c:a", "aac", "-b:a", "192k"].map(String::from));
        }
        args.extend(["-movflags".into(), "+faststart".into(), output.display().to_string()]);
        events(MediaEvent::Phase { text: "Cutting to the exact second".into(), fraction: Some(0.0) });
        let line = |text: &str| {
            if let Some(us) = text.strip_prefix("out_time_us=").and_then(|v| v.parse::<i64>().ok())
                && duration > 0.0 {
                    events(MediaEvent::Phase { text: "Cutting to the exact second".into(), fraction: Some((us as f64 / 1e6 / duration).clamp(0.0, 1.0)) });
                }
        };
        let result = self.run("ffmpeg.exe", &args, line, cancel).await?;
        if result.code != Some(0) || !output.exists() {
            return Err(if result.stderr.contains("h264_mf") || result.stderr.contains("MFT") {
                "Windows' video encoder is not available on this PC (for example a Windows \"N\" edition without the Media Feature Pack). Use Fast cut.".into()
            } else {
                format!("The exact cut failed: {}", failure(&result.stderr))
            });
        }
        let _ = std::fs::remove_file(input);
        Ok(output)
    }

    /// The output must hold the streams that were asked for.
    async fn verify(&self, file: &Path, selection: &Selection, tracks: usize, cancel: &CancellationToken) -> Result<(), String> {
        let args = ["-v".into(), "error".into(), "-show_entries".into(), "stream=codec_type".into(), "-of".into(), "json".into(), file.display().to_string()];
        let run = self.run("ffprobe.exe", &args, |_| {}, cancel).await?;
        if run.code != Some(0) {
            return Err(format!("The downloaded media could not be read: {}", failure(&run.stderr)));
        }
        let json: Value = serde_json::from_str(&run.stdout).map_err(|_| "The downloaded media contains no readable streams.")?;
        let streams: Vec<&str> = json["streams"].as_array().ok_or("The downloaded media contains no readable streams.")?.iter().filter_map(|s| s["codec_type"].as_str()).collect();
        if selection.audio_only {
            if !streams.contains(&"audio") {
                return Err("The output must contain an audio stream.".into());
            }
        } else {
            if !streams.contains(&"video") {
                return Err("The output must contain a video stream.".into());
            }
            // A merged selection must keep its sound; combined formats from other sites may be silent.
            if !selection.no_audio && tracks > 1 && !streams.contains(&"audio") {
                return Err("The output must contain both a video stream and an audio stream.".into());
            }
        }
        Ok(())
    }
}

/// Reads yt-dlp's progress lines: per-track bytes add up to the video's progress.
struct Tracker {
    ids: Vec<String>,
    bytes: HashMap<String, u64>,
    totals: HashMap<String, u64>,
    estimates: HashSet<String>,
}

impl Tracker {
    fn new(ids: Vec<String>) -> Tracker {
        Tracker { ids, bytes: HashMap::new(), totals: HashMap::new(), estimates: HashSet::new() }
    }

    fn line(&mut self, text: &str, result: &mut Option<PathBuf>, events: &impl Fn(MediaEvent)) {
        if let Some(path) = text.strip_prefix("FLUXFILE:") {
            *result = serde_json::from_str::<String>(path).ok().map(PathBuf::from);
            return;
        }
        if let Some(chosen) = text.strip_prefix("FLUXFORMAT:") {
            if let Ok(chosen) = serde_json::from_str::<String>(chosen)
                && formats::valid_format_id(&chosen) {
                    self.ids = chosen.split('+').map(String::from).collect();
                    events(MediaEvent::Tracks(self.ids.clone()));
                }
            return;
        }
        for (prefix, phase) in [("[Merger]", "Joining video and audio"), ("[VideoRemuxer]", "Preparing the file"), ("[FixupM3u8]", "Preparing the file"), ("[VideoConvertor]", "Preparing the file")] {
            if text.starts_with(prefix) {
                events(MediaEvent::Phase { text: phase.into(), fraction: None });
                return;
            }
        }
        let Some(json) = text.strip_prefix("FLUXPROGRESS:") else { return };
        let Ok(item) = serde_json::from_str::<Value>(json) else { return };
        let id = item["format"].as_str().unwrap_or("video").to_string();
        if !self.ids.contains(&id) {
            return;
        }
        self.bytes.insert(id.clone(), item["bytes"].as_f64().unwrap_or(0.0).max(0.0) as u64);
        if let Some(total) = item["total"].as_f64().filter(|t| *t > 0.0) {
            self.totals.insert(id.clone(), total as u64);
            self.estimates.remove(&id);
        } else if let Some(estimate) = item["estimate"].as_f64().filter(|e| *e > 0.0 && *e < u64::MAX as f64) {
            self.totals.insert(id.clone(), estimate as u64);
            self.estimates.insert(id);
        }
        let written = self.ids.iter().filter_map(|i| self.bytes.get(i)).sum();
        let total = self.ids.iter().all(|i| self.totals.contains_key(i)).then(|| self.ids.iter().map(|i| self.totals[i]).sum());
        let speed = item["speed"].as_f64().filter(|s| s.is_finite()).unwrap_or(0.0).max(0.0);
        events(MediaEvent::Progress { written, total, estimated: !self.estimates.is_empty(), bytes_per_second: speed });
    }
}

/// The browser's cookies in Netscape format for yt-dlp, deleted when dropped.
struct CookieFile(Option<PathBuf>);

impl CookieFile {
    fn write(session: Option<&Session>, page: &str) -> Result<CookieFile, String> {
        let Some(session) = session.filter(|s| !s.cookies.is_empty()) else { return Ok(CookieFile(None)) };
        let url = Url::parse(page).map_err(|_| "Invalid video address.")?;
        session.validate(&url).map_err(|e| e.message)?;
        let mut text = String::from("# Netscape HTTP Cookie File\n");
        for c in &session.cookies {
            let expires = c.expires_unix_seconds.filter(|e| *e > 0.0).map_or(0, |e| e as i64);
            text.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\n", c.domain, if c.host_only { "FALSE" } else { "TRUE" }, c.path, if c.secure { "TRUE" } else { "FALSE" }, expires, c.name, c.value));
        }
        let path = std::env::temp_dir().join(format!("flux-video-{:x}{:x}.cookies.txt", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()));
        std::fs::write(&path, text).map_err(|e| format!("The cookies could not be passed to the video tool: {e}"))?;
        Ok(CookieFile(Some(path)))
    }
    fn path(&self) -> Option<&Path> {
        self.0.as_deref()
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Sites behind Cloudflare and similar refuse ordinary clients; yt-dlp can present Chrome's TLS signature.
fn blocked(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("http error 403") || lower.contains("cloudflare") || error.contains("--impersonate") || lower.contains("http error 429")
}

/// The useful line of a tool's error output, without media addresses.
pub fn failure(error: &str) -> String {
    static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)https?://\S+").unwrap());
    let text = URL.replace_all(error, "[media URL]");
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut message = lines.iter().rev().find(|l| l.starts_with("ERROR:")).or(lines.last()).map_or("Video tool failed.".to_string(), |l| l.to_string());
    if message.to_ascii_lowercase().contains("unsupported url") {
        message = "No video was found on this page. Use the direct file link, or open the page in the browser and use the video panel.".into();
    }
    message.chars().take(700).collect()
}

fn format_arguments(selection: &Selection) -> Vec<String> {
    if !selection.is_preset() {
        let mut args = vec!["--format".to_string(), selection.format_id.clone()];
        if selection.format_id.contains('+') {
            args.extend(["--merge-output-format".into(), if selection.extension == "m4a" { "mp4".into() } else { selection.extension.clone() }]);
        }
        return args;
    }
    if selection.audio_only {
        return vec!["--format".into(), "ba[ext=m4a]/ba[acodec^=mp4a]/ba/b".into()];
    }
    // Prefer MP4/M4A so the tools copy streams without re-encoding; an exact cut also prefers H.264,
    // which the bundled FFmpeg can decode and re-encode.
    let codec = if selection.clip_exact { "vcodec:h264," } else { "" };
    let sort = match selection.preset_height() {
        Some(height) => format!("{codec}res:{height},ext:mp4:m4a"),
        None => format!("{codec}ext:mp4:m4a"),
    };
    vec!["--format".into(), "bv*+ba/b".into(), "--format-sort".into(), sort]
}

/// Subtitles (the site's own, or automatic ones when that is all there is) put into the file.
/// Saved as files beside the video ("Name.en.vtt", which players load on their own): the bundled
/// FFmpeg has no subtitle support to put them inside. Not for clips, whose timing would not match.
fn subtitle_arguments(selection: &Selection) -> Vec<String> {
    match selection.subtitles.as_deref().filter(|l| !selection.audio_only && !selection.is_clip() && formats::valid_languages(l)) {
        Some(languages) => ["--write-subs", "--write-auto-subs", "--sub-langs", languages, "--ignore-errors"].map(String::from).to_vec(),
        None => Vec::new(),
    }
}

/// `result.en.vtt` in the work folder becomes `<video name>.en.vtt` beside the video.
fn move_subtitles(work: &Path, video: &Path) {
    let Ok(entries) = std::fs::read_dir(work) else { return };
    let stem = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("result.") else { continue };
        let Some((language, ext)) = rest.rsplit_once('.') else { continue };
        if !matches!(ext, "vtt" | "srt" | "ass") || !formats::valid_languages(language) {
            continue;
        }
        let target = free_path(&video.with_file_name(format!("{stem}.{language}.{ext}")));
        let _ = std::fs::rename(entry.path(), target);
    }
}

fn seconds(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn clip_arguments(selection: &Selection) -> Vec<String> {
    if !selection.is_clip() {
        return Vec::new();
    }
    let end = selection.clip_end.map_or("inf".to_string(), seconds);
    vec!["--download-sections".into(), format!("*{}-{end}", seconds(selection.clip_start.unwrap_or(0.0)))]
}

/// `path`, or "name (n).ext" when taken.
fn free_path(path: &Path) -> PathBuf {
    let taken = |p: &Path| p.exists() || flux_engine::file::part_path(p).exists();
    if !taken(path) {
        return path.to_path_buf();
    }
    let folder = path.parent().unwrap_or(Path::new(""));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..10_000).map(|n| folder.join(format!("{stem} ({n}){ext}"))).find(|p| !taken(p)).unwrap_or_else(|| path.to_path_buf())
}

async fn copy_with_progress(from: &Path, to: &Path, events: &impl Fn(MediaEvent), cancel: &CancellationToken) -> Result<(), String> {
    let mut source = tokio::fs::File::open(from).await.map_err(|e| e.to_string())?;
    let length = source.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut target = tokio::fs::File::create(to).await.map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut copied = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(String::new());
        }
        let read = source.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        target.write_all(&buffer[..read]).await.map_err(|e| e.to_string())?;
        copied += read as u64;
        events(MediaEvent::Phase { text: "Copying to the destination drive".into(), fraction: (length > 0).then(|| copied as f64 / length as f64) });
    }
    target.sync_all().await.map_err(|e| e.to_string())
}

async fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_prefer_copyable_containers_and_h264_for_exact_cuts() {
        assert_eq!(format_arguments(&Selection::preset(Some(720), false)), ["--format", "bv*+ba/b", "--format-sort", "res:720,ext:mp4:m4a"]);
        let mut exact = Selection::preset(None, false);
        exact.clip_exact = true;
        assert_eq!(format_arguments(&exact)[3], "vcodec:h264,ext:mp4:m4a");
        assert_eq!(format_arguments(&Selection::preset(None, true))[1], "ba[ext=m4a]/ba[acodec^=mp4a]/ba/b");
        let pair = Selection { format_id: "137+140".into(), extension: "m4a".into(), ..Default::default() };
        assert_eq!(format_arguments(&pair), ["--format", "137+140", "--merge-output-format", "mp4"]);
    }

    #[test]
    fn subtitles_are_saved_beside_the_video_but_not_for_audio_or_clips() {
        let with = Selection { subtitles: Some("en,fr".into()), ..Selection::preset(Some(720), false) };
        assert_eq!(subtitle_arguments(&with)[..4], ["--write-subs", "--write-auto-subs", "--sub-langs", "en,fr"]);
        assert!(subtitle_arguments(&Selection { subtitles: Some("en".into()), ..Selection::preset(None, true) }).is_empty());
        assert!(subtitle_arguments(&Selection { clip_start: Some(5.0), ..with.clone() }).is_empty());

        let dir = std::env::temp_dir().join(format!("flux-subs-{}", std::process::id()));
        let work = dir.join(".flux-video-1");
        std::fs::create_dir_all(&work).unwrap();
        for name in ["result.en.vtt", "result.pt-BR.vtt", "result.mp4.part", "result.f137.mp4"] {
            std::fs::write(work.join(name), b"x").unwrap();
        }
        move_subtitles(&work, &dir.join("Talk (720p).mp4"));
        assert!(dir.join("Talk (720p).en.vtt").exists() && dir.join("Talk (720p).pt-BR.vtt").exists());
        assert!(work.join("result.f137.mp4").exists(), "only subtitles move");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clips_become_download_sections() {
        let clip = Selection { clip_start: Some(13.4), clip_end: Some(21.75), ..Default::default() };
        assert_eq!(clip_arguments(&clip), ["--download-sections", "*13.4-21.75"]);
        let open = Selection { clip_start: Some(5.0), ..Default::default() };
        assert_eq!(clip_arguments(&open)[1], "*5-inf");
        assert!(clip_arguments(&Selection::default()).is_empty());
    }

    #[test]
    fn errors_keep_the_useful_line_without_addresses() {
        let text = "WARNING: x\nERROR: [generic] Unable to download https://cdn.test/a?sig=1: HTTP Error 403\nnoise";
        assert_eq!(failure(text), "ERROR: [generic] Unable to download [media URL] HTTP Error 403");
        assert!(blocked(text));
        assert!(failure("ERROR: Unsupported URL: https://x.test").starts_with("No video was found"));
    }

    #[test]
    fn progress_of_both_tracks_adds_up() {
        let events = std::cell::RefCell::new(Vec::new());
        let push = |e: MediaEvent| events.borrow_mut().push(e);
        let mut tracker = Tracker::new(Vec::new());
        let mut file = None;
        tracker.line("FLUXFORMAT:\"137+140\"", &mut file, &push);
        tracker.line(r#"FLUXPROGRESS:{"format":"137","bytes":100,"total":1000,"estimate":null,"speed":50.0}"#, &mut file, &push);
        tracker.line(r#"FLUXPROGRESS:{"format":"140","bytes":20,"total":null,"estimate":200.0,"speed":10.0}"#, &mut file, &push);
        tracker.line("FLUXFILE:\"C:\\\\work\\\\result.mp4\"", &mut file, &push);
        let events = events.into_inner();
        assert_eq!(events[0], MediaEvent::Tracks(vec!["137".into(), "140".into()]));
        assert_eq!(events[1], MediaEvent::Progress { written: 100, total: None, estimated: false, bytes_per_second: 50.0 });
        assert_eq!(events[2], MediaEvent::Progress { written: 120, total: Some(1200), estimated: true, bytes_per_second: 10.0 });
        assert_eq!(file, Some(PathBuf::from(r"C:\work\result.mp4")));
    }
}
