//! Signed identity tokens for cross-region authentication.
//!
//! See the crate README for the wire format and the architectural role of
//! these tokens. The public surface is intentionally minimal: [`issue`],
//! [`parse`], and [`UnverifiedToken::verify`].

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use ids::{ParseIdError, RegionId};
use thiserror::Error;

/// Wire-format version. Bumped on any incompatible change to [`encode`].
const FORMAT_VERSION: u8 = 1;

/// Length of the issuer-supplied nonce field.
pub const NONCE_LEN: usize = 16;

/// The set of facts an identity token attests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    pub username: String,
    pub issuer_region: RegionId,
    /// Unix seconds when the token was minted. Informational; not enforced.
    pub issued_at: i64,
    /// Unix seconds at which the token stops being valid (exclusive).
    pub expires_at: i64,
    /// Issuer-chosen randomness, present so two tokens with otherwise
    /// identical claims have distinct signatures.
    pub nonce: [u8; NONCE_LEN],
}

/// Sign `claims` with `signing_key` and produce the wire-format token.
pub fn issue(signing_key: &SigningKey, claims: &Claims) -> String {
    let payload = encode(claims);
    let signature = signing_key.sign(&payload);
    let payload_b64 = URL_SAFE_NO_PAD.encode(&payload);
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    format!("{payload_b64}.{signature_b64}")
}

/// Decode a token without verifying its signature.
///
/// The returned [`UnverifiedToken`] exposes the issuer region so the caller
/// can resolve the right public key, then call
/// [`UnverifiedToken::verify`] to finalize the check.
pub fn parse(token: &str) -> Result<UnverifiedToken, VerifyError> {
    let (payload_b64, signature_b64) = token.split_once('.').ok_or(VerifyError::Malformed)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| VerifyError::Malformed)?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|_| VerifyError::Malformed)?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|_| VerifyError::Malformed)?;
    let claims = decode(&payload)?;
    Ok(UnverifiedToken {
        payload,
        signature,
        claims,
    })
}

/// A token whose structural decoding succeeded but whose signature has not
/// yet been checked. Holding one of these is *not* proof of anything — call
/// [`UnverifiedToken::verify`] to extract the [`Claims`].
#[derive(Debug)]
pub struct UnverifiedToken {
    payload: Vec<u8>,
    signature: Signature,
    claims: Claims,
}

impl UnverifiedToken {
    /// The issuer region declared inside the (unverified) payload.
    ///
    /// Used to look up which public key to verify against. Do not trust any
    /// other field of the token until [`Self::verify`] has succeeded.
    pub fn issuer_region(&self) -> &RegionId {
        &self.claims.issuer_region
    }

    /// Verify the signature against `verifying_key` and check expiry against
    /// the system clock. On success, returns the trusted [`Claims`].
    pub fn verify(self, verifying_key: &VerifyingKey) -> Result<Claims, VerifyError> {
        self.verify_at(verifying_key, unix_now())
    }

    /// As [`Self::verify`], but with `now_unix` supplied explicitly. Useful
    /// for tests and for callers that have already computed the time.
    pub fn verify_at(
        self,
        verifying_key: &VerifyingKey,
        now_unix: i64,
    ) -> Result<Claims, VerifyError> {
        verifying_key
            .verify(&self.payload, &self.signature)
            .map_err(|_| VerifyError::BadSignature)?;
        if self.claims.expires_at <= now_unix {
            return Err(VerifyError::Expired);
        }
        Ok(self.claims)
    }
}

#[derive(Error, Debug)]
pub enum VerifyError {
    #[error("token is malformed")]
    Malformed,

    #[error("unsupported token version: {0}")]
    UnsupportedVersion(u8),

    #[error("token signature is invalid")]
    BadSignature,

    #[error("token has expired")]
    Expired,

