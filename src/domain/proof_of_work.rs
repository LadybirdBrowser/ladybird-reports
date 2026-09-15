use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    domain::ChallengeId,
    error::{AppError, Result},
};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengeClaims {
    pub version: u32,
    pub id: ChallengeId,
    pub manifest_digest: String,
    pub expires_at_unix: i64,
    pub expected_work: u64,
}

pub fn sign_challenge(key: &[u8; 32], claims: &ChallengeClaims) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    let payload = serde_json::to_vec(claims).expect("challenge claims are serializable");
    let encoded_payload = URL_SAFE_NO_PAD.encode(payload);

    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a 32-byte key");
    mac.update(encoded_payload.as_bytes());

    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{encoded_payload}.{signature}")
}

pub fn verify_challenge(key: &[u8; 32], token: &str) -> Result<ChallengeClaims> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    if token.len() > 2048 {
        return Err(AppError::InvalidRequest("Invalid challenge token"));
    }

    let (encoded_payload, encoded_signature) = token
        .split_once('.')
        .ok_or(AppError::InvalidRequest("Invalid challenge token"))?;

    let signature = URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| AppError::InvalidRequest("Invalid challenge token"))?;

    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a 32-byte key");
    mac.update(encoded_payload.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| AppError::InvalidRequest("Invalid challenge token"))?;

    let payload = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .map_err(|_| AppError::InvalidRequest("Invalid challenge token"))?;

    serde_json::from_slice(&payload)
        .map_err(|_| AppError::InvalidRequest("Invalid challenge token"))
}

pub fn proof_is_valid(token: &str, nonce: u64, expected_work: u64) -> bool {
    if expected_work == 0 {
        return false;
    }

    let seed = Sha256::digest(token.as_bytes());

    let mut input = [0_u8; 40];
    input[..32].copy_from_slice(&seed);
    input[32..].copy_from_slice(&nonce.to_le_bytes());

    let candidate = Sha256::digest(input);
    let prefix = u64::from_be_bytes(candidate[..8].try_into().expect("eight-byte prefix"));
    let target = u64::MAX / expected_work;

    prefix <= target
}

pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_signatures_detect_tampering() {
        let claims = ChallengeClaims {
            version: 1,
            id: ChallengeId::new(),
            manifest_digest: "00".repeat(32),
            expires_at_unix: 1_800_000_000,
            expected_work: 10,
        };

        let token = sign_challenge(&[7; 32], &claims);
        let verified = verify_challenge(&[7; 32], &token).expect("valid token");

        assert_eq!(verified.manifest_digest, claims.manifest_digest);
        assert!(verify_challenge(&[8; 32], &token).is_err());
        assert!(verify_challenge(&[7; 32], &(token + "x")).is_err());
    }
}
