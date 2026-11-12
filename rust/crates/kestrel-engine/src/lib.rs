//! Kestrel's download engine.
//!
//! ```text
//! download()
//!  ├─ probe            size, byte ranges, validators, file name; 403 → retried as a browser navigation
//!  ├─ Plan             segments; a free connection splits the largest remaining part (plan.rs)
//!  ├─ workers × N      keep-alive connections writing at their offsets into one sparse part file
//!  ├─ HostLimits       a refused connection hands its part to the others; the site's limit is remembered
//!  ├─ Limiter          global and per-download token buckets
//!  └─ checkpoints      data flushed to disk before the resume point is reported (crash-safe resume)
//! ```

pub mod error;
pub mod file;
mod grabber;
pub mod limits;
pub mod plan;
pub mod session;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode, redirect};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;

mod page;
pub use error::{Error, ErrorKind, Result};
pub use limits::{HostLimits, Limiter};
pub use plan::{Plan, Segment};
pub use session::{Cookie, Header, Session};

/// Used when a bot check refuses the browser identity (see `probe`).
pub const HONEST_USER_AGENT: &str = concat!("Kestrel/", env!("CARGO_PKG_VERSION"), " (+https://github.com/AchrafTsouli/kestrel-download-manager)");

fn is_bot_challenge(response: &reqwest::Response) -> bool {
    let headers = response.headers();
    headers.contains_key("cf-mitigated") || headers.get(header::SERVER).and_then(|v| v.to_str().ok()).is_some_and(|s| s.eq_ignore_ascii_case("cloudflare"))
}

pub const DEFAULT_USER_AGENT: &str ="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Bytes a connection gathers before writing them.
const WRITE_BUFFER: usize = 1 << 20;
/// At the end, a part is shared with an idle connection when it still needs this long at its own speed...
const ENDGAME_SECONDS: u64 = 3;
/// ...and at least twice this much is left.
const ENDGAME_SPLIT: u64 = 64 << 10;

#[derive(Debug, Clone)]
pub struct Options {
    pub max_connections: usize,
    pub max_segments: usize,
    /// Parts smaller than this are not split further.
    pub min_split: u64,
    pub max_retries: u32,
    pub retry_base: Duration,
    pub max_retry_delay: Duration,
    pub headers_timeout: Duration,
    pub read_timeout: Duration,
    pub checkpoint_every: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_connections: 16,
            max_segments: 64,
            min_split: 512 << 10,
            max_retries: 3,
            retry_base: Duration::from_secs(2),
            max_retry_delay: Duration::from_secs(60),
            headers_timeout: Duration::from_secs(30),
            // A connection that stays silent this long is dropped and its part handed on.
            read_timeout: Duration::from_secs(20),
            checkpoint_every: Duration::from_secs(2),
        }
    }
}

/// Everything needed to continue a download later; saved by the app at each checkpoint.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Resume {
    pub total: Option<u64>,
    pub ranged: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub final_url: Option<String>,
    pub plan: Plan,
}

impl Resume {
    pub fn written(&self) -> u64 {
        self.plan.written()
    }
}

/// One download as the engine sees it.
#[derive(Debug, Clone)]
pub struct Job {
    pub url: Url,
    pub destination: PathBuf,
    pub connections: usize,
    pub session: Option<Session>,
    pub expected_sha256: Option<String>,
    pub limiter: Limiter,
    pub resume: Resume,
}

