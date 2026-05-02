//! WebAuthn parsing and signature verification.
//!
//! The API edge unwraps the WebAuthn JSON envelope returned by the
//! browser (base64url decoding of `clientDataJSON`, `attestationObject`,
//! `authenticatorData`, `signature`) and forwards the raw bytes here.
//! Everything below — JSON parsing of `clientDataJSON`, CBOR parsing of
//! `attestationObject`, signature verification — runs inside IAS.

use sha2::Digest;

#[derive(Debug)]
pub enum WebAuthnError {
    InvalidClientData(String),
    InvalidAttestationObject(String),
    InvalidAuthenticatorData(String),
    InvalidSignature(String),
    UnsupportedAlgorithm(String),
}

impl std::fmt::Display for WebAuthnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidClientData(msg) => write!(f, "invalid client data: {msg}"),
            Self::InvalidAttestationObject(msg) => {
                write!(f, "invalid attestation object: {msg}")
            }
            Self::InvalidAuthenticatorData(msg) => {
                write!(f, "invalid authenticator data: {msg}")
            }
            Self::InvalidSignature(msg) => write!(f, "invalid signature: {msg}"),
            Self::UnsupportedAlgorithm(msg) => write!(f, "unsupported algorithm: {msg}"),
        }
    }
}

pub struct ClientData {
    pub challenge: String,
}

pub struct AttestationData {
    pub cose_key: Vec<u8>,
    pub sign_count: u32,
    pub flags: u8,
}

pub struct AuthenticatorData {
    pub sign_count: u32,
    pub flags: u8,
}

/// Parse `clientDataJSON`, verify the `type` field and extract the
/// `challenge`. The challenge in `clientDataJSON` is base64url-encoded
/// per the WebAuthn spec; the caller is responsible for decoding it back
/// to the underlying challenge string.
pub fn parse_client_data(
    client_data_json: &[u8],
    expected_type: &str,
) -> Result<ClientData, WebAuthnError> {
    let value: serde_json::Value = serde_json::from_slice(client_data_json)
        .map_err(|e| WebAuthnError::InvalidClientData(format!("JSON parse error: {e}")))?;

    let typ = value
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WebAuthnError::InvalidClientData("missing 'type' field".into()))?;

    if typ != expected_type {
        return Err(WebAuthnError::InvalidClientData(format!(
            "expected type '{expected_type}', got '{typ}'"
        )));
    }

    let challenge = value
        .get("challenge")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WebAuthnError::InvalidClientData("missing 'challenge' field".into()))?
        .to_owned();

    Ok(ClientData { challenge })
}

/// Parse an attestation object (CBOR), extract `authData`, parse the
/// attested-credential portion to recover the COSE public key.
pub fn parse_attestation_object(
    attestation_object: &[u8],
) -> Result<AttestationData, WebAuthnError> {
    let value: ciborium::Value = ciborium::from_reader(attestation_object)
        .map_err(|e| WebAuthnError::InvalidAttestationObject(format!("CBOR parse error: {e}")))?;

    let map = match &value {
        ciborium::Value::Map(m) => m,
        _ => {
            return Err(WebAuthnError::InvalidAttestationObject(
                "expected CBOR map".into(),
            ));
        }
    };

    let auth_data_bytes = map
        .iter()
        .find_map(|(k, v)| {
            if k.as_text() == Some("authData") {
                v.as_bytes()
            } else {
                None
            }
        })
        .ok_or_else(|| {
            WebAuthnError::InvalidAttestationObject("missing 'authData' field".into())
        })?;

    parse_attestation_auth_data(auth_data_bytes)
}

fn parse_attestation_auth_data(auth_data: &[u8]) -> Result<AttestationData, WebAuthnError> {
    // authData layout:
    // 32 bytes rpIdHash
    // 1  byte  flags
    // 4  bytes signCount
    // attested credential data (if AT flag is set):
    //   16 bytes AAGUID
    //   2  bytes credentialIdLength (big-endian)
    //   N  bytes credentialId
    //   remainder: COSE public key
    if auth_data.len() < 37 {
        return Err(WebAuthnError::InvalidAuthenticatorData(
            "authData too short".into(),
        ));
    }

    let flags = auth_data[32];
    let sign_count =
        u32::from_be_bytes([auth_data[33], auth_data[34], auth_data[35], auth_data[36]]);

    if flags & 0x40 == 0 {
        return Err(WebAuthnError::InvalidAttestationObject(
            "AT flag not set in authData".into(),
        ));
    }

    let rest = &auth_data[37..];
    if rest.len() < 18 {
        return Err(WebAuthnError::InvalidAuthenticatorData(
            "authData too short for attested credential data".into(),
        ));
    }

    let cred_id_len = u16::from_be_bytes([rest[16], rest[17]]) as usize;
    let cred_start = 18;
    let cred_end = cred_start + cred_id_len;
    if rest.len() < cred_end {
        return Err(WebAuthnError::InvalidAuthenticatorData(
            "authData too short for credential ID".into(),
        ));
    }

    let cose_key = rest[cred_end..].to_vec();
    if cose_key.is_empty() {
        return Err(WebAuthnError::InvalidAuthenticatorData(
            "missing COSE public key".into(),
        ));
    }

    Ok(AttestationData {
        cose_key,
        sign_count,
        flags,
    })
}

