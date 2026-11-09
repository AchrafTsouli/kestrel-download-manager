//! A local HTTP/1.1 server for tests and benchmarks. Every file's bytes are computed, so nothing is
//! stored and any size can be served. Faults are chosen per request with query parameters:
//!
//! | parameter      | effect |
//! |----------------|--------|
//! | `size=N`       | file size in bytes (default 1 MiB) |
//! | `seed=N`       | changes the content (and the ETag) |
//! | `max_conn=N`   | more than N transfers of this path at once get `429 Too Many Requests` |
//! | `need_nav=1`   | `403` unless the request looks like a browser navigation (`Sec-Fetch-Mode: navigate`) |
//! | `no_range=1`   | ignores `Range` and always sends the whole file with `200` |
//! | `fail=N`       | the first N requests of this path get `503` |
//! | `drop_at=N`    | the first transfer that passes byte N of the file is cut there (once per path) |
//! | `slow_us=N`    | waits N microseconds per 16 KiB chunk |
//! | `html=1`       | answers with a web page instead of the file |
//!
//! [`LabServer::stats`] reports what each path saw, so tests can check how many connections the
//! client really opened.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::Frame;
use hyper::header::{self, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

/// The byte at `offset` of the file with `seed`: cheap, deterministic and not periodic over small spans.
pub fn byte_at(seed: u64, offset: u64) -> u8 {
    let mut x = offset.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed.wrapping_mul(0xD1B5_4A32_D192_ED03);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (x >> 32) as u8
}

/// The whole file, for comparing with what a client saved.
pub fn expected(size: u64, seed: u64) -> Vec<u8> {
    (0..size).map(|i| byte_at(seed, i)).collect()
}

#[derive(Debug, Default, Clone)]
pub struct PathStats {
    pub requests: u64,
    pub refused: u64,
    pub max_concurrent: usize,
}

#[derive(Default)]
struct PathState {
    active: AtomicUsize,
    requests: AtomicU64,
    refused: AtomicU64,
    max_concurrent: AtomicUsize,
    dropped: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct Shared {
    paths: Mutex<HashMap<String, Arc<PathState>>>,
}

impl Shared {
    fn state(&self, key: &str) -> Arc<PathState> {
        self.paths.lock().unwrap().entry(key.to_string()).or_default().clone()
    }
}

/// A running server; stops when dropped.
pub struct LabServer {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LabServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl LabServer {
    /// Starts on 127.0.0.1 with a free port.
    pub async fn start() -> std::io::Result<LabServer> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared::default());
        let serving = shared.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { continue };
                let shared = serving.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| handle(shared.clone(), request));
                    let _ = http1::Builder::new().keep_alive(true).serve_connection(TokioIo::new(socket), service).await;
                });
            }
        });
        Ok(LabServer { addr, shared, task })
    }

    /// The address of a file, with fault parameters such as `"size=5000000&max_conn=2"`.
    pub fn url(&self, name: &str, query: &str) -> String {
        if query.is_empty() { format!("http://{}/{}", self.addr, name) } else { format!("http://{}/{}?{}", self.addr, name, query) }
    }

    pub fn stats(&self, name: &str) -> PathStats {
        let state = self.shared.state(&format!("/{name}"));
        PathStats {
            requests: state.requests.load(Ordering::SeqCst),
            refused: state.refused.load(Ordering::SeqCst),
            max_concurrent: state.max_concurrent.load(Ordering::SeqCst),
        }
    }
}

type Body = BoxBody<Bytes, Infallible>;

fn text(status: StatusCode, body: &'static str) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::from_static(body.as_bytes())).boxed());
    *response.status_mut() = status;
    response
}

fn parse_range(value: &HeaderValue, size: u64) -> Option<(u64, u64)> {
    let spec = value.to_str().ok()?.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        return (suffix > 0).then(|| (size.saturating_sub(suffix), size - 1));
    }
    let start: u64 = start.parse().ok()?;
    let end = if end.is_empty() { size - 1 } else { end.parse::<u64>().ok()?.min(size - 1) };
    (start <= end && start < size).then_some((start, end))
}