    #[error("invalid issuer region: {0}")]
    InvalidIssuerRegion(#[from] ParseIdError),
}

fn encode(claims: &Claims) -> Vec<u8> {
    let username = claims.username.as_bytes();
    let region = claims.issuer_region.as_str().as_bytes();
    assert!(
        username.len() <= u8::MAX as usize,
        "username too long for token format",
    );
    assert!(
        region.len() <= u8::MAX as usize,
        "region too long for token format",
    );

    let mut out = Vec::with_capacity(1 + 1 + username.len() + 1 + region.len() + 8 + 8 + NONCE_LEN);
    out.push(FORMAT_VERSION);
    out.push(username.len() as u8);
    out.extend_from_slice(username);
    out.push(region.len() as u8);
    out.extend_from_slice(region);
    out.extend_from_slice(&claims.issued_at.to_be_bytes());
    out.extend_from_slice(&claims.expires_at.to_be_bytes());
    out.extend_from_slice(&claims.nonce);
    out
}

fn decode(payload: &[u8]) -> Result<Claims, VerifyError> {
    let mut cur = 0;
    let version = read_u8(payload, &mut cur)?;
    if version != FORMAT_VERSION {
        return Err(VerifyError::UnsupportedVersion(version));
    }
    let username = read_string(payload, &mut cur)?;
    let region = read_string(payload, &mut cur)?;
    let issued_at = read_i64(payload, &mut cur)?;
    let expires_at = read_i64(payload, &mut cur)?;
    let nonce = read_array::<NONCE_LEN>(payload, &mut cur)?;
    if cur != payload.len() {
        return Err(VerifyError::Malformed);
    }
    Ok(Claims {
        username,
        issuer_region: region.parse()?,
        issued_at,
        expires_at,
        nonce,
    })
}

fn read_u8(buf: &[u8], cur: &mut usize) -> Result<u8, VerifyError> {
    if *cur >= buf.len() {
        return Err(VerifyError::Malformed);
    }
    let v = buf[*cur];
    *cur += 1;
    Ok(v)
}

fn read_string(buf: &[u8], cur: &mut usize) -> Result<String, VerifyError> {
    let len = read_u8(buf, cur)? as usize;
    if *cur + len > buf.len() {
        return Err(VerifyError::Malformed);
    }
    let s = std::str::from_utf8(&buf[*cur..*cur + len])
        .map_err(|_| VerifyError::Malformed)?
        .to_owned();
    *cur += len;
    Ok(s)
}

fn read_i64(buf: &[u8], cur: &mut usize) -> Result<i64, VerifyError> {
    if *cur + 8 > buf.len() {
        return Err(VerifyError::Malformed);
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buf[*cur..*cur + 8]);
    *cur += 8;
    Ok(i64::from_be_bytes(bytes))
}

fn read_array<const N: usize>(buf: &[u8], cur: &mut usize) -> Result<[u8; N], VerifyError> {
    if *cur + N > buf.len() {
        return Err(VerifyError::Malformed);
    }
    let mut bytes = [0u8; N];
    bytes.copy_from_slice(&buf[*cur..*cur + N]);
    *cur += N;
    Ok(bytes)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn sample_claims(now_unix: i64) -> Claims {
        Claims {
            username: "alice".to_string(),
            issuer_region: "stockholm".parse().unwrap(),
            issued_at: now_unix,
            expires_at: now_unix + 3600,
            nonce: [42; NONCE_LEN],
        }
    }

    #[test]
    fn round_trip_succeeds() {
        let key = fixture_signing_key(1);
        let now = 1_000_000_000;
        let claims = sample_claims(now);
        let token = issue(&key, &claims);

        let parsed = parse(&token).unwrap();
        assert_eq!(parsed.issuer_region(), &claims.issuer_region);

        let verified = parsed.verify_at(&key.verifying_key(), now + 1).unwrap();
        assert_eq!(verified, claims);
    }

    #[test]
    fn rejects_signature_under_wrong_key() {
        let issuer = fixture_signing_key(1);
        let other = fixture_signing_key(2);
        let now = 1_000_000_000;
        let claims = sample_claims(now);
        let token = issue(&issuer, &claims);

        let parsed = parse(&token).unwrap();
        let err = parsed
            .verify_at(&other.verifying_key(), now + 1)
            .unwrap_err();
        assert!(matches!(err, VerifyError::BadSignature));
    }

    #[test]
    fn rejects_tampered_payload() {
        let key = fixture_signing_key(1);
        let now = 1_000_000_000;
        let claims = sample_claims(now);
        let token = issue(&key, &claims);

        // Flip a bit inside the encoded payload (before the dot).
        let dot = token.find('.').unwrap();
        let mut bytes = token.into_bytes();
        bytes[dot - 1] = if bytes[dot - 1] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();

        // Parse may succeed (still valid base64 + struct) or fail; either way
        // the token must not verify under the original key.
        match parse(&tampered) {
            Err(_) => {}
            Ok(parsed) => {
                let err = parsed.verify_at(&key.verifying_key(), now + 1).unwrap_err();
                assert!(matches!(err, VerifyError::BadSignature));
            }
        }
    }

    #[test]
    fn rejects_expired_token() {
        let key = fixture_signing_key(1);
        let now = 1_000_000_000;
        let claims = sample_claims(now);
        let token = issue(&key, &claims);

        let parsed = parse(&token).unwrap();
        let err = parsed
            .verify_at(&key.verifying_key(), claims.expires_at)
            .unwrap_err();
        assert!(matches!(err, VerifyError::Expired));

        let parsed = parse(&token).unwrap();
        let err = parsed
            .verify_at(&key.verifying_key(), claims.expires_at + 1)
            .unwrap_err();
        assert!(matches!(err, VerifyError::Expired));
    }

    #[test]
    fn rejects_garbage_input() {
        assert!(matches!(parse("not a token"), Err(VerifyError::Malformed)));
        assert!(matches!(parse("a.b"), Err(VerifyError::Malformed)));
        assert!(matches!(parse(""), Err(VerifyError::Malformed)));
    }

    #[test]
    fn rejects_unsupported_version() {
        let key = fixture_signing_key(1);
        // Hand-build a payload with a wrong version byte but otherwise
        // structurally valid layout, then sign it so the signature is good.
        let mut payload = Vec::new();
        payload.push(99); // wrong version
        payload.push(5);
        payload.extend_from_slice(b"alice");
        payload.push(9);
        payload.extend_from_slice(b"stockholm");
        payload.extend_from_slice(&0i64.to_be_bytes());
        payload.extend_from_slice(&i64::MAX.to_be_bytes());
        payload.extend_from_slice(&[0u8; NONCE_LEN]);
        let signature = key.sign(&payload).to_bytes();
        let token = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&payload),
            URL_SAFE_NO_PAD.encode(signature),
        );

        let err = parse(&token).unwrap_err();
        assert!(matches!(err, VerifyError::UnsupportedVersion(99)));
    }

    #[test]
    fn issuer_region_visible_before_verify() {
        let key = fixture_signing_key(1);
        let now = 1_000_000_000;
        let claims = sample_claims(now);
        let token = issue(&key, &claims);

        let parsed = parse(&token).unwrap();
        // Confirm we can read the issuer region without verifying — the API
        // depends on this so the caller can look up the right public key.
        assert_eq!(parsed.issuer_region().as_str(), "stockholm");
    }
}
