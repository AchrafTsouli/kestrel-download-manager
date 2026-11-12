//! Torrents: magnet links and .torrent files, downloaded by aria2 running as a child process.
//! A port of Flux 1.x's `Torrents.cs` (proven on a real torrent: chosen files only, verified film).

mod aria2;
mod bencode;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

pub use aria2::{Outcome, Status, Tools};

/// Well-known public trackers, added after the torrent's own: many magnet links carry none that work.
pub const PUBLIC_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "https://tracker.tamersunion.org:443/announce",
    "http://tracker.opentrackr.org:1337/announce",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentFile {
    /// 1-based, as aria2's --select-file counts.
    pub index: usize,
    /// Relative path inside the torrent's folder, every part made safe.
    pub path: String,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub name: String,
    pub info_hash: String,
    pub files: Vec<TorrentFile>,
    pub total_bytes: u64,
}

/// What a torrent download fetches: its saved file list and the files chosen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Selection {
    pub info_hash: String,
    pub name: String,
    pub metadata_file: PathBuf,
    /// 1-based file indexes; empty means every file.
    pub files: Vec<usize>,
    pub selected_bytes: u64,
    /// Share the finished files with others for this long (0 = not at all).
    pub seed_minutes: u32,
}

impl Selection {
    pub fn validate(&self, url: &str) -> Result<(), String> {
        let hex = self.info_hash.len() == 40 && self.info_hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let metadata_name = self.metadata_file.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase());
        if !hex
            || magnet_hash(url).as_deref() != Some(self.info_hash.as_str())
            || self.name.is_empty()
            || self.name.chars().count() > 255
            || self.name != kestrel_engine::sanitize_file_name(&self.name)
            || !self.metadata_file.is_absolute()
            || metadata_name != Some(format!("{}.torrent", self.info_hash))
            || self.files.len() > 10_000
            || self.files.iter().any(|i| !(1..=100_000).contains(i))
            || self.seed_minutes > 1440
        {
            return Err("Invalid torrent selection.".into());
        }
        Ok(())
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(value) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base32(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut out, mut buffer, mut bits) = (Vec::new(), 0u32, 0);
    for c in text.to_ascii_uppercase().bytes() {
        buffer = buffer << 5 | ALPHABET.iter().position(|a| *a == c).unwrap_or(0) as u32;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The 40-hex info hash of a magnet link (hex or base32 form).
pub fn magnet_hash(magnet: &str) -> Option<String> {
    if magnet.len() > 8192 || !magnet.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("magnet:?")) {
        return None;
    }
    for pair in magnet[8..].split('&') {
        let Some((key, value)) = pair.split_once('=') else { continue };
        if !key.eq_ignore_ascii_case("xt") {
            continue;
        }
        let value = percent_decode(value);
        let Some(hash) = value.get(..9).filter(|p| p.eq_ignore_ascii_case("urn:btih:")).map(|_| &value[9..]) else { continue };
        if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
        if hash.len() == 32 && hash.bytes().all(|b| b.is_ascii_alphabetic() || (b'2'..=b'7').contains(&b)) {
            return Some(hex(&base32(hash)));
        }
    }
    None
}

pub fn magnet_name(magnet: &str) -> Option<String> {
    magnet.get(8..)?.split('&').find_map(|pair| pair.strip_prefix("dn=")).map(|name| percent_decode(&name.replace('+', " ")))
}

/// A magnet link for a hash and name (what a torrent download is listed under).
pub fn magnet(hash: &str, name: &str) -> String {
    let encoded: String = name.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    format!("magnet:?xt=urn:btih:{hash}&dn={encoded}")
}

/// Reads a .torrent file: name, files (safe relative paths) and the SHA-1 info hash.
pub fn parse(data: &[u8]) -> Result<Info, String> {
    const INVALID: &str = "This is not a valid torrent file.";
    if data.is_empty() || data.len() > 50 * 1024 * 1024 {
        return Err(INVALID.into());
    }
    let (root, info_span) = bencode::read_top(data).ok_or(INVALID)?;
    let info_span = info_span.ok_or(INVALID)?;
    let info = root.get(b"info").filter(|i| i.as_dict().is_some()).ok_or(INVALID)?;
    let hash = hex(&Sha1::digest(&data[info_span.0..info_span.1]));
    let text = |d: &bencode::Value, key: &str| {
        d.get(format!("{key}.utf-8").as_bytes()).or_else(|| d.get(key.as_bytes())).and_then(|v| v.as_bytes()).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default()
    };
    let mut name = kestrel_engine::sanitize_file_name(&text(info, "name"));
    if name.is_empty() || name == "download.bin" {
        name = format!("torrent-{}", &hash[..8]);
    }
    let mut files = Vec::new();
    if let Some(entries) = info.get(b"files").and_then(|f| f.as_list()) {
        const DAMAGED: &str = "The torrent's file list is damaged.";
        for entry in entries {
            let length = entry.get(b"length").and_then(|l| l.as_int()).filter(|l| *l >= 0).ok_or(DAMAGED)?;
            let parts = entry.get(b"path.utf-8").or_else(|| entry.get(b"path")).and_then(|p| p.as_list()).filter(|p| !p.is_empty()).ok_or(DAMAGED)?;
            // Every part is made safe: a torrent can never write outside its own folder.
            let mut path = Vec::new();
            for part in parts {
                path.push(kestrel_engine::sanitize_file_name(&String::from_utf8_lossy(part.as_bytes().ok_or(DAMAGED)?)));
            }
            files.push(TorrentFile { index: files.len() + 1, path: path.join("\\"), length: length as u64 });
            if files.len() > 100_000 {
                return Err("The torrent has too many files.".into());
            }
        }
    } else if let Some(length) = info.get(b"length").and_then(|l| l.as_int()).filter(|l| *l >= 0) {
        files.push(TorrentFile { index: 1, path: name.clone(), length: length as u64 });
    } else {
        return Err("The torrent has no files.".into());
    }
    let total_bytes = files.iter().map(|f| f.length).sum();
    Ok(Info { name, info_hash: hash, files, total_bytes })
}

/// Where a magnet's or file's metadata is kept: `<cache>/<hash>.torrent`.
pub fn metadata_path(cache: &Path, hash: &str) -> PathBuf {
    cache.join(format!("{hash}.torrent"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(info: &str) -> Vec<u8> {
        format!("d8:announce9:udp://x:113:creation datei1e4:info{info}e").into_bytes()
    }

    #[test]
    fn magnet_hashes_in_both_forms() {
        let hex = "c9e15763f722f23e98a29decdfae341b98d53056";
        assert_eq!(magnet_hash(&format!("magnet:?xt=urn:btih:{}&dn=Sintel", hex.to_uppercase())).as_deref(), Some(hex));
        // The same hash in base32.
        assert_eq!(magnet_hash("magnet:?dn=x&xt=urn%3Abtih%3AZHQVOY7XELZD5GFCTXWN7LRUDOMNKMCW").as_deref(), Some(hex));
        assert_eq!(magnet_hash("magnet:?xt=urn:sha1:abc"), None);
        assert_eq!(magnet_hash("https://example.test/a.torrent"), None);
        assert_eq!(magnet_name("magnet:?xt=urn:btih:x&dn=Big+Buck%20Bunny").as_deref(), Some("Big Buck Bunny"));
        assert_eq!(magnet(hex, "A b"), format!("magnet:?xt=urn:btih:{hex}&dn=A%20b"));
    }

    #[test]
    fn a_torrent_lists_its_files_and_hashes_its_info() {
        let info = "d5:filesld6:lengthi5e4:pathl3:sub5:a.txteed6:lengthi7e4:pathl5:b.mkveee4:name6:Course12:piece lengthi16384ee";
        let data = torrent(info);
        let parsed = parse(&data).unwrap();
        assert_eq!(parsed.name, "Course");
        assert_eq!(parsed.files, vec![
            TorrentFile { index: 1, path: "sub\\a.txt".into(), length: 5 },
            TorrentFile { index: 2, path: "b.mkv".into(), length: 7 },
        ]);
        assert_eq!(parsed.total_bytes, 12);
        assert_eq!(parsed.info_hash, hex(&Sha1::digest(info.as_bytes())));
    }

    #[test]
    fn paths_cannot_leave_the_torrent_folder() {
        let data = torrent("d5:filesld6:lengthi1e4:pathl2:..6:x:.exeeee4:name4:evile");
        let parsed = parse(&data).unwrap();
        assert!(!parsed.files[0].path.contains(".."), "{}", parsed.files[0].path);
        assert!(!parsed.files[0].path.contains(':'));
    }

    #[test]
    fn a_single_file_torrent_and_damaged_ones() {
        let parsed = parse(&torrent("d6:lengthi42e4:name8:film.mkve")).unwrap();
        assert_eq!((parsed.files.len(), parsed.files[0].path.as_str(), parsed.total_bytes), (1, "film.mkv", 42));
        for bad in [&b""[..], b"d4:infoi1ee", b"d4:infod4:name1:xee", b"l", b"d4:infod5:filesld6:lengthi-1e4:pathl1:xeee4:name1:xee"] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn selections_must_match_their_magnet() {
        let hash = "c9e15763f722f23e98a29decdfae341b98d53056";
        let url = magnet(hash, "Sintel");
        let good = Selection { info_hash: hash.into(), name: "Sintel".into(), metadata_file: std::env::temp_dir().join(format!("{hash}.torrent")), files: vec![2, 6], ..Default::default() };
        assert!(good.validate(&url).is_ok());
        assert!(Selection { name: "..\\x".into(), ..good.clone() }.validate(&url).is_err());
        assert!(Selection { files: vec![0], ..good.clone() }.validate(&url).is_err());
        assert!(good.validate(&magnet(&"0".repeat(40), "Sintel")).is_err());
    }
}
