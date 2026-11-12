//! End-to-end engine checks against the local lab server (ported from Flux 1.x's integration suite).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flux_engine::{Engine, ErrorKind, Event, HostLimits, Job, Options, Outcome};
use labserver::{LabServer, expected};
use tokio_util::sync::CancellationToken;
use url::Url;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flux-engine-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(flux_engine::file::part_path(&path));
    path
}

fn engine() -> Engine {
    Engine::new(Options { retry_base: Duration::from_millis(20), checkpoint_every: Duration::from_millis(100), ..Options::default() }, HostLimits::new([]))
}

fn job(server: &LabServer, name: &str, query: &str, connections: usize) -> Job {
    let mut job = Job::new(Url::parse(&server.url(name, query)).unwrap(), temp(name));
    job.connections = connections;
    job
}

async fn run(engine: &Engine, job: &mut Job) -> flux_engine::Result<Outcome> {
    engine.download(job, |_| {}, CancellationToken::new()).await
}

#[tokio::test]
async fn several_connections_produce_an_identical_file_without_a_part_file_left() {
    let server = LabServer::start().await.unwrap();
    let engine = engine();
    let mut job = job(&server, "multi.bin", "size=12000003&seed=7&slow_us=500", 6);
    let outcome = run(&engine, &mut job).await.unwrap();
    assert!(matches!(outcome, Outcome::Completed { bytes: 12000003, .. }));
    assert_eq!(std::fs::read(&job.destination).unwrap(), expected(12000003, 7));
    assert!(!flux_engine::file::part_path(&job.destination).exists(), "no assembly leftovers");
    assert!(server.stats("multi.bin").max_concurrent >= 4, "connections were not used in parallel");
}

#[tokio::test]
async fn pause_keeps_progress_and_resume_continues_from_it() {
    let server = LabServer::start().await.unwrap();
    let engine = engine();
    let mut job = job(&server, "pause.bin", "size=8000000&seed=3&slow_us=8000", 4);
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    let checkpoints = Arc::new(Mutex::new(0));
    let seen = checkpoints.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(600)).await; stopper.cancel(); });
    let outcome = engine.download(&mut job, move |e| if let Event::Checkpoint(_) = e { *seen.lock().unwrap() += 1 }, cancel).await.unwrap();
    assert_eq!(outcome, Outcome::Paused);
    let saved = job.resume.written();
    assert!(saved > 0 && saved < 8000000, "paused with {saved} bytes");
    assert!(*checkpoints.lock().unwrap() > 0);
    let before = server.stats("pause.bin").requests;
    // The saved state survives serialization, as it does through the app's history.
    job.resume = serde_json::from_str(&serde_json::to_string(&job.resume).unwrap()).unwrap();
    let mut fast = job.clone();
    fast.url = Url::parse(&server.url("pause.bin", "size=8000000&seed=3")).unwrap();
    run(&engine, &mut fast).await.unwrap();
    assert_eq!(std::fs::read(&fast.destination).unwrap(), expected(8000000, 3));
    assert!(server.stats("pause.bin").requests > before);
}

#[tokio::test]
async fn a_server_without_ranges_is_downloaded_on_one_connection() {
    let server = LabServer::start().await.unwrap();
    let mut job = job(&server, "norange.bin", "size=3000000&no_range=1", 8);
    run(&engine(), &mut job).await.unwrap();
    assert_eq!(std::fs::read(&job.destination).unwrap(), expected(3000000, 1));
    assert!(!job.resume.ranged);
}

#[tokio::test]
async fn a_site_refusing_extra_connections_finishes_with_fewer_and_is_remembered() {
    let server = LabServer::start().await.unwrap();
    let learned = Arc::new(Mutex::new(None));
    let seen = learned.clone();
    let engine = Engine::new(Options { retry_base: Duration::from_millis(20), ..Options::default() }, HostLimits::new([]).on_learned(move |h, n| *seen.lock().unwrap() = Some((h.to_string(), n))));
    let mut first = job(&server, "limited.bin", "size=16000000&max_conn=2&slow_us=200", 8);
    run(&engine, &mut first).await.unwrap();
    assert_eq!(std::fs::read(&first.destination).unwrap(), expected(16000000, 1));
    let stats = server.stats("limited.bin");
    assert!(stats.refused > 0, "the server never refused, so nothing was tested");
    let (host, limit) = learned.lock().unwrap().clone().expect("limit learned");
    assert_eq!(host, "127.0.0.1");
    assert!(limit <= 2, "learned {limit}");
    let mut second = job(&server, "limited2.bin", "size=16000000&max_conn=2", 8);
    run(&engine, &mut second).await.unwrap();
    assert_eq!(server.stats("limited2.bin").refused, 0, "a remembered site was asked for too many connections again");
}

