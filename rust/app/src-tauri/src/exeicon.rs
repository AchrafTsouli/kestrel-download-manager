//! A program's logo before it is downloaded, as IDM shows it: the .exe's headers and its resources
//! section are read from the server with range requests (usually a few hundred KB, never the whole
//! file), and its main icon is rebuilt as a small .ico.

use reqwest::header::{CONTENT_RANGE, HeaderMap, HeaderValue, RANGE};
use url::Url;

/// The most read from the server for one icon.
const MAX_RESOURCES: u64 = 8 * 1024 * 1024;
const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;

fn u16_at(b: &[u8], i: usize) -> Option<u32> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u32)
}
fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

async fn range(client: &reqwest::Client, url: &Url, headers: &HeaderMap, from: u64, len: u64) -> Option<Vec<u8>> {
    let mut headers = headers.clone();
    headers.insert(RANGE, HeaderValue::from_str(&format!("bytes={from}-{}", from + len - 1)).ok()?);
    let response = client.get(url.clone()).headers(headers).send().await.ok()?;
    // Without ranges the whole file would come: not worth it for an icon.
    if response.status().as_u16() != 206 || response.headers().get(CONTENT_RANGE).is_none() {
        return None;
    }
    let body = response.bytes().await.ok()?;
    (body.len() as u64 <= len).then(|| body.to_vec())
}

/// Where the resources section is in the file: (file offset, size, its RVA).
fn resources_place(head: &[u8]) -> Result<(u64, u64, u32), usize> {
    let bad = Err(0);
    if head.get(0..2) != Some(b"MZ") { return bad; }
    let pe = u32_at(head, 0x3C).ok_or(0usize)? as usize;
    let need = |at: usize| if at > head.len() { Err(at) } else { Ok(()) };
    need(pe + 24)?;
    if head.get(pe..pe + 4) != Some(b"PE\0\0") { return bad; }
    let sections = u16_at(head, pe + 6).ok_or(0usize)? as usize;
    let optional_size = u16_at(head, pe + 20).ok_or(0usize)? as usize;
    let optional = pe + 24;
    need(optional + optional_size + sections * 40)?;
    let directories = match u16_at(head, optional) { Some(0x10b) => optional + 96, Some(0x20b) => optional + 112, _ => return bad };
    let (rva, size) = (u32_at(head, directories + 16).ok_or(0usize)?, u32_at(head, directories + 20).ok_or(0usize)?);
    if rva == 0 || size == 0 { return bad; }
    for s in 0..sections {
        let at = optional + optional_size + s * 40;
        let (virtual_size, va, raw_size, raw) = (u32_at(head, at + 8).unwrap_or(0), u32_at(head, at + 12).unwrap_or(0), u32_at(head, at + 16).unwrap_or(0), u32_at(head, at + 20).unwrap_or(0));
        if rva >= va && rva < va + virtual_size.max(raw_size) {
            let offset = raw as u64 + (rva - va) as u64;
            let available = (raw_size as u64).saturating_sub((rva - va) as u64);
            return Ok((offset, (size as u64).max(available).min(MAX_RESOURCES), rva));
        }
    }
    bad
}

/// The entries of one resource directory: (id or name offset, target, is a directory).
fn entries(res: &[u8], at: usize) -> Vec<(u32, usize, bool)> {
    let (named, ids) = (u16_at(res, at + 12).unwrap_or(0) as usize, u16_at(res, at + 14).unwrap_or(0) as usize);
    (0..(named + ids).min(4096))
        .filter_map(|i| {
            let e = at + 16 + i * 8;
            let (name, target) = (u32_at(res, e)?, u32_at(res, e + 4)?);
            Some((name, (target & 0x7FFF_FFFF) as usize, target & 0x8000_0000 != 0))
        })
        .collect()
}

/// The data of the first language of resource `kind` (with `id`, or the first one).
fn resource<'a>(res: &'a [u8], base_rva: u32, kind: u32, id: Option<u32>) -> Option<&'a [u8]> {
    let (_, types, _) = entries(res, 0).into_iter().find(|(n, _, dir)| *n == kind && *dir)?;
    let (_, names, _) = entries(res, types).into_iter().find(|(n, _, dir)| *dir && id.is_none_or(|id| *n == id))?;
    let (_, data, dir) = entries(res, names).into_iter().next()?;
    if dir { return None; }
    let (rva, size) = (u32_at(res, data)?, u32_at(res, data + 4)? as usize);
    let start = rva.checked_sub(base_rva)? as usize;
    res.get(start..start + size)
}

/// The main icon (the first icon group), at the size closest to 32 px, as a one-image .ico.
pub fn ico_from_resources(res: &[u8], base_rva: u32) -> Option<Vec<u8>> {
    let group = resource(res, base_rva, RT_GROUP_ICON, None)?;
    let count = u16_at(group, 4)? as usize;
    let choice = (0..count.min(64))
        .filter_map(|i| {
            let e = 6 + i * 14;
            let width = match *group.get(e)? { 0 => 256, w => w as u32 };
            Some((e, width, u16_at(group, e + 6)?))
        })
        // Closest to 32 px (bigger is better than smaller), then the most colours.
        .min_by_key(|(_, w, bits)| (if *w >= 32 { *w - 32 } else { (32 - *w) * 4 }, u32::MAX - *bits))?;
    let e = choice.0;
    let image = resource(res, base_rva, RT_ICON, Some(u16_at(group, e + 12)?))?;
    let mut ico = vec![0, 0, 1, 0, 1, 0];
    ico.extend_from_slice(&group[e..e + 8]); // width, height, colours, reserved, planes, bit count
    ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(image);
    Some(ico)
}

/// Reads the logo of the .exe at `url` from the server. None when the server does not send byte
/// ranges, or the file is not a Windows program, or it has no icon.
pub async fn remote(url: &str, session: Option<&flux_engine::Session>) -> Option<Vec<u8>> {
    let url = Url::parse(url).ok()?;
    let mut headers = HeaderMap::new();
    if let Some(session) = session {
        session.apply(&url, &mut headers);
    }
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().ok()?;
    let mut head = range(&client, &url, &headers, 0, 4096).await?;
    let (offset, size, rva) = match resources_place(&head) {
        Ok(place) => place,
        // Many sections: the headers go on past the first 4 KB.
        Err(need) if need > 0 && need <= 65536 => {
            head = range(&client, &url, &headers, 0, need as u64).await?;
            resources_place(&head).ok()?
        }
        Err(_) => return None,
    };
    let res = range(&client, &url, &headers, offset, size).await?;
    ico_from_resources(&res, rva)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn reads_the_logo_of_a_real_program() {
        let notepad = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or(r"C:\Windows".into())).join("notepad.exe");
        let bytes = std::fs::read(notepad).unwrap();
        let (offset, size, rva) = resources_place(&bytes[..4096.min(bytes.len())]).or_else(|need| resources_place(&bytes[..need])).unwrap();
        let end = (offset + size).min(bytes.len() as u64) as usize;
        let ico = ico_from_resources(&bytes[offset as usize..end], rva).expect("notepad has an icon");
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert!(ico.len() > 100);
    }
}

#[cfg(test)]
mod online {
    /// Reads 7-Zip's logo from its download server. Run with: cargo test -p flux-app exeicon_online -- --ignored
    #[test]
    #[ignore]
    fn exeicon_online() {
        let ico = tokio::runtime::Runtime::new().unwrap().block_on(super::remote("https://www.7-zip.org/a/7z2408-x64.exe", None));
        let ico = ico.expect("7-Zip's icon from the server");
        println!("icon: {} bytes", ico.len());
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
    }
}
