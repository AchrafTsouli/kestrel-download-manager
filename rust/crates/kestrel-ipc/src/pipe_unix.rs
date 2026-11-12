//! Linux and macOS: the link between `kestrel-nmhost` and Kestrel is a Unix socket in the user's own
//! runtime folder, readable only by the user; each side checks that the other runs as the same
//! user, so another account cannot stand in for Kestrel.

use std::path::PathBuf;
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};

fn uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

/// `$XDG_RUNTIME_DIR/kestrel-bridge.sock` (Linux: private to the user), otherwise
/// `$TMPDIR/kestrel-bridge-<uid>.sock` (macOS: TMPDIR is per user).
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_dir()) {
        Some(dir) => dir.join("kestrel-bridge.sock"),
        None => std::env::temp_dir().join(format!("kestrel-bridge-{}.sock", uid())),
    }
}

fn same_user(stream: &UnixStream) -> std::io::Result<()> {
    match stream.peer_cred() {
        Ok(cred) if cred.uid() == uid() => Ok(()),
        Ok(_) => Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "The Kestrel socket is used by another user's program.")),
        Err(e) => Err(e),
    }
}

pub struct Listener(UnixListener);

/// Starts listening; fails when another Kestrel of this user already does.
pub fn listen() -> std::io::Result<Listener> {
    listen_at(&socket_path())
}

fn listen_at(path: &std::path::Path) -> std::io::Result<Listener> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        // A socket nobody answers on is left from a crash.
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "Kestrel is already running."));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(Listener(listener))
}

impl Listener {
    /// The next connection from a program of the same user (others are dropped).
    pub async fn accept(&self) -> std::io::Result<UnixStream> {
        loop {
            let (stream, _) = self.0.accept().await?;
            if same_user(&stream).is_ok() {
                return Ok(stream);
            }
        }
    }
}

/// Connects to the running Kestrel, refusing a socket served by another user's program.
pub async fn connect(timeout: Duration) -> std::io::Result<UnixStream> {
    connect_at(&socket_path(), timeout).await
}

async fn connect_at(path: &std::path::Path, timeout: Duration) -> std::io::Result<UnixStream> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match UnixStream::connect(path).await {
            Ok(stream) => {
                same_user(&stream)?;
                return Ok(stream);
            }
            Err(e) if tokio::time::Instant::now() < deadline && matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Request, Response, read_frame, write_frame};

    #[tokio::test]
    async fn a_request_crosses_the_socket_and_a_second_listener_is_refused() {
        let path = std::env::temp_dir().join(format!("kestrel-bridge-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = listen_at(&path).unwrap();
        assert!(listen_at(&path).is_err(), "a second Kestrel must not take over");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let serve = tokio::spawn(async move {
            // The check above connected once and hung up; like the app, skip connections that carry nothing.
            loop {
                let mut stream = listener.accept().await.unwrap();
                if let Ok(Some(request)) = read_frame::<Request>(&mut stream).await {
                    write_frame(&mut stream, &Response { ok: request.action == "ping", ..Default::default() }).await.unwrap();
                    break;
                }
            }
        });
        let mut client = connect_at(&path, Duration::from_secs(2)).await.unwrap();
        write_frame(&mut client, &Request { action: "ping".into(), ..Default::default() }).await.unwrap();
        assert!(read_frame::<Response>(&mut client).await.unwrap().unwrap().ok);
        serve.await.unwrap();
        let _ = std::fs::remove_file(&path);
    }
}
