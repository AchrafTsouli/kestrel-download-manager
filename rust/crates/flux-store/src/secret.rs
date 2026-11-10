//! Data protected for the current user. Windows: DPAPI (only this Windows account can read it).
//! Linux and macOS: ChaCha20-Poly1305 with a random key in Flux's data folder that only the user
//! can read (the same protection DPAPI gives: other accounts cannot read it, programs of the same
//! user can).

/// Entropy of Flux 2.x records.
pub const ENTROPY_V2: &[u8] = b"Flux/state/v2";
/// Entropy Flux 1.x used (`SwiftFetch.Core/WindowsSecrets.cs`), needed to import its history.
pub const ENTROPY_V1: &[u8] = b"SwiftFetch/state/v1";

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as HLOCAL) };
        bytes
    }
    pub fn protect(data: &[u8], entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        let (input, extra) = (blob(data), blob(entropy));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe { CryptProtectData(&input, std::ptr::null(), &extra, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        if ok == 0 { return Err(std::io::Error::last_os_error()); }
        Ok(unsafe { take(out) })
    }
    pub fn unprotect(data: &[u8], entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        let (input, extra) = (blob(data), blob(entropy));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), &extra, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        if ok == 0 { return Err(std::io::Error::last_os_error()); }
        Ok(unsafe { take(out) })
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io::{Error, ErrorKind};
    use std::sync::OnceLock;

    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

    /// The user's key (`<data folder>/secret.key`, mode 0600), made on first use.
    fn key() -> std::io::Result<&'static Key> {
        static KEY: OnceLock<Key> = OnceLock::new();
        if let Some(key) = KEY.get() {
            return Ok(key);
        }
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let dir = crate::default_directory();
        std::fs::create_dir_all(&dir)?;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let path = dir.join("secret.key");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            Ok(_) => return Err(Error::new(ErrorKind::InvalidData, "Kestrel's key file is damaged.")),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let mut fresh = vec![0u8; 32];
                getrandom::getrandom(&mut fresh).map_err(|e| Error::other(e.to_string()))?;
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
                file.write_all(&fresh)?;
                file.sync_all()?;
                fresh
            }
            Err(e) => return Err(e),
        };
        Ok(KEY.get_or_init(|| *Key::from_slice(&bytes)))
    }

    /// nonce (12 bytes) followed by the sealed data; the entropy is bound in as associated data.
    pub fn protect(data: &[u8], entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut nonce).map_err(|e| Error::other(e.to_string()))?;
        let sealed = ChaCha20Poly1305::new(key()?).encrypt(Nonce::from_slice(&nonce), Payload { msg: data, aad: entropy }).map_err(|_| Error::other("encryption failed"))?;
        Ok([nonce.as_slice(), &sealed].concat())
    }
    pub fn unprotect(data: &[u8], entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        if data.len() < 12 + 16 {
            return Err(Error::new(ErrorKind::InvalidData, "The protected data is too short."));
        }
        let (nonce, sealed) = data.split_at(12);
        ChaCha20Poly1305::new(key()?).decrypt(Nonce::from_slice(nonce), Payload { msg: sealed, aad: entropy }).map_err(|_| Error::new(ErrorKind::InvalidData, "The protected data cannot be read with this user's key."))
    }
}

pub use imp::{protect, unprotect};

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip_and_wrong_entropy_fails() {
        let sealed = super::protect(b"site password", super::ENTROPY_V2).unwrap();
        assert_eq!(super::unprotect(&sealed, super::ENTROPY_V2).unwrap(), b"site password");
        assert_ne!(sealed, b"site password");
        assert!(super::unprotect(&sealed, super::ENTROPY_V1).is_err());
    }
}
