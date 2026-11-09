use std::time::Duration;

/// Why a download stopped. `kind` drives the UI state and whether a retry makes sense.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ErrorKind {
    Network,
    Server,
    NotFound,
    Expired,
    AccessDenied,
    RateLimited,
    Authentication,
    Protocol,
    DiskSpace,
    Storage,
    Integrity,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub retryable: bool,
    /// The server's Retry-After, when it sent one.
    pub retry_after: Option<Duration>,
    pub status: Option<u16>,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Error { kind, message: message.into(), retryable: false, retry_after: None, status: None }
    }
    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
    pub fn network(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Network, message).retryable()
    }
    pub fn protocol(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Protocol, message)
    }
    pub fn storage(error: std::io::Error) -> Self {
        // ERROR_DISK_FULL (112) / ERROR_HANDLE_DISK_FULL (39) on Windows, ENOSPC (28) elsewhere.
        let full = matches!(error.raw_os_error(), Some(112) | Some(39)) || error.kind() == std::io::ErrorKind::StorageFull;
        Error::new(if full { ErrorKind::DiskSpace } else { ErrorKind::Storage }, error.to_string())
    }
    /// The server refused this request (as opposed to failing while sending data).
    pub fn is_refusal(&self) -> bool {
        matches!(self.kind, ErrorKind::AccessDenied | ErrorKind::RateLimited) || self.status == Some(503)
    }

    pub fn from_status(status: u16, retry_after: Option<Duration>) -> Self {
        let (kind, retryable, message) = match status {
            401 => (ErrorKind::Authentication, false, "The server asks for a user name and password (HTTP 401).".to_string()),
            403 => (ErrorKind::AccessDenied, false, "The server refused this request (HTTP 403). The link may have expired, or the site may require its browser session: capture the download again from the browser.".to_string()),
            404 => (ErrorKind::NotFound, false, "The file was not found on the server (HTTP 404).".to_string()),
            410 => (ErrorKind::Expired, false, "The link has expired (HTTP 410).".to_string()),
            429 => (ErrorKind::RateLimited, true, "The server asks to slow down (HTTP 429).".to_string()),
            408 | 500 | 502 | 503 | 504 => (ErrorKind::Server, true, format!("The server had a temporary problem (HTTP {status}).")),
            _ => (ErrorKind::Server, false, format!("The server answered HTTP {status}.")),
        };
        Error { kind, message, retryable, retry_after, status: Some(status) }
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        if let Some(status) = error.status() {
            return Error::from_status(status.as_u16(), None);
        }
        let detail = std::error::Error::source(&error).map(|s| s.to_string()).unwrap_or_else(|| error.to_string());
        if error.is_redirect() {
            return Error::protocol(format!("Redirect rejected: {detail}"));
        }
        Error::network(format!("The connection failed: {detail}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
