use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::{Aead, OsRng, rand_core::RngCore},
};

use crate::error::{AppError, Result};

pub fn read_secret(name: &str) -> Result<[u8; 32]> {
    let encoded = std::env::var(name)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("{name} is required")))?;

    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("{name} must be base64")))?;

    bytes
        .try_into()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("{name} must contain 32 bytes")))
}

pub fn hash_secret(value: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(value.as_ref()))
}

pub fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Clone)]
pub struct SecretCipher {
    key: [u8; 32],
}

impl SecretCipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        let mut nonce = [0_u8; 12];
        OsRng.fill_bytes(&mut nonce);

        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
            .map_err(|_| AppError::Unavailable)?;

        let mut encoded = Vec::with_capacity(nonce.len() + ciphertext.len());
        encoded.extend_from_slice(&nonce);
        encoded.extend_from_slice(&ciphertext);

        Ok(URL_SAFE_NO_PAD.encode(encoded))
    }

    pub fn decrypt(&self, encoded: &str) -> Result<String> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| AppError::Unavailable)?;

        if bytes.len() < 12 {
            return Err(AppError::Unavailable);
        }

        let (nonce, ciphertext) = bytes.split_at(12);
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| AppError::Unavailable)?;

        String::from_utf8(plaintext).map_err(|_| AppError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_secrets_are_authenticated() {
        let cipher = SecretCipher::new([7; 32]);
        let encrypted = cipher.encrypt("secret").expect("encrypt secret");

        assert_eq!(
            cipher.decrypt(&encrypted).expect("decrypt secret"),
            "secret"
        );
        assert!(SecretCipher::new([8; 32]).decrypt(&encrypted).is_err());
    }
}