impl Job {
    pub fn new(url: Url, destination: impl Into<PathBuf>) -> Job {
        Job { url, destination: destination.into(), connections: 8, session: None, expected_sha256: None, limiter: Limiter::default(), resume: Resume::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Probed {
    pub total: Option<u64>,
    pub ranged: bool,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub final_url: String,
}

#[derive(Debug, Clone)]
pub enum Event {
    Probed(Probed),
    Progress { written: u64, total: Option<u64>, bytes_per_second: f64, connections: usize },
    /// Flushed to disk: save this to resume after a crash.
    Checkpoint(Resume),
    Note(String),
    Retrying { attempt: u32, delay: Duration, error: Error },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Completed { path: PathBuf, bytes: u64, sha256: Option<String> },
    Paused,
}

type Events = Arc<dyn Fn(Event) + Send + Sync>;

/// A proxy every download goes through: `http://`, `https://` or `socks5://` (a bare `host:port`
/// means HTTP), with optional credentials. Local addresses never use it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Proxy {
    pub url: String,
    pub username: String,
    pub password: String,
}

impl Proxy {
    /// The address with a scheme, or an error a person can act on.
    pub fn normalized(&self) -> std::result::Result<String, String> {
        let text = self.url.trim();
        let full = if text.contains("://") { text.to_string() } else { format!("http://{text}") };
        let url = Url::parse(&full).map_err(|_| format!("The proxy address \"{text}\" is not valid. Use host:port, http://host:port or socks5://host:port."))?;
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") || url.host_str().is_none() {
            return Err("The proxy must be an http://, https:// or socks5:// address.".into());
        }
        Ok(full)
    }
    /// The address with the credentials in it, for tools that take a single URL (yt-dlp, aria2).
    pub fn with_credentials(&self) -> std::result::Result<String, String> {
        let mut url = Url::parse(&self.normalized()?).map_err(|e| e.to_string())?;
        if !self.username.is_empty() {
            let _ = url.set_username(&self.username);
            let _ = url.set_password(Some(&self.password));
        }
        Ok(url.to_string())
    }
}

fn build_client(proxy: Option<&Proxy>, system_proxy: bool) -> std::result::Result<Client, String> {
    let mut builder = Client::builder()
        // Separate TCP connections: with HTTP/2 every "connection" would share one socket.
        .http1_only()
        .tcp_nodelay(true)
        .connect_timeout(Duration::from_secs(20))
        .pool_max_idle_per_host(64)
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 { attempt.error("too many redirects") }
            else if unsafe_redirect(&attempt) { attempt.error("unsafe redirect") }
            else { attempt.follow() }
        }));
    if let Some(proxy) = proxy {
        let mut all = reqwest::Proxy::all(proxy.normalized()?).map_err(|e| format!("The proxy cannot be used: {e}"))?;
        if !proxy.username.is_empty() {
            all = all.basic_auth(&proxy.username, &proxy.password);
        }
        builder = builder.proxy(all.no_proxy(reqwest::NoProxy::from_string("localhost,127.0.0.1,::1")));
    } else if !system_proxy {
        // No proxy chosen: no system proxy guessing either.
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| format!("The connection settings cannot be used: {e}"))
}

#[derive(Clone)]
pub struct Engine {
    client: Arc<RwLock<Client>>,
    /// Replaces the browser identity Kestrel presents (a browser session's own still wins).
    user_agent: Arc<RwLock<Option<HeaderValue>>>,
    pub options: Arc<Options>,
    pub host_limits: HostLimits,
    pub global_limiter: Limiter,
}

fn unsafe_redirect(attempt: &redirect::Attempt) -> bool {
    let next = attempt.url();
    let downgrade = attempt.previous().last().is_some_and(|prev| prev.scheme() == "https" && next.scheme() == "http");
    !matches!(next.scheme(), "http" | "https") || !next.username().is_empty() || next.password().is_some() || downgrade
}

impl Engine {
    pub fn new(options: Options, host_limits: HostLimits) -> Engine {
        let client = build_client(None, false).expect("HTTP client");
        Engine { client: Arc::new(RwLock::new(client)), user_agent: Arc::new(RwLock::new(None)), options: Arc::new(options), host_limits, global_limiter: Limiter::default() }
    }

    /// Applies the connection settings to the downloads that start from now on.
    pub fn set_network(&self, proxy: Option<&Proxy>, user_agent: Option<&str>) -> std::result::Result<(), String> {
        self.set_network_with(proxy, user_agent, false)
    }

    /// Like `set_network`; `system_proxy` uses Windows' proxy settings when no proxy is given.
    pub fn set_network_with(&self, proxy: Option<&Proxy>, user_agent: Option<&str>, system_proxy: bool) -> std::result::Result<(), String> {
        let agent = match user_agent.map(str::trim).filter(|a| !a.is_empty()) {
            Some(agent) => Some(HeaderValue::from_str(agent).map_err(|_| "The browser identity contains characters a header cannot carry.")?),
            None => None,
        };
        let client = build_client(proxy, system_proxy)?;
        *self.client.write().unwrap() = client;
        *self.user_agent.write().unwrap() = agent;
        Ok(())
    }

