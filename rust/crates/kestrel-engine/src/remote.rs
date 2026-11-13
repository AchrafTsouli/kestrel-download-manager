//! FTP, FTPS and SFTP downloads: one connection, resumed from where the part file stops (FTP's REST,
//! SFTP's seek), written into the same `.kestrel.part` file and renamed into place like HTTP.
//!
//! Credentials come from the address (`ftp://user:pass@host/file`) or the session's Authorization
//! (a site login). FTPS: `ftps://` is explicit TLS (AUTH TLS) on port 21. SFTP checks the server's key
//! like OpenSSH's "accept-new": the first key seen for a host is remembered in `known_hosts`, and a
//! different one later is refused.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, ErrorKind, Result};
use crate::plan::Plan;
use crate::{Event, Job, Outcome, Probed, file};

pub fn is_remote(url: &url::Url) -> bool {
    matches!(url.scheme(), "ftp" | "ftps" | "sftp")
}

/// User and password: from the address, else a Basic Authorization (site login), else anonymous.
fn credentials(job: &Job) -> (String, String) {
    let url = &job.url;
    if !url.username().is_empty() {
        let decode = |s: &str| percent_decode(s);
        return (decode(url.username()), decode(url.password().unwrap_or("")));
    }
    if let Some(basic) = job.session.as_ref().and_then(|s| s.authorization.as_deref()).and_then(|a| a.strip_prefix("Basic "))
        && let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(basic.trim())
        && let Ok(text) = String::from_utf8(raw)
        && let Some((user, password)) = text.split_once(':') {
            return (user.to_string(), password.to_string());
        }
    if url.scheme() == "sftp" { (String::new(), String::new()) } else { ("anonymous".into(), "kestrel@".into()) }
}

fn percent_decode(text: &str) -> String {
    percent_encoding::percent_decode_str(text).decode_utf8_lossy().into_owned()
}

fn remote_path(url: &url::Url) -> String {
    let path = percent_decode(url.path());
    if path.is_empty() { "/".into() } else { path }
}

fn network(error: impl std::fmt::Display) -> Error {
    Error::network(format!("The server connection failed: {error}"))
}

const TIMEOUT: Duration = Duration::from_secs(30);

// ------------------------------------------------------------------ FTP / FTPS

type Ftp = suppaftp::tokio::AsyncRustlsFtpStream;

async fn ftp_connect(job: &Job) -> Result<Ftp> {
    let url = &job.url;
    let host = url.host_str().ok_or_else(|| Error::protocol("The address has no server name."))?.to_string();
    let port = url.port().unwrap_or(21);
    let connect = Ftp::connect((host.as_str(), port));
    let mut ftp = tokio::time::timeout(TIMEOUT, connect).await.map_err(|_| Error::network("The FTP server did not answer."))?.map_err(network)?;
    if url.scheme() == "ftps" {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::protocol(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = suppaftp::tokio::AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(Arc::new(config)));
        ftp = ftp.into_secure(connector, &host).await.map_err(|e| Error::new(ErrorKind::Protocol, format!("The server's secure connection failed: {e}")))?;
    }
    let (user, password) = credentials(job);
    ftp.login(user, password).await.map_err(|e| Error::new(ErrorKind::Authentication, format!("The FTP server refused the sign-in: {e}")))?;
    ftp.transfer_type(suppaftp::types::FileType::Binary).await.map_err(network)?;
    Ok(ftp)
}

fn ftp_error(error: suppaftp::FtpError) -> Error {
    let text = error.to_string();
    if text.contains("550") {
        Error::new(ErrorKind::NotFound, "The file was not found on the server (FTP 550).")
    } else if text.contains("530") {
        Error::new(ErrorKind::Authentication, "The FTP server refused the sign-in (530).")
    } else {
        network(text)
    }
}

// ------------------------------------------------------------------ SFTP

struct HostCheck {
    host: String,
    known_hosts: Option<PathBuf>,
}

impl russh::client::Handler for HostCheck {
    type Error = Error;

