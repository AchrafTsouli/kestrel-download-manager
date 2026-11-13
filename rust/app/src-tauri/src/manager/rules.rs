//! Site logins, per-site rules, the virus scanner, server dates and the VPN.

use super::*;

impl Manager {

    /// The first enabled rule that matches the address and file name.
    pub fn rule_for(&self, url: &Url, file_name: &str) -> Option<crate::model::Rule> {
        self.settings.read().rules.iter().find(|r| r.matches(url, file_name)).cloned()
    }

    /// Adds a saved site login to the request, unless the browser sent credentials of its own.
    pub fn with_login(&self, url: &Url, session: Option<Session>) -> Option<Session> {
        use base64::Engine as _;
        if session.as_ref().is_some_and(|s| s.authorization.is_some()) {
            return session;
        }
        let settings = self.settings.read();
        let Some(login) = settings.site_logins.iter().find(|l| l.matches(url)) else { return session };
        let mut session = session.unwrap_or_else(|| Session::for_url(url));
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", login.username, login.password));
        session.authorization = Some(format!("Basic {token}"));
        Some(session)
    }

    /// Runs the chosen virus scanner on a finished file (or torrent folder). A non-zero exit code
    /// blocks it: the file is renamed with ".blocked" so it is not opened by accident.
    pub(super) async fn scan(&self, path: &Path) -> Result<(), String> {
        let (scanner, args) = {
            let s = self.settings.read();
            (s.virus_scanner.trim().to_string(), s.virus_scanner_args.clone())
        };
        if scanner.is_empty() {
            return Ok(());
        }
        let file = path.to_string_lossy().into_owned();
        let arguments: Vec<String> = split_arguments(&args)?.into_iter().map(|a| a.replace("{file}", &file)).collect();
        let mut command = tokio::process::Command::new(&scanner);
        command.args(&arguments).kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let status = tokio::time::timeout(Duration::from_secs(30 * 60), command.status()).await
            .map_err(|_| "The virus scanner did not finish within 30 minutes.".to_string())?
            .map_err(|e| format!("The virus scanner could not start: {e}"))?;
        if status.success() {
            return Ok(());
        }
        let mut blocked = path.as_os_str().to_owned();
        blocked.push(".blocked");
        let _ = std::fs::rename(path, &blocked);
        Err(format!("The virus scanner reported a problem (exit code {}). The file was kept as {}.", status.code().unwrap_or(-1), Path::new(&blocked).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
    }

    /// Gives the finished file the server's date (Options > Save to), when the server sent one.
    pub(super) fn server_date(&self, path: &Path, last_modified: Option<&str>) {
        if !self.settings.read().set_creation_date {
            return;
        }
        let Some(date) = last_modified.and_then(|d| chrono::DateTime::parse_from_rfc2822(d.trim()).ok()) else { return };
        let time = std::time::UNIX_EPOCH + Duration::from_secs(date.timestamp().max(0) as u64);
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = file.set_times(std::fs::FileTimes::new().set_modified(time).set_accessed(time));
        }
    }

    /// Unpacks a finished archive when enabled in Options. Its result is told in the main window;
    /// a failure leaves the archive as it is and does not fail the download.
    pub(super) async fn unpack(&self, id: &str, path: &Path) {
        let (wanted, delete, target) = {
            let s = self.settings.read();
            (s.extract_archives, s.delete_extracted_archives, s.archive_target)
        };
        if !wanted || !crate::extract::is_archive(path) {
            return;
        }
        if let Some(live) = self.live.lock().get_mut(id) {
            live.info.phase = Some("Unpacking the archive".into());
            live.info.phase_fraction = None;
        }
        self.update(id, |r| r.status = Status::Finishing);
        self.emit(id);
        let archive = path.to_path_buf();
        let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let message = match tokio::task::spawn_blocking(move || crate::extract::extract(&archive, target)).await {
            Ok(Ok((folder, what))) => {
                if delete {
                    crate::extract::recycle(path);
                }
                format!("{name}: {what} unpacked into {}.", folder.display())
            }
            Ok(Err(error)) => format!("{name} was not unpacked: {error}"),
            Err(error) => format!("{name} was not unpacked: {error}"),
        };
        let _ = self.app.emit("message", message);
    }

    /// Dials the chosen Windows VPN before a download starts (once; it stays up).
    pub(super) async fn ensure_vpn(&self) -> Result<(), String> {
        let (enabled, profile) = {
            let s = self.settings.read();
            (s.connect_vpn, s.vpn_profile.trim().to_string())
        };
        if !enabled || profile.is_empty() {
            return Ok(());
        }
        let mut up = self.vpn_up.lock().await;
        if *up {
            return Ok(());
        }
        rasdial(&profile, false).await?;
        *up = true;
        Ok(())
    }

    pub(super) fn disconnect_vpn_if_idle(self: &Arc<Self>) {
        let (wanted, profile) = {
            let s = self.settings.read();
            (s.disconnect_vpn_when_idle, s.vpn_profile.trim().to_string())
        };
        if !wanted || profile.is_empty() || !self.live.lock().is_empty() {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut up = manager.vpn_up.lock().await;
            if *up && manager.live.lock().is_empty() && rasdial(&profile, true).await.is_ok() {
                *up = false;
            }
        });
    }
}

/// Windows' own VPN dialler, with the profile's saved credentials.
#[cfg(not(windows))]
pub(super) async fn rasdial(_profile: &str, _disconnect: bool) -> Result<(), String> {
    Err("Connecting a VPN from Kestrel is available on Windows only; connect it from your system's network settings.".into())
}
#[cfg(windows)]
pub(super) async fn rasdial(profile: &str, disconnect: bool) -> Result<(), String> {
    let mut command = tokio::process::Command::new("rasdial.exe");
    command.arg(profile).kill_on_drop(true);
    if disconnect {
        command.arg("/disconnect");
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let status = tokio::time::timeout(Duration::from_secs(90), command.status()).await
        .map_err(|_| "The VPN did not connect within 90 seconds.".to_string())?
        .map_err(|e| format!("Windows VPN could not start: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("Windows VPN returned error {}. Check the profile \"{profile}\" and its saved sign-in.", status.code().unwrap_or(-1))) }
}