    fn headers(&self, job: &Job, target: &Url, session: Option<&Session>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let agent = self.user_agent.read().unwrap().clone().unwrap_or(HeaderValue::from_static(DEFAULT_USER_AGENT));
        headers.insert(header::USER_AGENT, agent);
        headers.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
        headers.insert(header::ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
        headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        if let Ok(origin) = HeaderValue::from_str(&format!("{}/", target.origin().ascii_serialization())) {
            headers.insert(header::REFERER, origin);
        }
        if let Some(session) = session.or(job.session.as_ref()) {
            session.apply(target, &mut headers);
        }
        headers
    }

    async fn send(&self, job: &Job, session: Option<&Session>, range: Option<(u64, Option<u64>)>, if_range: Option<&str>) -> Result<reqwest::Response> {
        let target = job.resume.final_url.as_deref().and_then(|u| Url::parse(u).ok()).unwrap_or_else(|| job.url.clone());
        let client = self.client.read().unwrap().clone();
        let mut request = client.get(target.clone()).headers(self.headers(job, &target, session));
        if let Some((start, end)) = range {
            request = request.header(header::RANGE, match end { Some(end) => format!("bytes={start}-{end}"), None => format!("bytes={start}-") });
            if let Some(validator) = if_range {
                request = request.header(header::IF_RANGE, validator);
            }
        }
        match tokio::time::timeout(self.options.headers_timeout, request.send()).await {
            Ok(response) => Ok(response?),
            Err(_) => Err(Error::network("The server did not answer before the timeout.")),
        }
    }

    fn check(response: &reqwest::Response) -> Result<()> {
        if response.status().is_success() {
            return Ok(());
        }
        let retry_after = response.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).map(Duration::from_secs);
        Err(Error::from_status(response.status().as_u16(), retry_after))
    }