/// Decrements the active-transfer count when the body finishes or is dropped.
struct ActiveGuard(Arc<PathState>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn handle(shared: Arc<Shared>, request: Request<hyper::body::Incoming>) -> Result<Response<Body>, Infallible> {
    let path = request.uri().path().to_string();
    let query: HashMap<String, String> = request
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| pair.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    let number = |key: &str, default: u64| query.get(key).and_then(|v| v.parse().ok()).unwrap_or(default);
    let state = shared.state(&path);
    let request_number = state.requests.fetch_add(1, Ordering::SeqCst) + 1;

    if request.method() != Method::GET && request.method() != Method::HEAD {
        return Ok(text(StatusCode::METHOD_NOT_ALLOWED, "method"));
    }
    if request_number <= number("fail", 0) {
        return Ok(text(StatusCode::SERVICE_UNAVAILABLE, "try again"));
    }
    if number("need_nav", 0) == 1 && request.headers().get("sec-fetch-mode").and_then(|v| v.to_str().ok()) != Some("navigate") {
        state.refused.fetch_add(1, Ordering::SeqCst);
        return Ok(text(StatusCode::FORBIDDEN, "browsers only"));
    }
    // A Cloudflare-style bot check: a browser identity from a non-browser client is challenged.
    if number("bot_check", 0) == 1 && request.headers().get("user-agent").and_then(|v| v.to_str().ok()).is_some_and(|ua| ua.contains("Mozilla")) {
        state.refused.fetch_add(1, Ordering::SeqCst);
        let mut challenge = text(StatusCode::FORBIDDEN, "Just a moment...");
        challenge.headers_mut().insert("cf-mitigated", HeaderValue::from_static("challenge"));
        challenge.headers_mut().insert(header::SERVER, HeaderValue::from_static("cloudflare"));
        return Ok(challenge);
    }
    if number("html", 0) == 1 {
        let mut page = text(StatusCode::OK, "<html><body>sign in</body></html>");
        page.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
        return Ok(page);
    }

    let size = number("size", 1 << 20);
    let seed = number("seed", 1);
    let ranged = number("no_range", 0) == 0;
    let range = if ranged { request.headers().get(header::RANGE).and_then(|v| parse_range(v, size)) } else { None };
    let (start, end) = range.unwrap_or((0, size.saturating_sub(1)));
    let probe = range.is_some_and(|(s, e)| s == 0 && e == 0);

    // Concurrency limit applies to real transfers, not to the one-byte probe.
    let guard = if probe {
        None
    } else {
        let now = state.active.fetch_add(1, Ordering::SeqCst) + 1;
        state.max_concurrent.fetch_max(now, Ordering::SeqCst);
        let limit = number("max_conn", 0) as usize;
        if limit > 0 && now > limit {
            state.active.fetch_sub(1, Ordering::SeqCst);
            state.refused.fetch_add(1, Ordering::SeqCst);
            return Ok(text(StatusCode::TOO_MANY_REQUESTS, "too many connections"));
        }
        Some(ActiveGuard(state.clone()))
    };

    let length = if size == 0 { 0 } else { end - start + 1 };
    let drop_at = query.get("drop_at").and_then(|v| v.parse::<u64>().ok());
    let cut = drop_at.filter(|at| *at > start && *at <= end && !state.dropped.swap(true, Ordering::SeqCst));
    let slow = Duration::from_micros(number("slow_us", 0));
    let send_until = cut.unwrap_or(end + 1);

    let mut builder = Response::builder()
        .status(if range.is_some() { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_LENGTH, length)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ETAG, format!("\"lab-{seed}-{size}\""))
        .header(header::LAST_MODIFIED, "Wed, 21 Oct 2026 07:28:00 GMT");
    if ranged {
        builder = builder.header(header::ACCEPT_RANGES, "bytes");
    }
    if range.is_some() {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    if request.method() == Method::HEAD || size == 0 {
        drop(guard);
        return Ok(builder.body(Full::new(Bytes::new()).boxed()).unwrap());
    }

    const CHUNK: u64 = 16 * 1024;
    let chunks = stream::unfold((start, guard), move |(position, guard)| async move {
        if position >= send_until {
            drop(guard);
            return None;
        }
        if !slow.is_zero() {
            tokio::time::sleep(slow).await;
        }
        let next = (position + CHUNK).min(send_until);
        let data: Vec<u8> = (position..next).map(|i| byte_at(seed, i)).collect();
        Some((Ok::<_, Infallible>(Frame::data(Bytes::from(data))), (next, guard)))
    });
    // A cut transfer ends early: the client sees fewer bytes than Content-Length (a broken connection).
    Ok(builder.body(BodyExt::boxed(StreamBody::new(chunks))).unwrap())
}
