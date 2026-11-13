//! Kestrel's history and settings: one SQLite file. Each download record and the settings are JSON,
//! protected for the current user, written in transactions (a crash never leaves half a save).
//! [`import_v1`] reads Flux 1.x's `history.db` so 2.0 can take over existing downloads.

pub mod secret;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("history database: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("history record: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protected data: {0}")]
    Secret(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

fn program_folder() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// A portable copy: a file named "portable" beside the program.
pub fn is_portable() -> bool {
    program_folder().is_some_and(|f| f.join("portable").is_file())
}

/// Where Kestrel keeps its data: `KESTREL_DATA_DIR` when set (tests, separate profiles), `data` beside a
/// portable copy, otherwise the system's place for an app's own data: `%LOCALAPPDATA%\Kestrel`,
/// `~/.local/share/Kestrel` (or `$XDG_DATA_HOME/Kestrel`), `~/Library/Application Support/Kestrel`.
pub fn default_directory() -> PathBuf {
    if let Some(dir) = std::env::var_os("KESTREL_DATA_DIR").map(PathBuf::from).filter(|d| d.is_absolute()) {
        return dir;
    }
    if is_portable()
        && let Some(folder) = program_folder() {
            return folder.join("data");
        }
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")))
    };
    // Only without any home folder at all (a bare service account) does it fall back to temp.
    let base = base.unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Kestrel");
    // Before the rename the folder was called Flux: it moves once. If it cannot move (in use), it
    // is used where it is, so nothing is lost.
    let old = base.join("Flux");
    if !dir.exists() && old.is_dir() && std::fs::rename(&old, &dir).is_err() {
        return old;
    }
    dir
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS downloads (id TEXT PRIMARY KEY, position INTEGER NOT NULL, data BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, data BLOB NOT NULL);",
        )?;
        Ok(Store { connection })
    }

    fn seal<T: Serialize>(value: &T) -> Result<Vec<u8>> {
        Ok(secret::protect(&serde_json::to_vec(value)?, secret::ENTROPY_V2)?)
    }
    fn open_sealed<T: DeserializeOwned>(data: &[u8]) -> Result<T> {
        Ok(serde_json::from_slice(&secret::unprotect(data, secret::ENTROPY_V2)?)?)
    }

    /// Replaces the whole list in one transaction (order kept).
    pub fn save_downloads<T: Serialize>(&mut self, downloads: &[(String, T)]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM downloads", [])?;
        {
            let mut insert = transaction.prepare("INSERT INTO downloads (id, position, data) VALUES (?1, ?2, ?3)")?;
            for (position, (id, value)) in downloads.iter().enumerate() {
                insert.execute(params![id, position as i64, Store::seal(value)?])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
    /// Saves one record without rewriting the others (a new one goes at the end of the list).
    pub fn save_download<T: Serialize>(&self, id: &str, value: &T) -> Result<()> {
        Store::upsert(&self.connection, id, &Store::seal(value)?)
    }
    /// Saves several records in one transaction (the progress of every running download at once).
    pub fn save_many<T: Serialize>(&mut self, downloads: &[(String, T)]) -> Result<()> {
        // Sealed before the transaction opens, so it lasts only as long as the writes.
        let sealed: Vec<(&String, Vec<u8>)> = downloads.iter().map(|(id, value)| Ok((id, Store::seal(value)?))).collect::<Result<_>>()?;
        let transaction = self.connection.transaction()?;
        for (id, data) in &sealed {
            Store::upsert(&transaction, id, data)?;
        }
        transaction.commit()?;
        Ok(())
    }
    fn upsert(connection: &Connection, id: &str, data: &[u8]) -> Result<()> {
        // An existing record is updated in place; only a new one needs its place at the end.
        if connection.prepare_cached("UPDATE downloads SET data = ?2 WHERE id = ?1")?.execute(params![id, data])? == 0 {
            connection
                .prepare_cached("INSERT INTO downloads (id, position, data) VALUES (?1, (SELECT COALESCE(MAX(position), -1) + 1 FROM downloads), ?2)")?
                .execute(params![id, data])?;
        }
        Ok(())
    }
    pub fn remove_download(&self, id: &str) -> Result<()> {
        self.connection.execute("DELETE FROM downloads WHERE id = ?1", params![id])?;
        Ok(())
    }
    pub fn load_downloads<T: DeserializeOwned>(&self) -> Result<Vec<(String, T)>> {
        let mut query = self.connection.prepare("SELECT id, data FROM downloads ORDER BY position")?;
        let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))?;
        rows.map(|row| { let (id, data) = row?; Ok((id, Store::open_sealed(&data)?)) }).collect()
    }
    pub fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.connection.execute(
            "INSERT INTO settings (key, data) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET data = excluded.data",
            params![key, Store::seal(value)?],
        )?;
        Ok(())
    }
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let data: Option<Vec<u8>> = self.connection.query_row("SELECT data FROM settings WHERE key = ?1", params![key], |r| r.get(0)).optional()?;
        data.map(|d| Store::open_sealed(&d)).transpose()
    }
}

