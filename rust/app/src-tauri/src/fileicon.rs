//! The file's own icon for the download list, as Windows Explorer shows it: a finished program
//! (.exe, .msi, .ico, .lnk) shows its own logo; anything else, or an unfinished file, the icon of
//! the app registered for its type. Returned as a PNG data URL, cached by path or by extension.

use std::collections::HashMap;
use std::path::Path;
use parking_lot::Mutex;

static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// Types whose icon is inside the file itself.
fn own_icon(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "exe" | "msi" | "ico" | "lnk" | "scr" | "cpl")
}

pub fn data_url(path: &Path, exists: bool) -> Option<String> {
    let mine = exists && own_icon(path);
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let key = if mine { path.to_string_lossy().to_lowercase() } else { format!(".{ext}") };
    if let Some(hit) = CACHE.lock().get_or_insert_with(HashMap::new).get(&key) {
        return hit.clone();
    }
    let png = platform::icon_png(path, mine);
    let url = png.map(|bytes| {
        use base64::Engine;
        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
    });
    CACHE.lock().get_or_insert_with(HashMap::new).insert(key, url.clone());
    url
}

/// Encodes 32×32 (or any size) RGBA pixels as PNG.
fn encode(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(out)
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW, HGDIOBJ};
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

    pub fn icon_png(path: &Path, own: bool) -> Option<Vec<u8>> {
        unsafe {
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            // Unfinished or not a program: ask by name only, so Windows answers for the type.
            let name: Vec<u16> = if own { path.as_os_str().encode_wide().chain([0]).collect() } else { format!("file.{}", path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()).encode_utf16().chain([0]).collect() };
            let mut info: SHFILEINFOW = std::mem::zeroed();
            let flags = SHGFI_ICON | SHGFI_LARGEICON | if own { 0 } else { SHGFI_USEFILEATTRIBUTES };
            if SHGetFileInfoW(name.as_ptr(), FILE_ATTRIBUTE_NORMAL, &mut info, std::mem::size_of::<SHFILEINFOW>() as u32, flags) == 0 || info.hIcon.is_null() {
                return None;
            }
            let png = pixels(info.hIcon);
            DestroyIcon(info.hIcon);
            png
        }
    }

    unsafe fn bits(bitmap: HGDIOBJ, width: i32, height: i32) -> Option<Vec<u8>> {
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let mut header: BITMAPINFO = std::mem::zeroed();
            header.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: width, biHeight: -height, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
            let mut data = vec![0u8; (width * height * 4) as usize];
            let lines = GetDIBits(dc, bitmap as _, 0, height as u32, data.as_mut_ptr().cast(), &mut header, DIB_RGB_COLORS);
            DeleteDC(dc);
            (lines == height).then_some(data)
        }
    }

    unsafe fn pixels(icon: HICON) -> Option<Vec<u8>> {
        unsafe {
            let mut info: ICONINFO = std::mem::zeroed();
            if GetIconInfo(icon, &mut info) == 0 {
                return None;
            }
            let result = (|| {
                let mut bitmap: BITMAP = std::mem::zeroed();
                if info.hbmColor.is_null() || GetObjectW(info.hbmColor as _, std::mem::size_of::<BITMAP>() as i32, (&mut bitmap as *mut BITMAP).cast()) == 0 {
                    return None;
                }
                let (w, h) = (bitmap.bmWidth, bitmap.bmHeight);
                let mut bgra = bits(info.hbmColor as _, w, h)?;
                // Old icons have no alpha: the mask says which pixels are transparent.
                if bgra.chunks(4).all(|p| p[3] == 0) {
                    let mask = bits(info.hbmMask as _, w, h)?;
                    for (p, m) in bgra.chunks_mut(4).zip(mask.chunks(4)) {
                        p[3] = if m[0] == 0 { 255 } else { 0 };
                    }
                }
                for p in bgra.chunks_mut(4) {
                    p.swap(0, 2); // BGRA → RGBA
                }
                super::encode(w as u32, h as u32, &bgra)
            })();
            if !info.hbmColor.is_null() { DeleteObject(info.hbmColor as _); }
            if !info.hbmMask.is_null() { DeleteObject(info.hbmMask as _); }
            result
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn icon_png(_path: &std::path::Path, _own: bool) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_program_shows_its_own_logo_not_the_plain_program_icon() {
        let notepad = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or(r"C:\Windows".into())).join("notepad.exe");
        let own = data_url(&notepad, true).expect("notepad's icon");
        let plain = data_url(Path::new("Unfinished-Setup.exe"), false).expect("the .exe type's icon");
        assert!(own.starts_with("data:image/png;base64,"));
        assert_ne!(own, plain, "a finished program has its own logo");
    }
}
