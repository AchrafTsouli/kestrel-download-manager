//! Data protected for the current user. Windows: DPAPI (only this Windows account can read it).
//! Elsewhere (until the keyring integration of M8): stored as is.

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
    pub fn protect(data: &[u8], _entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        Ok(data.to_vec())
    }
    pub fn unprotect(data: &[u8], _entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        Ok(data.to_vec())
    }
}

pub use imp::{protect, unprotect};

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip_and_wrong_entropy_fails() {
        let sealed = super::protect(b"site password", super::ENTROPY_V2).unwrap();
        assert_eq!(super::unprotect(&sealed, super::ENTROPY_V2).unwrap(), b"site password");
        #[cfg(windows)]
        {
            assert_ne!(sealed, b"site password");
            assert!(super::unprotect(&sealed, super::ENTROPY_V1).is_err());
        }
    }
}