/// What Flux 1.x saved, decrypted: its settings JSON and each download's JSON (camelCase), in list order.
#[derive(Debug, Default)]
pub struct V1History {
    pub settings: Option<serde_json::Value>,
    pub downloads: Vec<serde_json::Value>,
}

/// `%LOCALAPPDATA%\SwiftFetch\history.db`, where Flux 1.x keeps its history.
pub fn v1_history_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("SwiftFetch").join("history.db")
}

/// Reads Flux 1.x's history (read-only). Settings are stored as `dpapi:<base64>`; each download's
/// `payload` and `session` columns are DPAPI-protected JSON (entropy `SwiftFetch/state/v1`).
pub fn import_v1(path: &Path) -> Result<V1History> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut history = V1History::default();
    let preferences: Option<String> = connection.query_row("SELECT value FROM settings WHERE key = 'preferences'", [], |r| r.get(0)).optional()?;
    if let Some(value) = preferences {
        let plain = match value.strip_prefix("dpapi:") {
            Some(encoded) => String::from_utf8_lossy(&secret::unprotect(&base64(encoded)?, secret::ENTROPY_V1)?).into_owned(),
            None => value,
        };
        history.settings = Some(serde_json::from_str(&plain)?);
    }
    let mut query = connection.prepare("SELECT payload, session FROM downloads ORDER BY rowid")?;
    let rows = query.query_map([], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)))?;
    for row in rows {
        let (payload, session) = row?;
        let mut record: serde_json::Value = serde_json::from_slice(&secret::unprotect(&payload, secret::ENTROPY_V1)?)?;
        if let Some(session) = session {
            let session: serde_json::Value = serde_json::from_slice(&secret::unprotect(&session, secret::ENTROPY_V1)?)?;
            record["session"] = session;
        }
        history.downloads.push(record);
    }
    Ok(history)
}

fn base64(text: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let value = ALPHABET.iter().position(|a| *a == c).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad base64"))? as u32;
        buffer = buffer << 6 | value;
        bits += 6;
        if bits >= 8 { bits -= 8; out.push((buffer >> bits) as u8); }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Record { url: String, resume: kestrel_engine::Resume }

    #[test]
    fn records_and_settings_round_trip_protected() {
        let path = std::env::temp_dir().join(format!("kestrel-store-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        let a = Record { url: "https://a.test/x".into(), resume: kestrel_engine::Resume { total: Some(10), ..Default::default() } };
        store.save_downloads(&[("1".into(), a), ("2".into(), Record { url: "https://b.test/y".into(), resume: Default::default() })]).unwrap();
        store.save_download("1", &Record { url: "https://a.test/x".into(), resume: kestrel_engine::Resume { total: Some(99), ..Default::default() } }).unwrap();
        store.set("settings", &serde_json::json!({ "proxyPassword": "secret-123" })).unwrap();
        drop(store);
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(10).any(|w| w == b"secret-123"), "settings are stored protected");
        let store = Store::open(&path).unwrap();
        let loaded: Vec<(String, Record)> = store.load_downloads().unwrap();
        assert_eq!(loaded.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(loaded[0].1.resume.total, Some(99));
        assert_eq!(store.get::<serde_json::Value>("settings").unwrap().unwrap()["proxyPassword"], "secret-123");
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        // Several at once: existing ones keep their place, a new one goes last.
        let mut store = store;
        let record = |total| Record { url: "https://c.test/z".into(), resume: kestrel_engine::Resume { total: Some(total), ..Default::default() } };
        store.save_many(&[("3".into(), record(3)), ("1".into(), record(7))]).unwrap();
        let loaded: Vec<(String, Record)> = store.load_downloads().unwrap();
        assert_eq!(loaded.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
        assert_eq!(loaded[0].1.resume.total, Some(7));
        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
