//! The download's single part file: created at full size as a sparse file, written at offsets by every
//! connection, renamed into place when complete. No assembly pass and no second copy on disk.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};

#[derive(Clone)]
pub struct PartFile {
    file: Arc<File>,
    pub path: PathBuf,
}

/// `<destination>.kestrel.part`. An unfinished part from before the rename (`.flux.part`) is taken over.
pub fn part_path(destination: &Path) -> PathBuf {
    let named = |suffix: &str| {
        let mut name = destination.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(suffix);
        destination.with_file_name(name)
    };
    let path = named(".kestrel.part");
    let old = named(".flux.part");
    if !path.exists() && old.is_file() {
        let _ = std::fs::rename(&old, &path);
    }
    path
}

impl PartFile {
    /// Opens (or creates) the part file. A known size reserves the length as sparse, so writing far into
    /// the file never makes the system fill the gap with zeros first.
    pub fn open(path: &Path, size: Option<u64>, fresh: bool) -> Result<PartFile> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(Error::storage)?;
        }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(fresh).open(path).map_err(Error::storage)?;
        if fresh {
            make_sparse(&file);
        }
        if let Some(size) = size
            && file.metadata().map_err(Error::storage)?.len() != size {
                file.set_len(size).map_err(Error::storage)?;
            }
        Ok(PartFile { file: Arc::new(file), path: path.to_path_buf() })
    }

    /// Writes all of `data` at `offset`. Blocking: call from a blocking task.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        let mut written = 0;
        while written < data.len() {
            let n = positional_write(&self.file, &data[written..], offset + written as u64)?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "the disk accepted no data"));
            }
            written += n;
        }
        Ok(())
    }
    /// Data reaches the disk before the resume point is saved.
    pub fn sync(&self) -> std::io::Result<()> {
        self.file.sync_data()
    }
    #[allow(clippy::len_without_is_empty)] // the size on disk; "empty" means nothing here
    pub fn len(&self) -> std::io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
}

#[cfg(windows)]
fn positional_write(file: &File, data: &[u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(file, data, offset)
}
#[cfg(unix)]
fn positional_write(file: &File, data: &[u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(file, data, offset)
}

#[cfg(windows)]
fn make_sparse(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
    let mut returned = 0u32;
    // Best effort: FAT32/exFAT have no sparse files and simply keep the normal behaviour.
    unsafe {
        DeviceIoControl(file.as_raw_handle() as _, FSCTL_SET_SPARSE, std::ptr::null(), 0, std::ptr::null_mut(), 0, &mut returned, std::ptr::null_mut());
    }
}
#[cfg(not(windows))]
fn make_sparse(_file: &File) {}

/// Free space on the volume holding `path`, when the system says.
pub fn available_space(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let mut dir = path.to_path_buf();
        while !dir.exists() {
            dir = dir.parent()?.to_path_buf();
        }
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(free)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_land_at_their_offsets_in_any_order() {
        let dir = std::env::temp_dir().join(format!("kestrel-file-{}", std::process::id()));
        let path = part_path(&dir.join("x.bin"));
        let file = PartFile::open(&path, Some(10), true).unwrap();
        file.write_at(6, b"6789").unwrap();
        file.write_at(0, b"012345").unwrap();
        file.sync().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"0123456789");
        assert!(path.to_string_lossy().ends_with("x.bin.kestrel.part"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_part_from_before_the_rename_is_taken_over() {
        let dir = std::env::temp_dir().join(format!("kestrel-file-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("y.bin.flux.part"), b"half").unwrap();
        let path = part_path(&dir.join("y.bin"));
        assert_eq!(std::fs::read(&path).unwrap(), b"half");
        assert!(!dir.join("y.bin.flux.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
