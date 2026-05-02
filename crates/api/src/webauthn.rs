//! WebAuthn JSON envelope unwrapping.
//!
//! The browser hands the API edge a JSON object describing an
//! `AuthenticatorAttestationResponse` (signup) or
//! `AuthenticatorAssertionResponse` (signin). We flatten that envelope
//! into the IAS `Proof` proto and forward it to the user's home-region
//! IAS, which performs the cryptographic verification (challenge match,
//! CBOR/COSE parsing, signature verification).
//!
//! Nothing in this module is security-critical on its own — the IAS
//! re-validates everything below from raw bytes.

use base64::Engine;
use juniper::FieldResult;

use crate::field_error;
use crate::json_scalar::JsonValue;

/// Whether the proof is being verified for a registration ceremony
/// (signup / `addPublicKey`) or an assertion ceremony (signin).
pub(crate) enum ProofKind {
    Registration,
    Assertion,
}

/// Convert a GraphQL `proof` argument (string for SSH, object for
/// WebAuthn) into an `ias::proto::Proof` for forwarding to IAS.
pub(crate) fn proof_from_json(
    proof: &JsonValue,
    kind: ProofKind,
) -> FieldResult<ias::proto::Proof> {
    match &proof.0 {
        serde_json::Value::String(sig_pem) => Ok(ias::proto::Proof {
            kind: Some(ias::proto::proof::Kind::SshSignature(sig_pem.clone())),
        }),
        serde_json::Value::Object(_) => match kind {
            ProofKind::Registration => Ok(ias::proto::Proof {
                kind: Some(ias::proto::proof::Kind::Attestation(parse_attestation(
                    &proof.0,
                )?)),
            }),
            ProofKind::Assertion => Ok(ias::proto::Proof {
                kind: Some(ias::proto::proof::Kind::Assertion(parse_assertion(
                    &proof.0,
                )?)),
            }),
        },
        _ => Err(field_error(
            "Invalid proof: expected a string (SSH signature) or object (WebAuthn payload)",
        )),
    }
}

fn parse_attestation(value: &serde_json::Value) -> FieldResult<ias::proto::WebAuthnAttestation> {
    let credential_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'id' in WebAuthn attestation"))?
        .to_owned();

    let response = value
        .get("response")
        .ok_or_else(|| field_error("Missing 'response' in WebAuthn attestation"))?;

    let client_data_json = decode_b64url_field(response, "clientDataJSON")?;
    let attestation_object = decode_b64url_field(response, "attestationObject")?;

    Ok(ias::proto::WebAuthnAttestation {
        client_data_json,
        attestation_object,
        credential_id,
    })
}

fn parse_assertion(value: &serde_json::Value) -> FieldResult<ias::proto::WebAuthnAssertion> {
    let credential_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'id' in WebAuthn assertion"))?
        .to_owned();

    let response = value
        .get("response")
        .ok_or_else(|| field_error("Missing 'response' in WebAuthn assertion"))?;

    let client_data_json = decode_b64url_field(response, "clientDataJSON")?;
    let authenticator_data = decode_b64url_field(response, "authenticatorData")?;
    let signature = decode_b64url_field(response, "signature")?;

    Ok(ias::proto::WebAuthnAssertion {
        client_data_json,
        authenticator_data,
        signature,
        credential_id,
    })
}

fn decode_b64url_field(response: &serde_json::Value, field: &str) -> FieldResult<Vec<u8>> {
    let b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let raw = response
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error(&format!("Missing '{field}'")))?;
    b64url
        .decode(raw)
        .map_err(|_| field_error(&format!("Invalid base64url in '{field}'")))
}
