//! Signs a Kestrel release's update feed, so installed copies (Flux 1.x and Kestrel 2.x) accept it.
//!
//!   release-signer new-key <private-key-file>
//!   release-signer export-backup <private-key-file> <backup-file>   (asks for a password)
//!   release-signer import-backup <backup-file> <private-key-file>   (asks for the password)
//!   release-signer sign <private-key-file> <installer.exe> <https-download-url> <version> <version.json>
//!
//! The private key is a PKCS#8 P-256 key protected with DPAPI for this Windows account (default
//! %LOCALAPPDATA%\KestrelRelease\update-signing-2.key). It never leaves this computer and is never
//! printed; new-key prints only the public key, to build into the app. The feed's signature goes in
//! "signature2" (read by Kestrel 2.1 and later; older copies see an unsigned release and offer its
//! download page), and it is checked with the app's own verifier before it is written.

mod backup;

use base64::Engine;
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::pkcs8::DecodePrivateKey;
use sha2::{Digest, Sha256};

fn fail(message: &str, code: i32) -> ! {
    eprintln!("{message}");
    std::process::exit(code);
}

fn new_key(path: &str) {
    if std::path::Path::new(path).exists() {
        fail("A key file is already there; it is never overwritten.", 7);
    }
    use p256::pkcs8::{EncodePrivateKey, EncodePublicKey};
    let key = SigningKey::random(&mut rand_core::OsRng);
    let pkcs8 = key.to_pkcs8_der().unwrap_or_else(|_| fail("The key could not be encoded.", 8));
    let sealed = kestrel_store::secret::protect(pkcs8.as_bytes(), &[]).unwrap_or_else(|_| fail("The key could not be protected for this Windows account.", 8));
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, sealed).unwrap_or_else(|e| fail(&format!("Cannot write {path}: {e}"), 6));
    let public = key.verifying_key().to_public_key_der().unwrap_or_else(|_| fail("The public key could not be encoded.", 8));
    println!("{}", base64::engine::general_purpose::STANDARD.encode(public.as_bytes()));
}

fn public_of(pkcs8: &[u8]) -> String {
    use p256::pkcs8::EncodePublicKey;
    let key = SigningKey::from_pkcs8_der(pkcs8).unwrap_or_else(|_| fail("This is not a P-256 private key.", 2));
    let public = key.verifying_key().to_public_key_der().unwrap_or_else(|_| fail("The public key could not be encoded.", 8));
    base64::engine::general_purpose::STANDARD.encode(public.as_bytes())
}

fn report(public: &str) {
    if public == kestrel_update::PUBLIC_KEY_2 {
        println!("This is the release key built into Kestrel 2.1 and later.");
    } else {
        println!("Warning: this is not the release key built into Kestrel (public key {public}).");
    }
}

/// Typed by the user, hidden; never taken from the command line.
fn ask(prompt: &str) -> String {
    rpassword::prompt_password(prompt).unwrap_or_else(|_| fail("The password could not be read (run this in a terminal).", 9))
}

fn export_backup(key_file: &str, out: &str) {
    if std::path::Path::new(out).exists() {
        fail("The backup file is already there; choose another name.", 7);
    }
    let sealed = std::fs::read(key_file).unwrap_or_else(|e| fail(&format!("Cannot read the key file: {e}"), 2));
    let pkcs8 = kestrel_store::secret::unprotect(&sealed, &[]).unwrap_or_else(|_| fail("The key file cannot be opened with this Windows account.", 2));
    println!("Choose the backup's password: at least {} characters. Keep it somewhere else than the backup file.", backup::MIN_PASSWORD);
    let password = ask("Password: ");
    if ask("Same password again: ") != password {
        fail("The two passwords differ; nothing was written.", 9);
    }
    let text = backup::seal(&pkcs8, &password).unwrap_or_else(|e| fail(&e, 9));
    // Checked before it is written: the file opens with this password and gives back this key.
    if backup::open(&text, &password).ok().as_deref() != Some(pkcs8.as_slice()) {
        fail("The backup did not check out; nothing was written.", 5);
    }
    std::fs::write(out, text).unwrap_or_else(|e| fail(&format!("Cannot write {out}: {e}"), 6));
    println!("Backup written: {out}");
    report(&public_of(&pkcs8));
}

fn import_backup(input: &str, key_file: &str) {
    if std::path::Path::new(key_file).exists() {
        fail("A key file is already there; it is never overwritten.", 7);
    }
    let text = std::fs::read_to_string(input).unwrap_or_else(|e| fail(&format!("Cannot read {input}: {e}"), 2));
    let pkcs8 = backup::open(&text, &ask("Backup password: ")).unwrap_or_else(|e| fail(&e, 9));
    let public = public_of(&pkcs8);
    let sealed = kestrel_store::secret::protect(&pkcs8, &[]).unwrap_or_else(|_| fail("The key could not be protected for this Windows account.", 8));
    if let Some(dir) = std::path::Path::new(key_file).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(key_file, sealed).unwrap_or_else(|e| fail(&format!("Cannot write {key_file}: {e}"), 6));
    println!("Key restored: {key_file}");
    report(&public);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, key_file] if command == "new-key" => return new_key(key_file),
        [command, key_file, out] if command == "export-backup" => return export_backup(key_file, out),
        [command, input, key_file] if command == "import-backup" => return import_backup(input, key_file),
        _ => {}
    }
    let [command, key_file, installer, url, version, output] = args.as_slice() else {
        fail("Usage: release-signer sign <private-key-file> <installer.exe> <https-download-url> <version> <version.json>", 1)
    };
    if command != "sign" {
        fail("The only command is: sign", 1);
    }
    if kestrel_update::parse_version(version).is_none() || !url.starts_with("https://") {
        fail("Use a numeric version (2.0.0) and an HTTPS download URL.", 4);
    }
    let sealed = std::fs::read(key_file).unwrap_or_else(|e| fail(&format!("Cannot read the key file: {e}"), 2));
    // Flux 1.x protected it without extra entropy.
    let pkcs8 = kestrel_store::secret::unprotect(&sealed, &[]).unwrap_or_else(|_| fail("The key file cannot be opened with this Windows account.", 2));
    let key = SigningKey::from_pkcs8_der(&pkcs8).unwrap_or_else(|_| fail("The key file does not hold a P-256 private key.", 2));
    let bytes = std::fs::read(installer).unwrap_or_else(|e| fail(&format!("Cannot read the installer: {e}"), 3));
    let sha: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    let signature: Signature = key.sign(&kestrel_update::payload(version, url, &sha));
    let feed = serde_json::json!({
        "version": version,
        "url": url,
        "sha256": sha,
        "signature2": base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
    });
    let text = serde_json::to_string_pretty(&feed).expect("json");
    // The same check the app makes, with the public key it has built in.
    match kestrel_update::parse_feed(text.as_bytes()) {
        Ok(release) if release.signed => {}
        _ => fail("Signature self-check failed: this key is not the release key built into Kestrel.", 5),
    }
    std::fs::write(output, text).unwrap_or_else(|e| fail(&format!("Cannot write {output}: {e}"), 6));
    println!("Signed {version}: {output} (sha256 {sha})");
}