    async fn check_server_key(&mut self, key: &russh::keys::PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        let fingerprint = key.public_key().fingerprint(russh::keys::HashAlg::Sha256).to_string();
        let Some(path) = &self.known_hosts else { return Ok(true) };
        let known = std::fs::read_to_string(path).unwrap_or_default();
        match known.lines().find_map(|l| l.split_once(' ').filter(|(h, _)| *h == self.host).map(|(_, f)| f.trim().to_string())) {
            Some(saved) if saved == fingerprint => Ok(true),
            Some(_) => Err(Error::new(ErrorKind::Protocol, format!("The server {} presents a different key than before. Someone may be intercepting the connection; if the server really changed, remove its line from {}.", self.host, path.display()))),
            None => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(path, format!("{known}{} {fingerprint}\n", self.host));
                Ok(true)
            }
        }
    }
}

impl From<russh::Error> for Error {
    fn from(error: russh::Error) -> Self {
        network(error)
    }
}

struct Sftp {
    session: russh_sftp::client::SftpSession,
    _handle: russh::client::Handle<HostCheck>,
}

async fn sftp_connect(job: &Job, known_hosts: Option<PathBuf>) -> Result<Sftp> {
    let url = &job.url;
    let host = url.host_str().ok_or_else(|| Error::protocol("The address has no server name."))?.to_string();
    let port = url.port().unwrap_or(22);
    let config = Arc::new(russh::client::Config { inactivity_timeout: Some(Duration::from_secs(60)), ..Default::default() });
    let check = HostCheck { host: format!("{host}:{port}"), known_hosts };
    let connect = russh::client::connect(config, (host.as_str(), port), check);
    let mut handle = tokio::time::timeout(TIMEOUT, connect).await.map_err(|_| Error::network("The SFTP server did not answer."))??;
    let (user, password) = credentials(job);
    if user.is_empty() {
        return Err(Error::new(ErrorKind::Authentication, "SFTP needs a user name and password: write them in the address (sftp://user:password@server/file) or in Options > Sites Logins."));
    }
    let result = handle.authenticate_password(user, password).await?;
    if !matches!(result, russh::client::AuthResult::Success) {
        return Err(Error::new(ErrorKind::Authentication, "The SFTP server refused the user name or password."));
    }
    let channel = handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let session = russh_sftp::client::SftpSession::new(channel.into_stream()).await.map_err(network)?;
    Ok(Sftp { session, _handle: handle })
}

fn sftp_error(error: russh_sftp::client::error::Error) -> Error {
    let text = error.to_string();
    if text.to_ascii_lowercase().contains("no such file") {
        Error::new(ErrorKind::NotFound, "The file was not found on the server.")
    } else if text.to_ascii_lowercase().contains("permission") {
        Error::new(ErrorKind::AccessDenied, "The server does not allow reading this file.")
    } else {
        network(text)
    }
}

// ------------------------------------------------------------------ shared

/// Size of the remote file (when the server says).
pub async fn probe(job: &mut Job, known_hosts: Option<PathBuf>) -> Result<Probed> {
    let path = remote_path(&job.url);
    let total = match job.url.scheme() {
        "sftp" => {
            let sftp = sftp_connect(job, known_hosts).await?;
            sftp.session.metadata(path.clone()).await.map_err(sftp_error)?.size
        }
        _ => {
            let mut ftp = ftp_connect(job).await?;
            let size = ftp.size(&path).await.ok().map(|s| s as u64);
            let _ = ftp.quit().await;
            size
        }
    };
    let file_name = path.rsplit('/').next().filter(|n| !n.is_empty()).map(str::to_string);
    // The previous position is only valid for a file of the same size.
    if job.resume.total.is_some() && job.resume.total != total {
        job.resume.plan = Plan::default();
    }
    job.resume.total = total;
    job.resume.ranged = true;
    job.resume.etag = None;
    job.resume.final_url = Some(job.url.to_string());
    Ok(Probed { total, ranged: true, file_name, content_type: None, final_url: job.url.to_string() })
}

