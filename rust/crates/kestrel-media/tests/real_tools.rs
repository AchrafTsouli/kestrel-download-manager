//! Checks against the real yt-dlp and FFmpeg and a public test stream (the same checks Flux 1.0
//! was released with). They need the network and the tools, so they run only on request:
//!
//!     $env:KESTREL_VIDEO_TOOLS="…\video-tools"; cargo test -p kestrel-media --test real_tools -- --ignored --test-threads 1

use std::path::PathBuf;
use std::sync::Mutex;

use kestrel_media::{MediaEvent, Outcome, Selection, Tools, VideoJob};
use tokio_util::sync::CancellationToken;

const STREAM: &str = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8";

fn tools() -> Tools {
    let dir = std::env::var("KESTREL_VIDEO_TOOLS").expect("set KESTREL_VIDEO_TOOLS to a video-tools folder");
    let tools = Tools::new(dir, std::env::temp_dir().join("kestrel-media-test-updates"));
    assert!(tools.available(), "the folder lacks yt-dlp, FFmpeg or QuickJS");
    tools
}

fn job(name: &str, selection: Selection) -> VideoJob {
    let folder = std::env::temp_dir().join(format!("kestrel-media-test-{}", std::process::id()));
    VideoJob { page: STREAM.into(), destination: folder.join(name), work: folder.join(format!(".kestrel-video-{name}")), selection, session: None, connections: 4, retries: 3, speed_limit: 0, expected_sha256: None }
}

async fn duration(tools: &Tools, file: &PathBuf) -> f64 {
    let out = std::process::Command::new(tools.directory.join(format!("ffprobe{}", std::env::consts::EXE_SUFFIX))).args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"]).arg(file).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[tokio::test]
#[ignore]
async fn a_stream_lists_its_qualities() {
    let catalog = tools().formats(STREAM, None, &CancellationToken::new()).await.unwrap();
    // Five H.264 renditions with sound, best first. A bare manifest has no duration; the
    // browser names it after the page's tab.
    let labels: Vec<&str> = catalog.formats.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(labels.first(), Some(&"MP4 · 1080p · H.264 · video + audio"), "{labels:#?}");
    assert!(labels.contains(&"MP4 · 720p · H.264 · video + audio"), "{labels:#?}");
    assert!(catalog.formats.iter().all(|f| f.extension == "mp4" && !f.no_audio));
}

#[tokio::test]
#[ignore]
async fn a_preset_and_both_clip_cuts_download() {
    let tools = tools();
    let mut clip = Selection::preset(Some(360), false);
    clip.clip_start = Some(13.4);
    clip.clip_end = Some(21.7);

    let phases = Mutex::new(Vec::new());
    let events = |e: MediaEvent| if let MediaEvent::Phase { text, .. } = e { phases.lock().unwrap().push(text) };
    let mut fast = job("fast.mp4", clip.clone());
    let Outcome::Completed { path, bytes, .. } = tools.download(&mut fast, events, CancellationToken::new()).await.unwrap() else { panic!("paused") };
    assert!(bytes > 50_000);
    // A keyframe cut keeps a little more than asked.
    let length = duration(&tools, &path).await;
    assert!((8.0..14.0).contains(&length), "fast cut is {length} s");
    assert!(!fast.work.exists(), "the work folder is removed");

    if tools.supports_exact_cut().await {
        let mut exact_clip = clip;
        exact_clip.clip_exact = true;
        let mut exact = job("exact.mp4", exact_clip);
        let Outcome::Completed { path, .. } = tools.download(&mut exact, |_| {}, CancellationToken::new()).await.unwrap() else { panic!("paused") };
        let length = duration(&tools, &path).await;
        assert!((7.8..8.8).contains(&length), "exact cut is {length} s");
    }
    let _ = std::fs::remove_dir_all(fast.destination.parent().unwrap());
}

#[tokio::test]
#[ignore]
async fn a_paused_download_keeps_its_parts() {
    let tools = tools();
    let mut job = job("paused.mp4", Selection::preset(Some(360), false));
    job.speed_limit = 200_000;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        stop.cancel();
    });
    assert_eq!(tools.download(&mut job, |_| {}, cancel).await.unwrap(), Outcome::Paused);
    assert!(std::fs::read_dir(&job.work).unwrap().next().is_some(), "parts are kept for --continue");
    let _ = std::fs::remove_dir_all(job.destination.parent().unwrap());
}