#[tokio::test]
async fn a_site_accepting_only_browser_navigations_is_downloaded() {
    let server = LabServer::start().await.unwrap();
    let mut job = job(&server, "strict.bin", "size=2000000&need_nav=1", 4);
    run(&engine(), &mut job).await.unwrap();
    assert_eq!(std::fs::read(&job.destination).unwrap(), expected(2000000, 1));
    assert!(job.session.unwrap().headers.iter().any(|h| h.value == "navigate"), "the accepted form was not kept");
}

#[tokio::test]
async fn a_bot_check_refusing_the_browser_identity_is_passed_honestly() {
    let server = LabServer::start().await.unwrap();
    let mut job = job(&server, "botcheck.bin", "size=3000000&bot_check=1", 4);
    run(&engine(), &mut job).await.unwrap();
    assert_eq!(std::fs::read(&job.destination).unwrap(), expected(3000000, 1));
    assert_eq!(job.session.unwrap().user_agent.as_deref(), Some(flux_engine::HONEST_USER_AGENT), "the accepted identity was not kept for the transfer");
}

#[tokio::test]
async fn temporary_failures_and_cut_connections_are_retried() {
    let server = LabServer::start().await.unwrap();
    let engine = engine();
    let mut failing = job(&server, "flaky.bin", "size=1500000&fail=2", 2);
    run(&engine, &mut failing).await.unwrap();
    assert_eq!(std::fs::read(&failing.destination).unwrap(), expected(1500000, 1));
    let mut cut = job(&server, "cut.bin", "size=6000000&drop_at=2500000", 1);
    run(&engine, &mut cut).await.unwrap();
    assert_eq!(std::fs::read(&cut.destination).unwrap(), expected(6000000, 1));
}

#[tokio::test]
async fn a_changed_file_restarts_safely_instead_of_mixing_versions() {
    let server = LabServer::start().await.unwrap();
    let engine = engine();
    let mut job = job(&server, "changed.bin", "size=6000000&seed=1&slow_us=3000", 3);
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(400)).await; stopper.cancel(); });
    engine.download(&mut job, |_| {}, cancel).await.unwrap();
    assert!(job.resume.written() > 0);
    job.url = Url::parse(&server.url("changed.bin", "size=6000000&seed=2")).unwrap();
    run(&engine, &mut job).await.unwrap();
    assert_eq!(std::fs::read(&job.destination).unwrap(), expected(6000000, 2));
}

#[tokio::test]
async fn pages_missing_files_and_checksums_are_reported_clearly() {
    let server = LabServer::start().await.unwrap();
    let engine = engine();
    let mut page = job(&server, "page.bin", "html=1", 2);
    assert_eq!(run(&engine, &mut page).await.unwrap_err().kind, ErrorKind::Protocol);

    let mut wrong = job(&server, "sum.bin", "size=100000", 1);
    wrong.expected_sha256 = Some("0".repeat(64));
    assert_eq!(run(&engine, &mut wrong).await.unwrap_err().kind, ErrorKind::Integrity);
    assert!(!wrong.destination.exists(), "a file failing its checksum must not be published");

    let mut right = job(&server, "sum2.bin", "size=100000", 1);
    let data = expected(100000, 1);
    right.expected_sha256 = Some({ use sha2::Digest; sha2::Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect() });
    assert!(matches!(run(&engine, &mut right).await.unwrap(), Outcome::Completed { sha256: Some(_), .. }));

    let mut existing = job(&server, "exists.bin", "size=1000", 1);
    std::fs::write(&existing.destination, b"keep me").unwrap();
    assert_eq!(run(&engine, &mut existing).await.unwrap_err().kind, ErrorKind::Storage);
    assert_eq!(std::fs::read(&existing.destination).unwrap(), b"keep me");
}

#[tokio::test]
async fn speed_limit_is_respected() {
    let server = LabServer::start().await.unwrap();
    let mut job = job(&server, "limit.bin", "size=1000000", 4);
    job.limiter.set_rate(400_000);
    let start = std::time::Instant::now();
    run(&engine(), &mut job).await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(1500), "1 MB at 400 KB/s took {:?}", start.elapsed());
}