    /// Size, range support, validators and name. Tries a one-byte range, then a plain request, then the
    /// form of a browser navigation (some sites accept only that); keeps whichever the site accepted.
    pub async fn probe(&self, job: &mut Job) -> Result<Probed> {
        if let Some(session) = &job.session {
            session.validate(&job.url)?;
        }
        job.resume.final_url = None;
        // A download page ("your download will start…") is followed to the file, at most 3 pages deep.
        let mut pages = 0;
        let response = loop {
        let mut response = self.send(job, None, Some((0, Some(0))), None).await?;
        let refused = |r: &reqwest::Response| matches!(r.status().as_u16(), 400 | 403 | 405 | 406 | 416) && !r.headers().contains_key(header::WWW_AUTHENTICATE);
        if refused(&response) {
            response = self.send(job, None, None, None).await?;
        }
        if matches!(response.status(), StatusCode::FORBIDDEN | StatusCode::NOT_ACCEPTABLE) && !response.headers().contains_key(header::WWW_AUTHENTICATE) {
            let navigation = job.session.clone().unwrap_or_else(|| Session::for_url(&job.url)).as_navigation();
            let retry = self.send(job, Some(&navigation), None, None).await?;
            if retry.status().is_success() {
                job.session = Some(navigation);
            }
            response = retry;
        }
        // Cloudflare's bot check challenges a request that claims to be a browser but does not have a
        // browser's network fingerprint; the same request with an honest program name often passes.
        if response.status() == StatusCode::FORBIDDEN && is_bot_challenge(&response) {
            let mut honest = job.session.clone().unwrap_or_else(|| Session::for_url(&job.url));
            honest.user_agent = Some(HONEST_USER_AGENT.into());
            honest.headers.clear();
            let retry = self.send(job, Some(&honest), None, None).await?;
            if retry.status().is_success() {
                job.session = Some(honest);
            }
            response = retry;
        }
        Engine::check(&response)?;
        if response.headers().get(header::CONTENT_ENCODING).is_some_and(|e| e != "identity") {
            return Err(Error::protocol("The server ignored the request for an uncompressed transfer."));
        }
        let headers = response.headers();
        let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
        let file_name = headers.get(header::CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()).and_then(disposition_name);
        let destination_is_page = job.destination.extension().is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
        if matches!(content_type.as_deref(), Some("text/html" | "application/xhtml+xml")) && file_name.is_none() && !destination_is_page {
            if pages < 3 {
                let here = response.url().clone();
                let header_refresh = headers.get(header::REFRESH).and_then(|v| v.to_str().ok()).and_then(|v| page::refresh(v, &here));
                let next = match header_refresh {
                    Some(next) => Some(next),
                    None => {
                        let body = read_some(response, 512 * 1024).await;
                        let text = String::from_utf8_lossy(&body);
                        page::meta_refresh(&text, &here).or_else(|| page::same_name_link(&text, &here, &job.url))
                    }
                };
                if let Some(next) = next {
                    pages += 1;
                    job.resume.final_url = Some(next.to_string());
                    continue;
                }
            }
            return Err(Error::protocol("The link returned a web page instead of a file. Capture the download again from the browser; it may be a page link or an expired link."));
        }
        break response;
        };
        let headers = response.headers();
        let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
        let file_name = headers.get(header::CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()).and_then(disposition_name);
        let partial = response.status() == StatusCode::PARTIAL_CONTENT;
        let total = if partial {
            let range = headers.get(header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).and_then(content_range).ok_or_else(|| Error::protocol("The server returned invalid file-size information."))?;
            if range.0 != 0 { return Err(Error::protocol("The server returned invalid file-size information.")); }
            range.2
        } else {
            response.content_length()
        };
        let etag = headers.get(header::ETAG).and_then(|v| v.to_str().ok()).filter(|v| !v.starts_with("W/")).map(str::to_string);
        let modified = headers.get(header::LAST_MODIFIED).and_then(|v| v.to_str().ok()).map(str::to_string);
        let final_url = response.url().to_string();
        let ranged = partial && total.is_some_and(|t| t > 0);
        // The previous resume point is only valid for the same file.
        let same = job.resume.total == total && job.resume.ranged && ranged
            && (job.resume.etag.is_none() || job.resume.etag == etag)
            && (job.resume.etag.is_some() || job.resume.last_modified.is_none() || job.resume.last_modified == modified);
        if !same {
            job.resume.plan = Plan::default();
        }
        job.resume.total = total;
        job.resume.ranged = ranged;
        job.resume.etag = etag;
        job.resume.last_modified = modified;
        job.resume.final_url = Some(final_url.clone());
        Ok(Probed { total, ranged, file_name, content_type, final_url })
    }

