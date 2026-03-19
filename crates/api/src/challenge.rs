use base64::Engine;
use chrono::{DateTime, Utc};
use sha2::Digest;

const CHALLENGE_FRAME_SECONDS: i64 = 60;
const CHALLENGE_NAMESPACE: &str = "skyr-auth-challenge";

pub(crate) struct Challenger {
    salt: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckSignatureError {
    InvalidSignature,
}

impl Challenger {
    pub(crate) fn new(salt: Vec<u8>) -> Self {
        Self { salt }
    }

    pub(crate) fn challenge(&self, now: DateTime<Utc>, username: &str) -> String {
        self.challenge_for_frame(Self::frame_start(now.timestamp()), username)
    }

    pub(crate) fn check(
        &self,
        public_key: &russh::keys::ssh_key::PublicKey,
        signature: &str,
        username: &str,
        now: DateTime<Utc>,
    ) -> Result<(), CheckSignatureError> {
        let signature = signature
            .parse::<russh::keys::ssh_key::SshSig>()
            .map_err(|_| CheckSignatureError::InvalidSignature)?;

        let frame_start = Self::frame_start(now.timestamp());
        for frame_offset in [-1_i64, 0, 1] {
            let frame = frame_start + (frame_offset * CHALLENGE_FRAME_SECONDS);
            let challenge = self.challenge_for_frame(frame, username);

            if public_key
                .verify(CHALLENGE_NAMESPACE, challenge.as_bytes(), &signature)
                .is_ok()
            {
                return Ok(());
            }
        }

        Err(CheckSignatureError::InvalidSignature)
    }

    fn frame_start(timestamp: i64) -> i64 {
        timestamp.div_euclid(CHALLENGE_FRAME_SECONDS) * CHALLENGE_FRAME_SECONDS
    }

    fn challenge_for_frame(&self, frame_start: i64, username: &str) -> String {
        let mut hasher = sha2::Sha256::new();
        hasher.update(frame_start.to_be_bytes());
        hasher.update(b":");
        hasher.update(username.as_bytes());
        hasher.update(b":");
        hasher.update(&self.salt);

        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
    }
}