/// A minimal HTTP forward proxy: checks the credentials on each connection's first request,
/// counts connections, then relays bytes to the origin (which accepts absolute-form requests).
async fn start_proxy(expected_auth: &'static str) -> (String, Arc<Mutex<(usize, usize)>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let counts = Arc::new(Mutex::new((0usize, 0usize))); // (connections, refused)
    let seen = counts.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else { return };
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && head.len() < 16384 {
                    if client.read(&mut byte).await.unwrap_or(0) == 0 { return }
                    head.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&head).to_string();
                let authorized = text.lines().any(|l| l.eq_ignore_ascii_case(&format!("proxy-authorization: Basic {expected_auth}")));
                if !authorized {
                    seen.lock().unwrap().1 += 1;
                    let _ = client.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                seen.lock().unwrap().0 += 1;
                let target = text.split_whitespace().nth(1).and_then(|u| Url::parse(u).ok()).unwrap();
                let mut origin = tokio::net::TcpStream::connect((target.host_str().unwrap(), target.port_or_known_default().unwrap())).await.unwrap();
                origin.write_all(&head).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut client, &mut origin).await;
            });
        }
    });
    (format!("127.0.0.1:{}", address.port()), counts)
}

#[tokio::test]
async fn downloads_go_through_the_proxy_with_its_credentials() {
    let server = LabServer::start().await.unwrap();
    // The lab server is on 127.0.0.1, which never uses a proxy; reach it by another name.
    let url = server.url("proxied.bin", "size=3000000&seed=5").replace("127.0.0.1", "lab.localtest.me");
    let (proxy, counts) = start_proxy("Zmx1eDpzM2NyZXQ=").await; // flux:s3cret
    let engine = engine();
    let mut job = Job::new(Url::parse(&url).unwrap(), temp("proxied.bin"));
    job.connections = 3;

    // Wrong password: the proxy refuses, and the download says so.
    engine.set_network(Some(&flux_engine::Proxy { url: proxy.clone(), username: "flux".into(), password: "wrong".into() }), None).unwrap();
    assert!(run(&engine, &mut job).await.is_err());
    assert!(counts.lock().unwrap().1 > 0);

    let mut job = Job::new(Url::parse(&url).unwrap(), temp("proxied.bin"));
    job.connections = 3;
    engine.set_network(Some(&flux_engine::Proxy { url: proxy, username: "flux".into(), password: "s3cret".into() }), Some("FluxTest/1.0")).unwrap();
    assert!(matches!(run(&engine, &mut job).await.unwrap(), Outcome::Completed { bytes: 3000000, .. }));
    assert_eq!(std::fs::read(&job.destination).unwrap(), expected(3000000, 5));
    assert!(counts.lock().unwrap().0 > 0, "nothing went through the proxy");

    assert!(flux_engine::Proxy { url: "ftp://x:1".into(), ..Default::default() }.normalized().is_err());
    assert_eq!(flux_engine::Proxy { url: "10.0.0.1:8080".into(), username: "a b".into(), password: "p@ss".into() }.with_credentials().unwrap(), "http://a%20b:p%40ss@10.0.0.1:8080/");
}

/// A tiny site: two pages linking to files, a page elsewhere and a type that is not wanted.
async fn start_site() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 4096];
                let n = socket.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = match path.as_str() {
                    "/" => format!(r#"<a href="/files/a.zip">a</a> <a href='page2.html'>next</a> <a href="https://other.test/x.zip">x</a> <a href="notes.txt">t</a> <a href="http://127.0.0.1:{port}/files/a.zip#again">dup</a>"#),
                    "/page2.html" => r#"<a HREF="/files/b.PDF?v=2&amp;k=1">b</a>"#.to_string(),
                    _ => String::new(),
                };
                let status = if body.is_empty() { "404 Not Found" } else { "200 OK" };
                let reply = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    format!("http://127.0.0.1:{port}/")
}

#[tokio::test]
async fn the_grabber_follows_one_site_and_lists_the_wanted_files() {
    let root = start_site().await;
    let files = engine().grab(&Url::parse(&root).unwrap(), "zip pdf", None).await.unwrap();
    assert_eq!(files, [format!("{root}files/a.zip"), format!("{root}files/b.PDF?v=2&k=1")]);
    assert!(engine().grab(&Url::parse(&root).unwrap(), " ", None).await.is_err(), "no types, nothing to look for");
}

/// One connection crawls (80 KB/s) while the others are fast: the free connections take over what
/// it has left, instead of everyone waiting for it ("stuck at 99 %"). Alone it would need about 13 s.
#[tokio::test]
async fn idle_connections_finish_a_slow_part() {
    let server = LabServer::start().await.unwrap();
    let engine = engine();
    let size = 16_000_000u64;
    let mut job = job(&server, "endgame.bin", &format!("size={size}&seed=5&lag_from={}", size - size / 4), 4);
    let started = std::time::Instant::now();
    let outcome = run(&engine, &mut job).await.unwrap();
    let took = started.elapsed();
    assert!(matches!(outcome, Outcome::Completed { bytes, .. } if bytes == size));
    assert_eq!(std::fs::read(&job.destination).unwrap(), expected(size, 5));
    assert!(took < Duration::from_secs(8), "the slow part held the download for {took:?}");
}
