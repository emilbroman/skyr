use base64::Engine;
use chrono::Utc;
use juniper::FieldResult;

use crate::challenge;
use crate::webauthn;
use crate::{field_error, internal_error, Context, JsonValue};

/// Dispatch proof verification: SSH (string) or WebAuthn (object).
/// Returns (openssh_key, credential_id, sign_count).
pub(crate) fn verify_registration_proof(
    context: &Context,
    proof: &JsonValue,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> FieldResult<(String, Option<String>, u32)> {
    match &proof.0 {
        serde_json::Value::String(sig_pem) => {
            verify_ssh_registration(context, sig_pem, username, now)
        }
        serde_json::Value::Object(_) => {
            verify_webauthn_registration(context, &proof.0, username, now)
        }
        _ => Err(field_error(
            "Invalid proof: expected a string (SSH signature) or object (WebAuthn attestation)",
        )),
    }
}

/// SSH registration: parse SshSig from PEM, extract pubkey, verify against challenge frames.
/// Returns (openssh_key, credential_id=None, sign_count=0).
fn verify_ssh_registration(
    context: &Context,
    sig_pem: &str,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> FieldResult<(String, Option<String>, u32)> {
    let ssh_sig: ssh_key::SshSig = sig_pem.parse().map_err(|e| {
        tracing::warn!("Invalid SSH signature PEM: {e}");
        field_error("Invalid credentials")
    })?;

    let public_key = ssh_key::PublicKey::from(ssh_sig.public_key().clone());
    let openssh_key = public_key.to_openssh().map_err(|e| {
        tracing::warn!("Failed to serialize public key: {e}");
        field_error("Invalid credentials")
    })?;

    // Verify against valid challenge frames
    let valid_challenges = context.challenger.valid_challenges(now, username);
    let verified = valid_challenges.iter().any(|challenge| {
        public_key
            .verify(
                challenge::CHALLENGE_NAMESPACE,
                challenge.as_bytes(),
                &ssh_sig,
            )
            .is_ok()
    });
    if !verified {
        return Err(field_error("Invalid credentials"));
    }

    Ok((openssh_key, None, 0))
}

/// WebAuthn registration (attestation): parse attestation response, extract COSE key, convert to SSH.
/// Returns (openssh_key, credential_id, sign_count).
fn verify_webauthn_registration(
    context: &Context,
    proof: &serde_json::Value,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> FieldResult<(String, Option<String>, u32)> {
    let b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let credential_id_b64 = proof
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'id' in WebAuthn attestation"))?;

    let response = proof
        .get("response")
        .ok_or_else(|| field_error("Missing 'response' in WebAuthn attestation"))?;

    let client_data_b64 = response
        .get("clientDataJSON")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'clientDataJSON'"))?;

    let attestation_object_b64 = response
        .get("attestationObject")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'attestationObject'"))?;

    let client_data_bytes = b64url.decode(client_data_b64).map_err(|e| {
        tracing::warn!("Invalid base64url in clientDataJSON: {e}");
        field_error("Invalid credentials")
    })?;

    let client_data =
        webauthn::parse_client_data(&client_data_bytes, "webauthn.create").map_err(|e| {
            tracing::warn!("Invalid clientDataJSON: {e}");
            field_error("Invalid credentials")
        })?;

    // The challenge in clientDataJSON is base64url(challenge_string_bytes).
    // Decode it and match against valid challenge frames.
    let challenge_bytes = b64url.decode(&client_data.challenge).map_err(|e| {
        tracing::warn!("Invalid base64url challenge in clientDataJSON: {e}");
        field_error("Invalid credentials")
    })?;
    let challenge_string = String::from_utf8(challenge_bytes).map_err(|e| {
        tracing::warn!("Challenge is not valid UTF-8: {e}");
        field_error("Invalid credentials")
    })?;

    let valid_challenges = context.challenger.valid_challenges(now, username);
    if !valid_challenges.contains(&challenge_string) {
        return Err(field_error("Invalid credentials"));
    }

    let attestation_bytes = b64url.decode(attestation_object_b64).map_err(|e| {
        tracing::warn!("Invalid base64url in attestationObject: {e}");
        field_error("Invalid credentials")
    })?;

    let attestation = webauthn::parse_attestation_object(&attestation_bytes).map_err(|e| {
        tracing::warn!("Invalid attestation object: {e}");
        field_error("Invalid credentials")
    })?;

    // Verify UP flag (bit 0)
    if attestation.flags & 0x01 == 0 {
        return Err(field_error("User presence flag not set"));
    }

    let (_fingerprint, openssh_key) = udb::cose_key_to_ssh(&attestation.cose_key).map_err(|e| {
        tracing::warn!("Failed to convert COSE key to SSH: {e}");
        field_error("Invalid credentials")
    })?;

    Ok((
        openssh_key,
        Some(credential_id_b64.to_owned()),
        attestation.sign_count,
    ))
}

/// SSH signin: parse SshSig, verify against challenge, check fingerprint exists.
pub(crate) async fn signin_ssh(
    context: &Context,
    user_client: &udb::UserClient,
    sig_pem: &str,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> FieldResult<()> {
    let ssh_sig: ssh_key::SshSig = sig_pem.parse().map_err(|e| {
        tracing::warn!("Invalid SSH signature PEM: {e}");
        field_error("Invalid credentials")
    })?;

    let public_key = ssh_key::PublicKey::from(ssh_sig.public_key().clone());
    let fingerprint = public_key
        .fingerprint(ssh_key::HashAlg::default())
        .to_string();

    let pubkeys = user_client.pubkeys();
    let has_fingerprint = pubkeys.contains(&fingerprint).await.map_err(|e| {
        tracing::error!("Failed to check pubkey fingerprint: {e}");
        internal_error()
    })?;

    if !has_fingerprint {
        return Err(field_error("Invalid credentials"));
    }

    // Verify against valid challenge frames
    let valid_challenges = context.challenger.valid_challenges(now, username);
    let verified = valid_challenges.iter().any(|challenge| {
        public_key
            .verify(
                challenge::CHALLENGE_NAMESPACE,
                challenge.as_bytes(),
                &ssh_sig,
            )
            .is_ok()
    });
    if !verified {
        return Err(field_error("Invalid credentials"));
    }

    // Store credential record if not present (idempotent migration)
    let openssh_key = public_key.to_openssh().map_err(|e| {
        tracing::warn!("Failed to serialize public key: {e}");
        field_error("Invalid credentials")
    })?;
    let _ = pubkeys.add_credential(&openssh_key, None, 0).await;

    Ok(())
}

/// WebAuthn signin (assertion): verify assertion signature against stored credential.
pub(crate) async fn signin_webauthn(
    context: &Context,
    user_client: &udb::UserClient,
    proof: &serde_json::Value,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> FieldResult<()> {
    let b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let credential_id_b64 = proof
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'id' in WebAuthn assertion"))?;

    let response = proof
        .get("response")
        .ok_or_else(|| field_error("Missing 'response' in WebAuthn assertion"))?;

    let auth_data_b64 = response
        .get("authenticatorData")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'authenticatorData'"))?;

    let client_data_b64 = response
        .get("clientDataJSON")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'clientDataJSON'"))?;

    let signature_b64 = response
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| field_error("Missing 'signature'"))?;

    let auth_data_bytes = b64url.decode(auth_data_b64).map_err(|e| {
        tracing::warn!("Invalid base64url in authenticatorData: {e}");
        field_error("Invalid credentials")
    })?;

    let client_data_bytes = b64url.decode(client_data_b64).map_err(|e| {
        tracing::warn!("Invalid base64url in clientDataJSON: {e}");
        field_error("Invalid credentials")
    })?;

    let signature_bytes = b64url.decode(signature_b64).map_err(|e| {
        tracing::warn!("Invalid base64url in signature: {e}");
        field_error("Invalid credentials")
    })?;

    // Parse and verify clientDataJSON
    let client_data =
        webauthn::parse_client_data(&client_data_bytes, "webauthn.get").map_err(|e| {
            tracing::warn!("Invalid clientDataJSON: {e}");
            field_error("Invalid credentials")
        })?;

    let challenge_bytes = b64url.decode(&client_data.challenge).map_err(|e| {
        tracing::warn!("Invalid base64url challenge in clientDataJSON: {e}");
        field_error("Invalid credentials")
    })?;
    let challenge_string = String::from_utf8(challenge_bytes).map_err(|e| {
        tracing::warn!("Challenge is not valid UTF-8: {e}");
        field_error("Invalid credentials")
    })?;

    let valid_challenges = context.challenger.valid_challenges(now, username);
    if !valid_challenges.contains(&challenge_string) {
        return Err(field_error("Invalid credentials"));
    }

    // Find matching credential
    let pubkeys = user_client.pubkeys();
    let credentials = pubkeys.list_credentials().await.map_err(|e| {
        tracing::error!("Failed to list credentials: {e}");
        internal_error()
    })?;

    let credential = credentials
        .iter()
        .find(|c| c.credential_id.as_deref() == Some(credential_id_b64))
        .ok_or_else(|| field_error("Invalid credentials"))?;

    // Parse authenticator data
    let auth_data = webauthn::parse_authenticator_data(&auth_data_bytes).map_err(|e| {
        tracing::warn!("Invalid authenticatorData: {e}");
        field_error("Invalid credentials")
    })?;

    // Verify UP flag
    if auth_data.flags & 0x01 == 0 {
        return Err(field_error("User presence flag not set"));
    }

    // Check sign counter (if non-zero, must be > stored)
    if (auth_data.sign_count > 0 || credential.sign_count > 0)
        && auth_data.sign_count <= credential.sign_count
    {
        tracing::warn!(
            "Sign counter regression: got {}, stored {}",
            auth_data.sign_count,
            credential.sign_count
        );
        return Err(field_error("Invalid credentials"));
    }

    // Verify signature
    let message = webauthn::build_assertion_message(&auth_data_bytes, &client_data_bytes);

    // Determine algorithm from stored key
    let ssh_pubkey = ssh_key::PublicKey::from_openssh(&credential.public_key).map_err(|e| {
        tracing::error!("Failed to parse stored public key: {e}");
        internal_error()
    })?;

    match ssh_pubkey.key_data() {
        ssh_key::public::KeyData::Ecdsa(_) => {
            webauthn::verify_es256(&credential.public_key, &message, &signature_bytes).map_err(
                |e| {
                    tracing::warn!("ES256 verification failed: {e}");
                    field_error("Invalid credentials")
                },
            )?;
        }
        ssh_key::public::KeyData::Ed25519(_) => {
            webauthn::verify_ed25519(&credential.public_key, &message, &signature_bytes).map_err(
                |e| {
                    tracing::warn!("Ed25519 verification failed: {e}");
                    field_error("Invalid credentials")
                },
            )?;
        }
        _ => {
            return Err(field_error("Unsupported key algorithm"));
        }
    }

    // Update sign count
    pubkeys
        .update_sign_count(&credential.fingerprint, auth_data.sign_count)
        .await
        .map_err(|e| {
            tracing::error!("Failed to update sign count: {e}");
            internal_error()
        })?;

    Ok(())
}