    /// Downloads `job` to its destination, retrying temporary failures. `job.resume` is kept current,
    /// so after `Paused` or an error the same job continues where it stopped.
    pub async fn download(&self, job: &mut Job, events: impl Fn(Event) + Send + Sync + 'static, cancel: CancellationToken) -> Result<Outcome> {
        let events: Events = Arc::new(events);
        let mut attempt = 0;
        loop {
            if cancel.is_cancelled() {
                return Ok(Outcome::Paused);
            }
            match self.attempt(job, events.clone(), cancel.clone()).await {
                Ok(outcome) => return Ok(outcome),
                Err(error) => {
                    let mut error = error;
                    // Refused while using several connections: try once more with one.
                    if error.kind == ErrorKind::AccessDenied && job.connections > 1 && job.resume.ranged {
                        job.connections = 1;
                        error.retryable = true;
                        error.retry_after = Some(Duration::from_secs(1));
                        self.host_limits.learn(&job.url, 1);
                    }
                    if !error.retryable || attempt >= self.options.max_retries || cancel.is_cancelled() {
                        return if cancel.is_cancelled() { Ok(Outcome::Paused) } else { Err(error) };
                    }
                    attempt += 1;
                    let delay = error.retry_after.unwrap_or(self.options.retry_base * 2u32.pow(attempt - 1)).min(self.options.max_retry_delay);
                    events(Event::Retrying { attempt, delay, error: error.clone() });
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancel.cancelled() => return Ok(Outcome::Paused),
                    }
                }
            }
        }
    }

    async fn attempt(&self, job: &mut Job, events: Events, cancel: CancellationToken) -> Result<Outcome> {
        let probed = self.probe(job).await?;
        events(Event::Probed(probed.clone()));
        let total = job.resume.total;
        let resuming = !job.resume.plan.segments.is_empty() && job.resume.written() > 0;
        let limit = self.host_limits.limit_for(&job.url).unwrap_or(self.options.max_connections);
        let connections = job.connections.clamp(1, self.options.max_connections).min(limit);
        if !resuming {
            job.resume.plan = Plan::new(total, job.resume.ranged, connections, 1 << 20);
        } else {
            events(Event::Note("Continuing from the saved position.".into()));
        }
        if let (Some(total), Some(free)) = (total, file::available_space(&job.destination))
            && free < total.saturating_sub(job.resume.written()) {
                return Err(Error::new(ErrorKind::DiskSpace, "Not enough free disk space for this file."));
            }
        let part = file::PartFile::open(&file::part_path(&job.destination), total, !resuming)?;
        self.run_workers(job, &part, connections, events.clone(), cancel.clone()).await?;
        if cancel.is_cancelled() {
            part.sync().map_err(Error::storage)?;
            events(Event::Checkpoint(job.resume.clone()));
            return Ok(Outcome::Paused);
        }

        // Finish: every byte present, flushed, optionally verified, then renamed into place.
        if job.resume.total.is_none() {
            let written = job.resume.plan.segments[0].done;
            job.resume.total = Some(written);
            job.resume.plan.segments[0].end = Some(written.saturating_sub(1));
        }
        let total = job.resume.total.unwrap();
        if total > 0 && !job.resume.plan.complete(Some(total)) {
            return Err(Error::network("The connection ended before the file was complete.").retryable());
        }
        part.sync().map_err(Error::storage)?;
        drop(part);
        let part_path = file::part_path(&job.destination);
        let sha256 = match &job.expected_sha256 {
            Some(expected) => {
                let path = part_path.clone();
                let actual = tokio::task::spawn_blocking(move || hash_file(&path)).await.map_err(|e| Error::protocol(e.to_string()))?.map_err(Error::storage)?;
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err(Error::new(ErrorKind::Integrity, "The file does not match the expected SHA-256 checksum."));
                }
                Some(actual)
            }
            None => None,
        };
        if job.destination.exists() {
            return Err(Error::new(ErrorKind::Storage, "A file with this name already exists; choose another name."));
        }
        std::fs::rename(&part_path, &job.destination).map_err(Error::storage)?;
        events(Event::Checkpoint(job.resume.clone()));
        Ok(Outcome::Completed { path: job.destination.clone(), bytes: total, sha256 })
    }

    async fn run_workers(&self, job: &mut Job, part: &file::PartFile, connections: usize, events: Events, cancel: CancellationToken) -> Result<()> {
        let plan = Arc::new(Mutex::new(std::mem::take(&mut job.resume.plan)));
        let ranged = job.resume.ranged;
        let workers = if ranged { connections } else { 1 };
        let running = Arc::new(AtomicUsize::new(workers));
        let written = Arc::new(AtomicU64::new(plan.lock().unwrap().written()));
        let stop = cancel.child_token();
        let shared_job = Arc::new(job.clone());
        let mut tasks = tokio::task::JoinSet::new();
        // Rings when a part ends, so connections waiting to help look again at once.
        let wake = Arc::new(tokio::sync::Notify::new());
        for _ in 0..workers {
            let (engine, plan, part, job, running, written, stop, wake) = (self.clone(), plan.clone(), part.clone(), shared_job.clone(), running.clone(), written.clone(), stop.clone(), wake.clone());
            tasks.spawn(async move { engine.worker(&job, &plan, &part, &running, &written, &stop, &wake).await });
        }
        // Progress and checkpoints while the workers run.
        let ticker = {
            let (plan, part, written, running, events, stop, resume) = (plan.clone(), part.clone(), written.clone(), running.clone(), events.clone(), stop.clone(), job.resume.clone());
            let every = self.options.checkpoint_every;
            tokio::spawn(async move {
                let (mut last, mut last_at, mut speed, mut checkpoint_at) = (written.load(Ordering::Relaxed), Instant::now(), 0.0f64, Instant::now());
                loop {
                    tokio::select! { _ = tokio::time::sleep(Duration::from_millis(250)) => {}, _ = stop.cancelled() => break }
                    let now_written = written.load(Ordering::Relaxed);
                    let elapsed = last_at.elapsed().as_secs_f64().max(0.001);
                    let instant = (now_written - last) as f64 / elapsed;
                    speed = if speed == 0.0 { instant } else { speed * 0.7 + instant * 0.3 };
                    (last, last_at) = (now_written, Instant::now());
                    events(Event::Progress { written: now_written, total: resume.total, bytes_per_second: speed, connections: running.load(Ordering::Relaxed) });
                    if checkpoint_at.elapsed() >= every {
                        checkpoint_at = Instant::now();
                        let part = part.clone();
                        if tokio::task::spawn_blocking(move || part.sync()).await.is_ok_and(|r| r.is_ok()) {
                            let mut snapshot = resume.clone();
                            snapshot.plan = plan.lock().unwrap().clone();
                            events(Event::Checkpoint(snapshot));
                        }
                    }
                }
            })
        };
        let mut failure: Option<Error> = None;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    if failure.is_none() {
                        failure = Some(error);
                        stop.cancel();
                    }
                }
                Err(panic) => {
                    failure.get_or_insert(Error::protocol(format!("A download worker stopped unexpectedly: {panic}")));
                    stop.cancel();
                }
            }
        }
        stop.cancel();
        let _ = ticker.await;
        let mut final_plan = std::mem::take(&mut *plan.lock().unwrap());
        final_plan.segments.iter_mut().for_each(|s| s.active = false);
        job.resume.plan = final_plan;
        events(Event::Progress { written: written.load(Ordering::Relaxed), total: job.resume.total, bytes_per_second: 0.0, connections: 0 });
        match failure {
            Some(error) if !cancel.is_cancelled() => Err(error),
            _ => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn worker(&self, job: &Job, plan: &Mutex<Plan>, part: &file::PartFile, running: &AtomicUsize, written: &AtomicU64, stop: &CancellationToken, wake: &tokio::sync::Notify) -> Result<()> {
        let max_segments = if self.host_limits.limit_for(&job.url).is_some() { job.connections.max(1) } else { self.options.max_segments };
        // Connections cut in a row on this worker without its part moving on.
        let mut cuts = 0u32;
        loop {
            if stop.is_cancelled() {
                return Ok(());
            }
            let claim = plan.lock().unwrap().claim(self.options.min_split, max_segments);
            let Some(claim) = claim else {
                // Nothing to take now. While another part is still coming, stay: when it turns out
                // slow, this connection takes over half of what it has left (no "stuck at 99 %").
                // Listening before looking: a part that ends in between still wakes this one.
                let notified = wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if !plan.lock().unwrap().segments.iter().any(|s| s.active && !s.finished()) {
                    return Ok(());
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    _ = &mut notified => continue,
                    _ = stop.cancelled() => return Ok(()),
                }
                let help = plan.lock().unwrap().help_slowest(ENDGAME_SPLIT, ENDGAME_SECONDS, max_segments);
                let Some(claim) = help else { continue };
                let result = self.transfer(job, plan, claim.index, part, written, stop).await;
                plan.lock().unwrap().release(claim.index);
                wake.notify_waiters();
                match result {
                    Ok(()) => {}
                    // The helper is a bonus: when its connection fails, the part waits for the next free connection.
                    Err(error) if error.kind == ErrorKind::Network && !stop.is_cancelled() => {}
                    Err(error) if error.is_refusal() && !stop.is_cancelled() => return Ok(()),
                    Err(error) => {
                        running.fetch_sub(1, Ordering::SeqCst);
                        return Err(error);
                    }
                }
                continue;
            };
            let before = plan.lock().unwrap().segments[claim.index].position();
            let result = self.transfer(job, plan, claim.index, part, written, stop).await;
            let moved = plan.lock().unwrap().segments[claim.index].position() > before;
            plan.lock().unwrap().release(claim.index);
            wake.notify_waiters();
            match result {
                Ok(()) => cuts = 0,
                // A dropped connection: this part continues at once on a new connection, from where it
                // stopped, while the others keep going. Only a connection that keeps failing without
                // receiving anything fails the attempt (which then waits and retries as a whole).
                Err(error) if error.kind == ErrorKind::Network && !stop.is_cancelled() && (moved || cuts < 3) => {
                    cuts = if moved { 1 } else { cuts + 1 };
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200 * (cuts as u64 - 1))) => {}
                        _ = stop.cancelled() => return Ok(()),
                    }
                }
                // A refusal while other connections still work: this one stops, its part stays for them,
                // and the site is remembered with the number that worked.
                Err(error) if error.is_refusal() && running.load(Ordering::SeqCst) > 1 && !stop.is_cancelled() => {
                    let remaining = running.fetch_sub(1, Ordering::SeqCst) - 1;
                    self.host_limits.learn(&job.url, remaining);
                    return Ok(());
                }
                Err(error) => {
                    running.fetch_sub(1, Ordering::SeqCst);
                    return Err(error);
                }
            }
        }
    }

    async fn transfer(&self, job: &Job, plan: &Mutex<Plan>, index: usize, part: &file::PartFile, written: &AtomicU64, stop: &CancellationToken) -> Result<()> {
        let (position, end) = {
            let plan = plan.lock().unwrap();
            let segment = &plan.segments[index];
            if segment.finished() { return Ok(()); }
            (segment.position(), segment.end)
        };
        let ranged = job.resume.ranged;
        let validator = job.resume.etag.clone().or_else(|| job.resume.last_modified.clone());
        let response = self.send(job, None, ranged.then_some((position, end)), validator.as_deref()).await?;
        Engine::check(&response)?;
        if ranged {
            let range = response.headers().get(header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).and_then(content_range);
            if response.status() != StatusCode::PARTIAL_CONTENT || range.map(|r| (r.0, r.2)) != Some((position, job.resume.total)) {
                return Err(Error::protocol("The server changed the file or refused a byte range. Resume to restart safely."));
            }
            if let (Some(expected), Some(current)) = (&job.resume.etag, response.headers().get(header::ETAG).and_then(|v| v.to_str().ok()))
                && expected != current {
                    return Err(Error::protocol("The file changed on the server during the download. Resume to restart safely."));
                }
        } else if response.status() != StatusCode::OK {
            return Err(Error::protocol("The server did not send the whole file."));
        }

        let mut stream = response.bytes_stream();
        let mut offset = position;
        // One buffer for the whole connection: handed to the writer and given back, never reallocated.
        let mut buffer: Vec<u8> = Vec::with_capacity(WRITE_BUFFER);
        let flush = |buffer: &mut Vec<u8>, at: u64| {
            let data = std::mem::take(buffer);
            let part = part.clone();
            async move {
                let len = data.len() as u64;
                let data = tokio::task::spawn_blocking(move || part.write_at(at, &data).map(|_| data))
                    .await
                    .map_err(|e| Error::protocol(e.to_string()))?
                    .map_err(Error::storage)?;
                Ok::<(u64, Vec<u8>), Error>((len, data))
            }
        };
        let mut buffer_start = offset;
        let (started, mut received, mut flushed_at, mut clipped) = (Instant::now(), 0u64, Instant::now(), false);
        let clip = |buffer: &mut Vec<u8>, at: u64| -> bool {
            let end = plan.lock().unwrap().segments[index].end;
            let room = end.map(|end| (end + 1).saturating_sub(at) as usize).unwrap_or(usize::MAX);
            let clipped = buffer.len() > room;
            buffer.truncate(room);
            clipped
        };
        loop {
            let next = tokio::select! {
                next = tokio::time::timeout(self.options.read_timeout, stream.next()) => next,
                _ = stop.cancelled() => break,
            };
            let chunk = match next {
                Err(_) => return Err(Error::network("The server stopped sending data.")),
                Ok(None) => break,
                Ok(Some(Err(error))) => return Err(Error::network(format!("The connection was interrupted: {error}"))),
                Ok(Some(Ok(chunk))) => chunk,
            };
            // The segment may have been shortened by a split meanwhile: never write past its end.
            let limit = plan.lock().unwrap().segments[index].end;
            let room = limit.map(|end| (end + 1).saturating_sub(offset)).unwrap_or(u64::MAX);
            let take = (chunk.len() as u64).min(room) as usize;
            self.global_limiter.take(take).await;
            job.limiter.take(take).await;
            buffer.extend_from_slice(&chunk[..take]);
            offset += take as u64;
            received += take as u64;
            // Full, at the part's end, or a second old: a slow connection still shows (and saves) its progress.
            if buffer.len() >= WRITE_BUFFER || take < chunk.len() || flushed_at.elapsed() >= Duration::from_secs(1) {
                // A helper may have taken the rest of this part since these bytes arrived.
                if clip(&mut buffer, buffer_start) {
                    offset = buffer_start + buffer.len() as u64;
                    clipped = true;
                }
                let (len, mut data) = flush(&mut buffer, buffer_start).await?;
                data.clear();
                buffer = data;
                {
                    let mut plan = plan.lock().unwrap();
                    let segment = &mut plan.segments[index];
                    segment.done += len;
                    segment.speed = (received as f64 / started.elapsed().as_secs_f64().max(0.001)) as u64;
                }
                written.fetch_add(len, Ordering::Relaxed);
                buffer_start = offset;
                flushed_at = Instant::now();
            }
            if clipped || take < chunk.len() || limit.is_some_and(|end| offset > end) {
                break;
            }
        }
        clip(&mut buffer, buffer_start);
        if !buffer.is_empty() {
            let (len, _) = flush(&mut buffer, buffer_start).await?;
            plan.lock().unwrap().segments[index].done += len;
            written.fetch_add(len, Ordering::Relaxed);
        }
        if stop.is_cancelled() {
            return Ok(());
        }
        let segment = plan.lock().unwrap().segments[index].clone();
        if segment.end.is_some() && !segment.finished() {
            return Err(Error::network("The connection ended before its part was complete."));
        }
        Ok(())
    }
}

