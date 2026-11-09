//! Flux's history and settings: one SQLite file. Each download record and the settings are JSON,
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

/// `%LOCALAPPDATA%\Flux` on Windows (the app may pass another folder, e.g. for a portable copy).
pub fn default_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Flux")
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
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
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
    /// Saves one record (progress checkpoints) without rewriting the others.
    pub fn save_download<T: Serialize>(&self, id: &str, value: &T) -> Result<()> {
        let position: i64 = self.connection.query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM downloads", [], |r| r.get(0))?;
        self.connection.execute(
            "INSERT INTO downloads (id, position, data) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            params![id, position, Store::seal(value)?],
        )?;
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
    struct Record { url: String, resume: flux_engine::Resume }

    #[test]
    fn records_and_settings_round_trip_protected() {
        let path = std::env::temp_dir().join(format!("flux-store-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        let a = Record { url: "https://a.test/x".into(), resume: flux_engine::Resume { total: Some(10), ..Default::default() } };
        store.save_downloads(&[("1".into(), a), ("2".into(), Record { url: "https://b.test/y".into(), resume: Default::default() })]).unwrap();
        store.save_download("1", &Record { url: "https://a.test/x".into(), resume: flux_engine::Resume { total: Some(99), ..Default::default() } }).unwrap();
        store.set("settings", &serde_json::json!({ "proxyPassword": "secret-123" })).unwrap();
        drop(store);
        let raw = std::fs::read(&path).unwrap();
        #[cfg(windows)]
        assert!(!raw.windows(10).any(|w| w == b"secret-123"), "settings are stored protected");
        let store = Store::open(&path).unwrap();
        let loaded: Vec<(String, Record)> = store.load_downloads().unwrap();
        assert_eq!(loaded.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(loaded[0].1.resume.total, Some(99));
        assert_eq!(store.get::<serde_json::Value>("settings").unwrap().unwrap()["proxyPassword"], "secret-123");
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
