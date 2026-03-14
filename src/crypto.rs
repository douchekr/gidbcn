use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{bail, Context, Result};
use rand::RngCore;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const PBKDF2_ITERATIONS: u32 = 100_000;

/// 패스프레이즈에서 AES-256 키 유도 (PBKDF2-HMAC-SHA256)
fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
        passphrase.as_bytes(),
        salt,
        PBKDF2_ITERATIONS,
        &mut key,
    );
    key
}

/// 평문을 AES-256-GCM으로 암호화
/// 반환: salt(16) + nonce(12) + ciphertext
pub fn encrypt(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    let key = derive_key(passphrase, &salt);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("cipher 초기화 실패: {e}"))?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("암호화 실패: {e}"))?;

    let mut result = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    result.extend_from_slice(&salt);
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// AES-256-GCM 복호화
/// 입력: salt(16) + nonce(12) + ciphertext
pub fn decrypt(data: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if data.len() < SALT_LEN + NONCE_LEN + 1 {
        bail!("암호화 데이터가 너무 짧습니다");
    }

    let salt = &data[..SALT_LEN];
    let nonce_bytes = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &data[SALT_LEN + NONCE_LEN..];

    let key = derive_key(passphrase, salt);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("cipher 초기화 실패: {e}"))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("복호화 실패 — 패스프레이즈가 틀렸습니다"))
}

/// 평문 JSON → base64 암호문
pub fn encrypt_to_base64(json: &str, passphrase: &str) -> Result<String> {
    let encrypted = encrypt(json.as_bytes(), passphrase)?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &encrypted,
    ))
}

/// base64 암호문 → 평문 JSON
pub fn decrypt_from_base64(b64: &str, passphrase: &str) -> Result<String> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("base64 디코딩 실패")?;
    let plaintext = decrypt(&data, passphrase)?;
    String::from_utf8(plaintext).context("UTF-8 변환 실패")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"hello, secrets!";
        let passphrase = "my-strong-passphrase";
        let encrypted = encrypt(plaintext, passphrase).unwrap();
        let decrypted = decrypt(&encrypted, passphrase).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let plaintext = b"secret data";
        let encrypted = encrypt(plaintext, "correct").unwrap();
        assert!(decrypt(&encrypted, "wrong").is_err());
    }

    #[test]
    fn base64_roundtrip() {
        let json = r#"{"app_key":"test123","app_secret":"secret456"}"#;
        let passphrase = "p@ssw0rd";
        let b64 = encrypt_to_base64(json, passphrase).unwrap();
        let decrypted = decrypt_from_base64(&b64, passphrase).unwrap();
        assert_eq!(decrypted, json);
    }

    #[test]
    fn different_encryptions_differ() {
        let plaintext = b"same data";
        let pass = "same-pass";
        let enc1 = encrypt(plaintext, pass).unwrap();
        let enc2 = encrypt(plaintext, pass).unwrap();
        // salt/nonce가 다르므로 암호문도 달라야 함
        assert_ne!(enc1, enc2);
        // 하지만 둘 다 복호화 가능
        assert_eq!(decrypt(&enc1, pass).unwrap(), plaintext);
        assert_eq!(decrypt(&enc2, pass).unwrap(), plaintext);
    }
}
