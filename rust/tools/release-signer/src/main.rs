//! Signs a Kestrel release's update feed, so installed copies (Flux 1.x and Kestrel 2.x) accept it.
//!
//!   release-signer sign <private-key-file> <installer.exe> <https-download-url> <version> <version.json>
//!
//! The private key file is the one Flux 1.x's UpdateSigner made: a PKCS#8 P-256 key protected with
//! DPAPI for this Windows account (default %LOCALAPPDATA%\ApexRelease\update-signing.key). It never
//! leaves this computer and is never printed. The feed is checked with the app's own verifier and
//! the public key built into the app before it is written.

use base64::Engine;
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::pkcs8::DecodePrivateKey;
use sha2::{Digest, Sha256};

fn fail(message: &str, code: i32) -> ! {
    eprintln!("{message}");
    std::process::exit(code);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [command, key_file, installer, url, version, output] = args.as_slice() else {
        fail("Usage: release-signer sign <private-key-file> <installer.exe> <https-download-url> <version> <version.json>", 1)
    };
    if command != "sign" {
        fail("The only command is: sign", 1);
    }
    if flux_update::parse_version(version).is_none() || !url.starts_with("https://") {
        fail("Use a numeric version (2.0.0) and an HTTPS download URL.", 4);
    }
    let sealed = std::fs::read(key_file).unwrap_or_else(|e| fail(&format!("Cannot read the key file: {e}"), 2));
    // Flux 1.x protected it without extra entropy.
    let pkcs8 = flux_store::secret::unprotect(&sealed, &[]).unwrap_or_else(|_| fail("The key file cannot be opened with this Windows account.", 2));
    let key = SigningKey::from_pkcs8_der(&pkcs8).unwrap_or_else(|_| fail("The key file does not hold a P-256 private key.", 2));
    let bytes = std::fs::read(installer).unwrap_or_else(|e| fail(&format!("Cannot read the installer: {e}"), 3));
    let sha: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    let signature: Signature = key.sign(&flux_update::payload(version, url, &sha));
    let feed = serde_json::json!({
        "version": version,
        "url": url,
        "sha256": sha,
        "signature": base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
    });
    let text = serde_json::to_string_pretty(&feed).expect("json");
    // The same check the app makes, with the public key it has built in.
    match flux_update::parse(text.as_bytes(), flux_update::PUBLIC_KEY) {
        Ok(release) if release.signed => {}
        _ => fail("Signature self-check failed: this key is not the release key built into Kestrel.", 5),
    }
    std::fs::write(output, text).unwrap_or_else(|e| fail(&format!("Cannot write {output}: {e}"), 6));
    println!("Signed {version}: {output} (sha256 {sha})");
}