/// Downloads into the part file from where it stops.
pub async fn download(job: &mut Job, events: &(dyn Fn(Event) + Send + Sync), cancel: CancellationToken, known_hosts: Option<PathBuf>, global: &crate::limits::Limiter) -> Result<Outcome> {
    let probed = probe(job, known_hosts.clone()).await?;
    events(Event::Probed(probed.clone()));
    let total = probed.total;
    if job.resume.plan.segments.len() != 1 {
        job.resume.plan = Plan::new(total, false, 1, 1 << 20);
    }
    let offset = job.resume.plan.segments[0].done;
    let part_path = file::part_path(&job.destination);
    let part = file::PartFile::open(&part_path, total, offset == 0)?;
    let path = remote_path(&job.url);
    // The connections stay open while the file is read.
    let mut _sftp_keep: Option<Sftp> = None;
    let mut _ftp_keep: Option<Ftp> = None;
    let mut reader: Box<dyn AsyncRead + Unpin + Send> = match job.url.scheme() {
        "sftp" => {
            let sftp = sftp_connect(job, known_hosts).await?;
            let mut file = sftp.session.open(path.clone()).await.map_err(sftp_error)?;
            _sftp_keep = Some(sftp);
            if offset > 0 {
                file.seek(std::io::SeekFrom::Start(offset)).await.map_err(network)?;
            }
            Box::new(file)
        }
        _ => {
            let mut ftp = ftp_connect(job).await?;
            if offset > 0 {
                ftp.resume_transfer(offset as usize).await.map_err(ftp_error)?;
            }
            let stream = ftp.retr_as_stream(&path).await.map_err(ftp_error)?;
            _ftp_keep = Some(ftp);
            Box::new(stream)
        }
    };
    let mut buffer = vec![0u8; 256 << 10];
    let mut at = offset;
    let (mut window_start, mut window_bytes) = (Instant::now(), 0u64);
    let mut speed = 0.0;
    let mut last_event = Instant::now();
    let mut last_checkpoint = Instant::now();
    loop {
        let n = tokio::select! {
            r = tokio::time::timeout(TIMEOUT, reader.read(&mut buffer)) => r.map_err(|_| Error::network("The server stopped sending."))?.map_err(network)?,
            _ = cancel.cancelled() => {
                part.sync().map_err(Error::storage)?;
                events(Event::Checkpoint(job.resume.clone()));
                return Ok(Outcome::Paused);
            }
        };
        if n == 0 {
            break;
        }
        global.take(n).await;
        job.limiter.take(n).await;
        part.write_at(at, &buffer[..n]).map_err(Error::storage)?;
        at += n as u64;
        job.resume.plan.segments[0].done = at;
        window_bytes += n as u64;
        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            speed = window_bytes as f64 / elapsed.as_secs_f64();
            (window_start, window_bytes) = (Instant::now(), 0);
        }
        if last_event.elapsed() >= Duration::from_millis(250) {
            events(Event::Progress { written: at, total, bytes_per_second: speed, connections: 1 });
            last_event = Instant::now();
        }
        if last_checkpoint.elapsed() >= Duration::from_secs(2) {
            part.sync().map_err(Error::storage)?;
            events(Event::Checkpoint(job.resume.clone()));
            last_checkpoint = Instant::now();
        }
    }
    drop(reader);
    if let Some(expected) = total
        && at < expected {
            return Err(Error::network("The connection ended before the file was complete.").retryable());
        }
    let total = total.unwrap_or(at);
    job.resume.total = Some(total);
    job.resume.plan.segments[0].end = Some(total.saturating_sub(1));
    part.sync().map_err(Error::storage)?;
    drop(part);
    crate::finish_file(job, events, total).await
}

/// Where SFTP remembers server keys: beside the app's data, or nowhere (tests).
pub fn known_hosts_in(dir: &Path) -> PathBuf {
    dir.join("known_hosts")
}