fn content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, if total == "*" { None } else { Some(total.parse().ok()?) }))
}

/// The file name from a Content-Disposition header (RFC 6266, including `filename*`), made safe.
pub fn disposition_name(value: &str) -> Option<String> {
    let mut plain = None;
    for part in value.split(';').map(str::trim) {
        if let Some(encoded) = part.strip_prefix("filename*=") {
            let encoded = encoded.trim_matches('"');
            let raw = encoded.splitn(3, '\'').nth(2).unwrap_or(encoded);
            if let Ok(decoded) = percent_decode(raw) {
                return Some(sanitize_file_name(&decoded));
            }
        } else if let Some(name) = part.strip_prefix("filename=") {
            plain = Some(sanitize_file_name(name.trim_matches('"')));
        }
    }
    plain.filter(|n| !n.is_empty())
}

fn percent_decode(value: &str) -> std::result::Result<String, std::string::FromUtf8Error> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out)
}

/// A file name Windows accepts: no path, no reserved characters or device names, at most 150 characters.
pub fn sanitize_file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut cleaned: String = last.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    cleaned = cleaned.trim().trim_end_matches('.').to_string();
    if cleaned.is_empty() {
        return "download.bin".into();
    }
    if cleaned.chars().count() > 150 {
        let path = Path::new(&cleaned);
        let ext = path.extension().map(|e| e.to_string_lossy().to_string()).filter(|e| e.len() <= 20);
        let stem: String = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default().chars().take(140).collect();
        cleaned = match ext { Some(ext) => format!("{stem}.{ext}"), None => stem };
    }
    let stem = cleaned.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0') {
        cleaned.insert(0, '_');
    }
    cleaned
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe() {
        assert_eq!(sanitize_file_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_file_name("a:b?.zip"), "a_b_.zip");
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_file_name("  "), "download.bin");
        assert_eq!(disposition_name("attachment; filename=\"report.pdf\""), Some("report.pdf".into()));
        assert_eq!(disposition_name("attachment; filename=\"x.bin\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf"), Some("résumé.pdf".into()));
        assert_eq!(content_range("bytes 0-0/12345"), Some((0, 0, Some(12345))));
    }
}

/// The start of a response body, at most `max` bytes (for reading a download page).
async fn read_some(mut response: reqwest::Response, max: usize) -> Vec<u8> {
    let mut body = Vec::new();
    while body.len() < max {
        match tokio::time::timeout(Duration::from_secs(10), response.chunk()).await {
            Ok(Ok(Some(chunk))) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    body.truncate(max);
    body
}
