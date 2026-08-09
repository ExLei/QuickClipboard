use serde::{Deserialize, Serialize};

use super::{DEFAULT_PAIRING_CODE_TTL_SECS, DEFAULT_PAIRING_MAX_ATTEMPTS};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingChallenge {
    pub pairing_code: String,
    pub expires_at_ms: i64,
    pub max_attempts: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingConfirmResponse {
    pub peer_token: String,
    pub expires_at_ms: Option<i64>,
}

pub fn create_pairing_challenge() -> PairingChallenge {
    PairingChallenge {
        pairing_code: format!("{:06}", fastrand::u32(0..1_000_000)),
        expires_at_ms: now_ms().saturating_add((DEFAULT_PAIRING_CODE_TTL_SECS as i64) * 1000),
        max_attempts: DEFAULT_PAIRING_MAX_ATTEMPTS,
    }
}

pub fn create_peer_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_pairing_challenge_code_is_six_digits() {
        for _ in 0..200 {
            let challenge = create_pairing_challenge();
            assert_eq!(challenge.pairing_code.len(), 6, "配对码恒为 6 位");
            assert!(challenge.pairing_code.bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn create_pairing_challenge_expiry_is_exactly_ttl() {
        let before = now_ms();
        let challenge = create_pairing_challenge();
        let after = now_ms();
        // expires_at_ms = 创建时刻 + 300s；双侧边界断言
        assert!(challenge.expires_at_ms >= before + (DEFAULT_PAIRING_CODE_TTL_SECS as i64) * 1000);
        assert!(challenge.expires_at_ms <= after + (DEFAULT_PAIRING_CODE_TTL_SECS as i64) * 1000);
    }

    #[test]
    fn create_pairing_challenge_max_attempts_is_default() {
        let challenge = create_pairing_challenge();
        assert_eq!(challenge.max_attempts, DEFAULT_PAIRING_MAX_ATTEMPTS);
        assert_eq!(DEFAULT_PAIRING_MAX_ATTEMPTS, 5);
        assert_eq!(DEFAULT_PAIRING_CODE_TTL_SECS, 300);
    }

    #[test]
    fn create_peer_token_is_uuid_v4() {
        let token = create_peer_token();
        assert_eq!(token.len(), 36);
        assert_eq!(&token[14..15], "4");
        assert_eq!(&token[8..9], "-");
        assert_eq!(&token[13..14], "-");
        assert_eq!(&token[18..19], "-");
        assert_eq!(&token[23..24], "-");
    }

    #[test]
    fn peer_tokens_are_unique() {
        let a = create_peer_token();
        let b = create_peer_token();
        assert_ne!(a, b);
    }
}
