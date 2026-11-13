//! A backup of the release key that opens on any computer, with a password the owner chooses.
//! The key is encrypted with AES-256-GCM; the AES key comes from the password through
//! PBKDF2-HMAC-SHA256 (600,000 rounds, random salt). Without the password the file is useless, so it
//! can be kept in a cloud drive; the password must be kept somewhere else.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use rand_core::RngCore;
use sha2::Sha256;

const MAGIC: &str = "kestrel-release-key-backup";
const ROUNDS: u32 = 600_000;
pub const MIN_PASSWORD: usize = 12;

fn derive(password: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, rounds, &mut key);
    key
}

/// The backup file's text for `secret` (the key's PKCS#8 bytes).
pub fn seal(secret: &[u8], password: &str) -> Result<String, String> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(format!("Choose a password of at least {MIN_PASSWORD} characters."));
    }
    let (mut salt, mut nonce) = ([0u8; 16], [0u8; 12]);
    rand_core::OsRng.fill_bytes(&mut salt);
    rand_core::OsRng.fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(&derive(password, &salt, ROUNDS)).map_err(|e| e.to_string())?;
    let sealed = cipher.encrypt(Nonce::from_slice(&nonce), secret).map_err(|_| "Encryption failed.".to_string())?;
    let file = serde_json::json!({
        "format": MAGIC,
        "kdf": "pbkdf2-hmac-sha256",
        "rounds": ROUNDS,
        "salt": B64.encode(salt),
        "cipher": "aes-256-gcm",
        "nonce": B64.encode(nonce),
        "data": B64.encode(sealed),
    });
    serde_json::to_string_pretty(&file).map_err(|e| e.to_string())
}

/// The key's PKCS#8 bytes from a backup file's text.
pub fn open(text: &str, password: &str) -> Result<Vec<u8>, String> {
    let file: serde_json::Value = serde_json::from_str(text).map_err(|_| "This is not a Kestrel key backup.".to_string())?;
    if file["format"] != MAGIC {
        return Err("This is not a Kestrel key backup.".into());
    }
    let field = |name: &str| file[name].as_str().and_then(|s| B64.decode(s).ok()).ok_or_else(|| "The backup is damaged.".to_string());
    let rounds = file["rounds"].as_u64().filter(|r| (100_000..=10_000_000).contains(r)).ok_or("The backup is damaged.")? as u32;
    let (salt, nonce, data) = (field("salt")?, field("nonce")?, field("data")?);
    if nonce.len() != 12 {
        return Err("The backup is damaged.".into());
    }
    let cipher = Aes256Gcm::new_from_slice(&derive(password, &salt, rounds)).map_err(|e| e.to_string())?;
    cipher.decrypt(Nonce::from_slice(&nonce), data.as_slice()).map_err(|_| "Wrong password (or a damaged backup).".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backup_opens_only_with_its_password() {
        let secret = b"pkcs8 bytes of a key";
        let text = seal(secret, "correct horse battery").unwrap();
        assert!(!text.contains("pkcs8"), "the key is not readable in the file");
        assert_eq!(open(&text, "correct horse battery").unwrap(), secret);
        assert!(open(&text, "correct horse batterY").unwrap_err().contains("Wrong password"));
        assert!(seal(secret, "short").is_err(), "a short password is refused");
        let tampered = text.replace("\"data\": \"", "\"data\": \"AA");
        assert!(open(&tampered, "correct horse battery").is_err());
    }
}
