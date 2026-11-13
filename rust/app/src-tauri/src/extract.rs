//! Unpacking finished archives (when enabled in Options): zip inside Kestrel, 7z/rar/tar through an
//! installed 7-Zip. Everything goes into a new folder named after the archive, never over existing
//! files, and no entry can escape that folder.

use std::path::{Path, PathBuf};

/// The archive kinds Kestrel can unpack, by file name.
pub fn is_archive(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    [".zip", ".7z", ".rar", ".tar", ".tar.gz", ".tgz", ".tar.xz", ".tar.bz2"].iter().any(|e| name.ends_with(e))
}

/// "Course.zip" → "Course", "data.tar.gz" → "data", in the same folder, made unique.
fn target_folder(archive: &Path) -> PathBuf {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    let stem_len = [".tar.gz", ".tar.xz", ".tar.bz2", ".tgz", ".tar", ".zip", ".7z", ".rar"].iter().find(|e| lower.ends_with(*e)).map_or(name.len(), |e| name.len() - e.len());
    let stem = &name[..stem_len];
    let parent = archive.parent().unwrap_or(Path::new(""));
    let first = parent.join(stem);
    if !first.exists() {
        return first;
    }
    (1..1000).map(|n| parent.join(format!("{stem} ({n})"))).find(|p| !p.exists()).unwrap_or(first)
}

/// What `unpack` reports when an archive is protected and the password tried does not open it.
const LOCKED: &str = "The archive is protected by a password; add it in Options > Archives.";

fn unzip(archive: &Path, into: &Path, password: Option<&str>) -> Result<usize, String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("The archive is not a readable zip: {e}"))?;
    let mut count = 0;
    for i in 0..zip.len() {
        let encrypted = zip.by_index_raw(i).map_err(|e| e.to_string())?.encrypted();
        let mut entry = match (encrypted, password) {
            (false, _) => zip.by_index(i).map_err(|e| e.to_string())?,
            (true, None) => return Err(LOCKED.into()),
            (true, Some(p)) => zip.by_index_decrypt(i, p.as_bytes()).map_err(|_| LOCKED.to_string())?,
        };
        // enclosed_name refuses "..", absolute paths and drive letters: nothing lands outside `into`.
        let Some(relative) = entry.enclosed_name() else { return Err(format!("The archive holds an unsafe path: {}", entry.name())) };
        let target = into.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        // A wrong ZipCrypto password can pass the header check and fail only on the data.
        if let Err(e) = std::io::copy(&mut entry, &mut out) {
            drop(out);
            let _ = std::fs::remove_file(&target);
            return Err(if encrypted { LOCKED.to_string() } else { e.to_string() });
        }
        count += 1;
    }
    Ok(count)
}

/// 7-Zip's command-line program, if installed.
fn seven_zip() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(var) {
            candidates.push(PathBuf::from(dir).join("7-Zip").join("7z.exe"));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["7z", "7zz", "7za"] {
                candidates.push(dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)));
            }
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Unpacks `archive` into a new folder beside it. Returns the folder and what it holds.
/// Unpacks `archive`. `target`: 0 = into a folder named after it when it holds several things
/// (a single file or folder lands beside the archive), 1 = always into such a folder, 2 = straight
/// into the archive's folder (never over existing files).
/// Unpacks `archive`; a protected one is tried with each of `passwords` in turn.
pub fn extract(archive: &Path, target: u8, passwords: &[String]) -> Result<(PathBuf, String), String> {
    let beside = archive.parent().unwrap_or(Path::new("")).to_path_buf();
    let (into, made) = if target == 2 { (beside.clone(), false) } else { (target_folder(archive), true) };
    let mut result = unpack(archive, &into, None);
    for password in passwords.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        if !matches!(&result, Err(e) if e == LOCKED) {
            break;
        }
        if made {
            let _ = std::fs::remove_dir_all(&into);
        }
        result = unpack(archive, &into, Some(password));
    }
    match result {
        Ok(what) if target == 0 => Ok((single_up(&into, &beside).unwrap_or(into), what)),
        Ok(what) => Ok((into, what)),
        Err(error) => {
            // Leave nothing half-unpacked behind (only a folder Kestrel made).
            if made {
                let _ = std::fs::remove_dir_all(&into);
            }
            Err(error)
        }
    }
}

