//! "Preview" in the start dialog: the files inside a .zip still on the server, read from its
//! central directory with a couple of range requests (nothing else is downloaded).

use reqwest::header::{HeaderMap, HeaderValue, RANGE};
use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub packed: u64,
    /// "YYYY-MM-DD HH:MM", the archive's own (local) time.
    pub modified: String,
    pub folder: bool,
}

const MAX_DIRECTORY: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

fn u16_at(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u64)
}
fn u32_at(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64)
}
fn u64_at(b: &[u8], i: usize) -> Option<u64> {
    b.get(i..i + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

struct Remote {
    client: reqwest::Client,
    url: Url,
    headers: HeaderMap,
}

impl Remote {
    /// `range` is "start-end" or "-suffix". Returns the bytes and the file's total size.
    async fn get(&self, range: String) -> Result<(Vec<u8>, u64), String> {
        let mut headers = self.headers.clone();
        headers.insert(RANGE, HeaderValue::from_str(&format!("bytes={range}")).map_err(|e| e.to_string())?);
        let response = self.client.get(self.url.clone()).headers(headers).send().await.map_err(|e| format!("The server did not answer: {e}"))?;
        if response.status().as_u16() != 206 {
            return Err("The server does not send parts of this file, so its contents cannot be previewed.".into());
        }
        let total = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or("The server did not say how large the file is.")?;
        let body = response.bytes().await.map_err(|e| e.to_string())?;
        if body.len() as u64 > MAX_DIRECTORY + 70_000 {
            return Err("The server sent more than was asked.".into());
        }
        Ok((body.to_vec(), total))
    }
}

fn dos_time(date: u64, time: u64) -> String {
    let (y, mo, d) = (1980 + (date >> 9), (date >> 5) & 15, date & 31);
    let (h, mi) = (time >> 11, (time >> 5) & 63);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

/// Reads the central directory's entries.
fn entries(directory: &[u8], count: u64) -> Result<Vec<Entry>, String> {
    let mut list = Vec::new();
    let mut at = 0usize;
    while list.len() < count.min(MAX_ENTRIES as u64) as usize {
        if u32_at(directory, at) != Some(0x0201_4b50) {
            break;
        }
        let bad = || "The archive's list of files is damaged.".to_string();
        let (time, date) = (u16_at(directory, at + 12).ok_or_else(bad)?, u16_at(directory, at + 14).ok_or_else(bad)?);
        let mut packed = u32_at(directory, at + 20).ok_or_else(bad)?;
        let mut size = u32_at(directory, at + 24).ok_or_else(bad)?;
        let name_len = u16_at(directory, at + 28).ok_or_else(bad)? as usize;
        let extra_len = u16_at(directory, at + 30).ok_or_else(bad)? as usize;
        let comment_len = u16_at(directory, at + 32).ok_or_else(bad)? as usize;
        let flags = u16_at(directory, at + 8).ok_or_else(bad)?;
        let raw = directory.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
        // Bit 11: UTF-8 names; older archives use the DOS code page, close enough as lossy UTF-8.
        let name = if flags & 0x800 != 0 { String::from_utf8_lossy(raw).into_owned() } else { raw.iter().map(|&b| if b < 0x80 { b as char } else { '?' }).collect() };
        // Zip64: the real sizes are in the 0x0001 extra field, in this order, when the 32-bit ones are full.
        let extra = directory.get(at + 46 + name_len..at + 46 + name_len + extra_len).ok_or_else(bad)?;
        let mut e = 0;
        while e + 4 <= extra.len() {
            let (id, len) = (u16_at(extra, e).unwrap_or(0), u16_at(extra, e + 2).unwrap_or(0) as usize);
            if id == 1 {
                let mut f = e + 4;
                if size == 0xFFFF_FFFF { size = u64_at(extra, f).unwrap_or(size); f += 8; }
                if packed == 0xFFFF_FFFF { packed = u64_at(extra, f).unwrap_or(packed); }
            }
            e += 4 + len;
        }
        list.push(Entry { folder: name.ends_with('/'), name, size, packed, modified: dos_time(date, time) });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(list)
}

pub async fn list(url: &str, session: Option<&flux_engine::Session>) -> Result<Vec<Entry>, String> {
    let url = Url::parse(url).map_err(|e| e.to_string())?;
    let mut headers = HeaderMap::new();
    if let Some(session) = session {
        session.apply(&url, &mut headers);
    }
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(25)).build().map_err(|e| e.to_string())?;
    let remote = Remote { client, url, headers };
    // The end record is in the last 22 bytes, or up to 64 KB before them when the archive has a comment.
    let (tail, total) = remote.get("-65558".into()).await?;
    let tail_start = total.saturating_sub(tail.len() as u64);
    let end = (0..tail.len().saturating_sub(21)).rev().find(|&i| u32_at(&tail, i) == Some(0x0605_4b50)).ok_or("This is not a zip archive (or its end is missing).")?;
    let mut count = u16_at(&tail, end + 10).unwrap_or(0);
    let mut dir_size = u32_at(&tail, end + 12).unwrap_or(0);
    let mut dir_offset = u32_at(&tail, end + 16).unwrap_or(0);
    if dir_offset == 0xFFFF_FFFF || count == 0xFFFF || dir_size == 0xFFFF_FFFF {
        // Zip64: a locator just before the end record points at the Zip64 end record.
        let locator = end.checked_sub(20).filter(|&l| u32_at(&tail, l) == Some(0x0706_4b50)).ok_or("The archive's Zip64 record is missing.")?;
        let record_at = u64_at(&tail, locator + 8).ok_or("The archive's Zip64 record is damaged.")?;
        let record = if record_at >= tail_start { tail[(record_at - tail_start) as usize..].to_vec() } else { remote.get(format!("{record_at}-{}", record_at + 55)).await?.0 };
        if u32_at(&record, 0) != Some(0x0606_4b50) {
            return Err("The archive's Zip64 record is damaged.".into());
        }
        count = u64_at(&record, 32).unwrap_or(0);
        dir_size = u64_at(&record, 40).unwrap_or(0);
        dir_offset = u64_at(&record, 48).unwrap_or(0);
    }
    if dir_size > MAX_DIRECTORY {
        return Err("This archive holds too many files to preview.".into());
    }
    let directory = if dir_offset >= tail_start && dir_offset + dir_size <= total {
        tail[(dir_offset - tail_start) as usize..(dir_offset - tail_start + dir_size) as usize].to_vec()
    } else if dir_size > 0 {
        remote.get(format!("{dir_offset}-{}", dir_offset + dir_size - 1)).await?.0
    } else {
        Vec::new()
    };
    entries(&directory, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_a_central_directory() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            zip.start_file("film/part one.mp4", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(&[7u8; 5000]).unwrap();
            zip.add_directory("film/extras/", zip::write::SimpleFileOptions::default()).unwrap();
            zip.finish().unwrap();
        }
        let data = bytes.into_inner();
        let end = (0..data.len() - 21).rev().find(|&i| u32_at(&data, i) == Some(0x0605_4b50)).unwrap();
        let (count, size, offset) = (u16_at(&data, end + 10).unwrap(), u32_at(&data, end + 12).unwrap() as usize, u32_at(&data, end + 16).unwrap() as usize);
        let list = entries(&data[offset..offset + size], count).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "film/part one.mp4");
        assert_eq!(list[0].size, 5000);
        assert!(list[0].packed < 5000);
        assert!(list[1].folder);
    }
}
