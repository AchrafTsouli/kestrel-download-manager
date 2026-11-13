//! Downloads from real sites, to notice early when a site changes how it serves files. They need the
//! network, so they run only on request (and every night in CI, .github/workflows/nightly.yml):
//!
//!     cargo test -p kestrel-engine --test real_sites -- --ignored --test-threads 1

use std::time::Duration;

use kestrel_engine::{Engine, HostLimits, Job, Options, Outcome};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;

fn destination(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kestrel-real-sites-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(kestrel_engine::file::part_path(&path));
    path
}

fn sha256_of(path: &std::path::Path) -> String {
    Sha256::digest(std::fs::read(path).unwrap()).iter().map(|b| format!("{b:02x}")).collect()
}

/// GitHub releases: a redirect to signed storage links, many connections, a pause and a resume.
/// The file is Kestrel's own latest installer, checked against the checksum the release key signed.
#[tokio::test]
#[ignore]
async fn a_github_release_downloads_pauses_resumes_and_matches_its_signed_checksum() {
    let release = kestrel_update::check("").await.expect("the update feed");
    assert!(release.signed, "the published feed is signed");
    let engine = Engine::new(Options::default(), HostLimits::new([]));
    let mut job = Job::new(Url::parse(&release.url).unwrap(), destination("kestrel-setup.exe"));
    job.connections = 8;

    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(1500)).await; stopper.cancel(); });
    let first = engine.download(&mut job, |_| {}, cancel).await.unwrap();
    if first == Outcome::Paused {
        assert!(job.resume.written() > 0, "nothing arrived in 1.5 s");
    }
    if !matches!(first, Outcome::Completed { .. }) {
        let outcome = engine.download(&mut job, |_| {}, CancellationToken::new()).await.unwrap();
        assert!(matches!(outcome, Outcome::Completed { .. }), "{outcome:?}");
    }
    assert_eq!(sha256_of(&job.destination), release.sha256, "the installer matches its signed checksum");
    let _ = std::fs::remove_file(&job.destination);
}

/// A server that sends a generated file in one stream (no known name, possibly no ranges).
#[tokio::test]
#[ignore]
async fn a_streamed_download_arrives_whole() {
    let engine = Engine::new(Options::default(), HostLimits::new([]));
    let mut job = Job::new(Url::parse("https://speed.cloudflare.com/__down?bytes=5000000").unwrap(), destination("cloudflare.bin"));
    job.connections = 4;
    let outcome = engine.download(&mut job, |_| {}, CancellationToken::new()).await.unwrap();
    assert!(matches!(outcome, Outcome::Completed { bytes: 5000000, .. }), "{outcome:?}");
    assert_eq!(std::fs::metadata(&job.destination).unwrap().len(), 5000000);
    let _ = std::fs::remove_file(&job.destination);
}