/// When the new folder holds one thing only, that thing moves up beside the archive.
fn single_up(folder: &Path, beside: &Path) -> Option<PathBuf> {
    let mut entries = std::fs::read_dir(folder).ok()?.flatten();
    let only = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    let target = beside.join(only.file_name());
    if target.exists() {
        return None;
    }
    std::fs::rename(only.path(), &target).ok()?;
    let _ = std::fs::remove_dir(folder);
    Some(target)
}

/// Moves a file to the Recycle Bin (Windows), so an unpacked archive can still be brought back.
pub fn recycle(path: &Path) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW};
        // A double-NUL-terminated list of one path.
        let from: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain([0, 0]).collect();
        let mut op: SHFILEOPSTRUCTW = std::mem::zeroed();
        op.wFunc = FO_DELETE;
        op.pFrom = from.as_ptr();
        op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;
        if SHFileOperationW(&mut op) == 0 {
            return;
        }
    }
    let _ = std::fs::remove_file(path);
}

fn unpack(archive: &Path, into: &Path, password: Option<&str>) -> Result<String, String> {
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let lower = archive.to_string_lossy().to_ascii_lowercase();
    
    if lower.ends_with(".zip") {
        unzip(archive, into, password).map(|n| format!("{n} file{}", if n == 1 { "" } else { "s" }))
    } else {
        let tool = seven_zip().ok_or("Install 7-Zip (7-zip.org) to unpack this kind of archive.")?;
        let mut command = std::process::Command::new(tool);
        // -o: into the new folder; -aos: never overwrite; -snl- would follow links: 7-Zip keeps entries inside -o.
        // -p always: with no password given, 7-Zip must fail instead of waiting for one to be typed.
        command.arg("x").arg(format!("-o{}", into.display())).arg(format!("-p{}", password.unwrap_or(""))).args(["-y", "-aos", "-bso0", "-bsp0"]).arg(archive);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        match command.output() {
            Ok(out) if out.status.success() => Ok("unpacked".to_string()),
            Ok(out) => {
                let said = String::from_utf8_lossy(&out.stderr).to_string();
                if said.to_ascii_lowercase().contains("password") { Err(LOCKED.into()) } else { Err(format!("7-Zip could not unpack it: {}", said.lines().last().unwrap_or("").trim())) }
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in entries {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn a_zip_unpacks_into_its_own_new_folder() {
        let dir = std::env::temp_dir().join(format!("kestrel-extract-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("Course.zip");
        make_zip(&archive, &[("a.txt", b"one"), ("sub/b.txt", b"two")]);
        std::fs::create_dir_all(dir.join("Course")).unwrap(); // already taken: a new name is used
        let (into, what) = extract(&archive, 1, &[]).unwrap();
        assert_eq!(into, dir.join("Course (1)"));
        assert_eq!(what, "2 files");
        assert_eq!(std::fs::read(into.join("sub").join("b.txt")).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn entries_cannot_escape_the_folder() {
        let dir = std::env::temp_dir().join(format!("kestrel-extract-evil-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("evil.zip");
        make_zip(&archive, &[("ok.txt", b"x"), ("../escaped.txt", b"bad")]);
        assert!(extract(&archive, 0, &[]).unwrap_err().contains("unsafe path"));
        assert!(!dir.join("escaped.txt").exists());
        assert!(!dir.join("evil").exists(), "nothing half-unpacked is left");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_protected_zip_opens_with_a_password_from_the_list() {
        let dir = std::env::temp_dir().join(format!("kestrel-extract-locked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("Locked.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "right");
        writer.start_file("secret.txt", options).unwrap();
        writer.write_all(b"inside").unwrap();
        writer.finish().unwrap();
        assert!(extract(&archive, 1, &[]).unwrap_err().contains("password"), "without a password it says so");
        assert!(!dir.join("Locked").exists(), "and leaves no folder behind");
        let (into, _) = extract(&archive, 1, &["wrong".into(), " right ".into()]).unwrap();
        assert_eq!(std::fs::read(into.join("secret.txt")).unwrap(), b"inside");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn archive_names() {
        assert!(is_archive(Path::new("x/Data.TAR.GZ")) && is_archive(Path::new("a.rar")) && !is_archive(Path::new("a.mp4")));
        assert_eq!(target_folder(Path::new("/nowhere/data.tar.gz")), Path::new("/nowhere/data"));
    }
}
