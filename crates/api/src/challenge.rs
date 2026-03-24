use base64::Engine;
use chrono::{DateTime, Utc};
use sha2::Digest;

const CHALLENGE_FRAME_SECONDS: i64 = 60;
pub(crate) const CHALLENGE_NAMESPACE: &str = "skyr-auth-challenge";

pub(crate) struct Challenger {
    salt: Vec<u8>,
}

impl Challenger {
    pub(crate) fn new(salt: Vec<u8>) -> Self {
        Self { salt }
    }

    pub(crate) fn challenge(&self, now: DateTime<Utc>, username: &str) -> String {
        self.challenge_for_frame(Self::frame_start(now.timestamp()), username)
    }

    pub(crate) fn valid_challenges(&self, now: DateTime<Utc>, username: &str) -> Vec<String> {
        let frame_start = Self::frame_start(now.timestamp());
        (-1..=1)
            .map(|offset| {
                self.challenge_for_frame(frame_start + (offset * CHALLENGE_FRAME_SECONDS), username)
            })
            .collect()
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
