use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Algorithm, Argon2, Params, Version,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;
use zeroize::Zeroize;

use super::types::{EncryptedBlob, SecurityPreset};
use crate::error::OpalError;

const MASTER_KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const SALT_LEN: usize = 16;

pub struct DerivedKey(pub [u8; MASTER_KEY_LEN]);

impl Drop for DerivedKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub struct MasterKey(pub [u8; MASTER_KEY_LEN]);

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl MasterKey {
    pub fn generate() -> Self {
        let mut key = [0u8; MASTER_KEY_LEN];
        rand::thread_rng().fill_bytes(&mut key);
        Self(key)
    }
}

pub fn generate_salt_b64() -> String {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    B64.encode(salt)
}

fn salt_string_from_b64(salt_b64: &str) -> Result<SaltString, OpalError> {
    let bytes = B64
        .decode(salt_b64)
        .map_err(|e| OpalError::Crypto(format!("bad salt encoding: {e}")))?;
    if bytes.len() != SALT_LEN {
        return Err(OpalError::Crypto(format!(
            "salt must be {SALT_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    SaltString::encode_b64(&bytes).map_err(|e| OpalError::Crypto(format!("salt encode: {e}")))
}

pub fn derive_kek(
    password: &str,
    salt_b64: &str,
    preset: SecurityPreset,
) -> Result<(DerivedKey, u32, u32, u32), OpalError> {
    let (m_cost, t_cost, p_cost) = preset.argon2_params();
    derive_kek_with_params(password, salt_b64, m_cost, t_cost, p_cost)
}

pub fn derive_kek_with_params(
    password: &str,
    salt_b64: &str,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
) -> Result<(DerivedKey, u32, u32, u32), OpalError> {
    let salt = salt_string_from_b64(salt_b64)?;
    let params = Params::new(m_cost, t_cost, p_cost, Some(MASTER_KEY_LEN))
        .map_err(|e| OpalError::Crypto(format!("invalid argon2 params: {e}")))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| OpalError::Crypto(format!("argon2 failed: {e}")))?;

    let hash = password_hash
        .hash
        .ok_or_else(|| OpalError::Crypto("argon2 produced no hash".into()))?;

    let bytes = hash.as_bytes();
    if bytes.len() < MASTER_KEY_LEN {
        return Err(OpalError::Crypto("argon2 hash too short".into()));
    }

    let mut key = [0u8; MASTER_KEY_LEN];
    key.copy_from_slice(&bytes[..MASTER_KEY_LEN]);
    Ok((DerivedKey(key), m_cost, t_cost, p_cost))
}

pub fn encrypt(key: &[u8; MASTER_KEY_LEN], plaintext: &[u8]) -> Result<EncryptedBlob, OpalError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| OpalError::Crypto(format!("encrypt failed: {e}")))?;

    Ok(EncryptedBlob {
        nonce_b64: B64.encode(nonce_bytes),
        ciphertext_b64: B64.encode(ciphertext),
    })
}

pub fn decrypt(key: &[u8; MASTER_KEY_LEN], blob: &EncryptedBlob) -> Result<Vec<u8>, OpalError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce_bytes = B64
        .decode(&blob.nonce_b64)
        .map_err(|e| OpalError::Crypto(format!("bad nonce: {e}")))?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(OpalError::Crypto("nonce must be 12 bytes".into()));
    }
    let ciphertext = B64
        .decode(&blob.ciphertext_b64)
        .map_err(|e| OpalError::Crypto(format!("bad ciphertext: {e}")))?;

    cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| OpalError::InvalidPassword)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::types::SecurityPreset;

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let key = MasterKey::generate();
        let blob = encrypt(&key.0, b"opal-secret").unwrap();
        let plain = decrypt(&key.0, &blob).unwrap();
        assert_eq!(plain, b"opal-secret");
    }

    #[test]
    fn wrong_key_fails() {
        let key = MasterKey::generate();
        let other = MasterKey::generate();
        let blob = encrypt(&key.0, b"opal-secret").unwrap();
        assert!(matches!(
            decrypt(&other.0, &blob),
            Err(OpalError::InvalidPassword)
        ));
    }

    #[test]
    fn kek_derivation_stable() {
        let salt = generate_salt_b64();
        let (a, m, t, p) = derive_kek("test-password-12", &salt, SecurityPreset::Fast).unwrap();
        let (b, m2, t2, p2) =
            derive_kek_with_params("test-password-12", &salt, m, t, p).unwrap();
        assert_eq!(a.0, b.0);
        assert_eq!((m, t, p), (m2, t2, p2));
    }
}