/// Parse `authenticatorData` from an assertion response: just the flags
/// and signCount; the attested-credential portion isn't present on
/// assertions.
pub fn parse_authenticator_data(auth_data: &[u8]) -> Result<AuthenticatorData, WebAuthnError> {
    if auth_data.len() < 37 {
        return Err(WebAuthnError::InvalidAuthenticatorData(
            "authData too short".into(),
        ));
    }

    let flags = auth_data[32];
    let sign_count =
        u32::from_be_bytes([auth_data[33], auth_data[34], auth_data[35], auth_data[36]]);

    Ok(AuthenticatorData { sign_count, flags })
}

/// Verify an ES256 (ECDSA P-256) signature over `message`. The public key
/// is in OpenSSH format and the signature is DER-encoded.
pub fn verify_es256(
    public_key_openssh: &str,
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<(), WebAuthnError> {
    use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};

    let ssh_pubkey = ssh_key::PublicKey::from_openssh(public_key_openssh)
        .map_err(|e| WebAuthnError::InvalidSignature(format!("invalid SSH public key: {e}")))?;

    let key_data = match ssh_pubkey.key_data() {
        ssh_key::public::KeyData::Ecdsa(ecdsa_key) => ecdsa_key,
        _ => {
            return Err(WebAuthnError::UnsupportedAlgorithm(
                "expected ECDSA key for ES256 verification".into(),
            ));
        }
    };

    let point_bytes = key_data.as_ref();
    let verifying_key = VerifyingKey::from_sec1_bytes(point_bytes)
        .map_err(|e| WebAuthnError::InvalidSignature(format!("invalid EC point: {e}")))?;

    let sig = Signature::from_der(signature_bytes)
        .map_err(|e| WebAuthnError::InvalidSignature(format!("invalid DER signature: {e}")))?;

    verifying_key
        .verify(message, &sig)
        .map_err(|e| WebAuthnError::InvalidSignature(format!("ES256 verification failed: {e}")))
}

/// Verify an Ed25519 signature over `message`. The public key is in
/// OpenSSH format.
pub fn verify_ed25519(
    public_key_openssh: &str,
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<(), WebAuthnError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let ssh_pubkey = ssh_key::PublicKey::from_openssh(public_key_openssh)
        .map_err(|e| WebAuthnError::InvalidSignature(format!("invalid SSH public key: {e}")))?;

    let key_data = match ssh_pubkey.key_data() {
        ssh_key::public::KeyData::Ed25519(ed_key) => ed_key,
        _ => {
            return Err(WebAuthnError::UnsupportedAlgorithm(
                "expected Ed25519 key for EdDSA verification".into(),
            ));
        }
    };

    let key_bytes: [u8; 32] = *key_data.as_ref();

    let verifying_key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| WebAuthnError::InvalidSignature(format!("invalid Ed25519 key: {e}")))?;

    let sig_bytes: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| WebAuthnError::InvalidSignature("Ed25519 signature not 64 bytes".into()))?;
    let sig = Signature::from_bytes(&sig_bytes);

    verifying_key
        .verify(message, &sig)
        .map_err(|e| WebAuthnError::InvalidSignature(format!("Ed25519 verification failed: {e}")))
}

/// Build the verification message for a WebAuthn assertion:
/// `authenticatorData || SHA256(clientDataJSON)`.
pub fn build_assertion_message(authenticator_data: &[u8], client_data_json: &[u8]) -> Vec<u8> {
    let client_data_hash = sha2::Sha256::digest(client_data_json);
    let mut message = Vec::with_capacity(authenticator_data.len() + 32);
    message.extend_from_slice(authenticator_data);
    message.extend_from_slice(&client_data_hash);
    message
}
