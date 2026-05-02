//! Proof verification: checks that a presented SSH or WebAuthn proof was
//! generated against a currently-valid challenge frame for `username`,
//! and (for sign-in) corresponds to a stored credential.
//!
//! All verification failures are reported as `Status::unauthenticated`
//! with a generic "Invalid credentials" message, matching the API's
//! pre-IAS behaviour: never leak whether a username exists, whether the
//! signature was malformed, or whether the challenge was wrong.

use base64::Engine;
use chrono::Utc;
use tonic::Status;

use crate::challenge::{CHALLENGE_NAMESPACE, Challenger};
use crate::proto;
use crate::webauthn;

/// Generic invalid-credentials error. Use for anything an unauthenticated
/// caller could provoke — never include a hint about whether the user
/// exists, the proof was malformed, or the signature failed.
fn invalid_credentials() -> Status {
    Status::unauthenticated("Invalid credentials")
}

/// Result of verifying a registration proof. Captures everything the
/// service needs to persist a credential.
pub struct RegistrationOutcome {
    pub openssh_key: String,
    pub credential_id: Option<String>,
    pub sign_count: u32,
}

/// Verify a registration proof (signup or AddCredential). Accepts an SSH
/// signature or a WebAuthn attestation; rejects assertions.
pub fn verify_registration_proof(
    challenger: &Challenger,
    proof: &proto::Proof,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> Result<RegistrationOutcome, Status> {
    match proof.kind.as_ref() {
        Some(proto::proof::Kind::SshSignature(sig_pem)) => {
            verify_ssh_registration(challenger, sig_pem, username, now)
        }
        Some(proto::proof::Kind::Attestation(attestation)) => {
            verify_webauthn_registration(challenger, attestation, username, now)
        }
        Some(proto::proof::Kind::Assertion(_)) => Err(Status::invalid_argument(
            "registration requires an attestation, not an assertion",
        )),
        None => Err(Status::invalid_argument("missing proof")),
    }
}

fn verify_ssh_registration(
    challenger: &Challenger,
    sig_pem: &str,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> Result<RegistrationOutcome, Status> {
    let ssh_sig: ssh_key::SshSig = sig_pem.parse().map_err(|e| {
        tracing::warn!("Invalid SSH signature PEM: {e}");
        invalid_credentials()
    })?;

    let public_key = ssh_key::PublicKey::from(ssh_sig.public_key().clone());
    let openssh_key = public_key.to_openssh().map_err(|e| {
        tracing::warn!("Failed to serialize public key: {e}");
        invalid_credentials()
    })?;

    let valid_challenges = challenger.valid_challenges(now, username);
    let verified = valid_challenges.iter().any(|challenge| {
        public_key
            .verify(CHALLENGE_NAMESPACE, challenge.as_bytes(), &ssh_sig)
            .is_ok()
    });
    if !verified {
        return Err(invalid_credentials());
    }

    Ok(RegistrationOutcome {
        openssh_key,
        credential_id: None,
        sign_count: 0,
    })
}

fn verify_webauthn_registration(
    challenger: &Challenger,
    attestation: &proto::WebAuthnAttestation,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> Result<RegistrationOutcome, Status> {
    let b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let client_data = webauthn::parse_client_data(&attestation.client_data_json, "webauthn.create")
        .map_err(|e| {
            tracing::warn!("Invalid clientDataJSON: {e}");
            invalid_credentials()
        })?;

    let challenge_bytes = b64url.decode(&client_data.challenge).map_err(|e| {
        tracing::warn!("Invalid base64url challenge in clientDataJSON: {e}");
        invalid_credentials()
    })?;
    let challenge_string = String::from_utf8(challenge_bytes).map_err(|e| {
        tracing::warn!("Challenge is not valid UTF-8: {e}");
        invalid_credentials()
    })?;

    let valid_challenges = challenger.valid_challenges(now, username);
    if !valid_challenges.contains(&challenge_string) {
        return Err(invalid_credentials());
    }

    let parsed =
        webauthn::parse_attestation_object(&attestation.attestation_object).map_err(|e| {
            tracing::warn!("Invalid attestation object: {e}");
            invalid_credentials()
        })?;

    if parsed.flags & 0x01 == 0 {
        return Err(Status::unauthenticated("User presence flag not set"));
    }

    let (_fingerprint, openssh_key) = udb::cose_key_to_ssh(&parsed.cose_key).map_err(|e| {
        tracing::warn!("Failed to convert COSE key to SSH: {e}");
        invalid_credentials()
    })?;

    Ok(RegistrationOutcome {
        openssh_key,
        credential_id: Some(attestation.credential_id.clone()),
        sign_count: parsed.sign_count,
    })
}

pub async fn verify_signin(
    challenger: &Challenger,
    user_client: &udb::UserClient,
    proof: &proto::Proof,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> Result<(), Status> {
    match proof.kind.as_ref() {
        Some(proto::proof::Kind::SshSignature(sig_pem)) => {
            verify_signin_ssh(challenger, user_client, sig_pem, username, now).await
        }
        Some(proto::proof::Kind::Assertion(assertion)) => {
            verify_signin_webauthn(challenger, user_client, assertion, username, now).await
        }
        Some(proto::proof::Kind::Attestation(_)) => Err(Status::invalid_argument(
            "sign-in requires an assertion, not an attestation",
        )),
        None => Err(Status::invalid_argument("missing proof")),
    }
}

async fn verify_signin_ssh(
    challenger: &Challenger,
    user_client: &udb::UserClient,
    sig_pem: &str,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> Result<(), Status> {
    let ssh_sig: ssh_key::SshSig = sig_pem.parse().map_err(|e| {
        tracing::warn!("Invalid SSH signature PEM: {e}");
        invalid_credentials()
    })?;

    let public_key = ssh_key::PublicKey::from(ssh_sig.public_key().clone());
    let fingerprint = public_key
        .fingerprint(ssh_key::HashAlg::default())
        .to_string();

    let pubkeys = user_client.pubkeys();
    let has_fingerprint = pubkeys.contains(&fingerprint).await.map_err(|e| {
        tracing::error!("Failed to check pubkey fingerprint: {e}");
        Status::internal("Internal server error")
    })?;
    if !has_fingerprint {
        return Err(invalid_credentials());
    }

    let valid_challenges = challenger.valid_challenges(now, username);
    let verified = valid_challenges.iter().any(|challenge| {
        public_key
            .verify(CHALLENGE_NAMESPACE, challenge.as_bytes(), &ssh_sig)
            .is_ok()
    });
    if !verified {
        return Err(invalid_credentials());
    }

    // Idempotently upsert the credential record so that legacy entries
    // added via the bare `add()` path get a proper credential row.
    let openssh_key = public_key.to_openssh().map_err(|e| {
        tracing::warn!("Failed to serialize public key: {e}");
        invalid_credentials()
    })?;
    let _ = pubkeys.add_credential(&openssh_key, None, 0).await;

    Ok(())
}

async fn verify_signin_webauthn(
    challenger: &Challenger,
    user_client: &udb::UserClient,
    assertion: &proto::WebAuthnAssertion,
    username: &str,
    now: chrono::DateTime<Utc>,
) -> Result<(), Status> {
    let b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let client_data = webauthn::parse_client_data(&assertion.client_data_json, "webauthn.get")
        .map_err(|e| {
            tracing::warn!("Invalid clientDataJSON: {e}");
            invalid_credentials()
        })?;

    let challenge_bytes = b64url.decode(&client_data.challenge).map_err(|e| {
        tracing::warn!("Invalid base64url challenge in clientDataJSON: {e}");
        invalid_credentials()
    })?;
    let challenge_string = String::from_utf8(challenge_bytes).map_err(|e| {
        tracing::warn!("Challenge is not valid UTF-8: {e}");
        invalid_credentials()
    })?;

    let valid_challenges = challenger.valid_challenges(now, username);
    if !valid_challenges.contains(&challenge_string) {
        return Err(invalid_credentials());
    }

    let pubkeys = user_client.pubkeys();
    let credentials = pubkeys.list_credentials().await.map_err(|e| {
        tracing::error!("Failed to list credentials: {e}");
        Status::internal("Internal server error")
    })?;

    let credential = credentials
        .iter()
        .find(|c| c.credential_id.as_deref() == Some(assertion.credential_id.as_str()))
        .ok_or_else(invalid_credentials)?;

    let auth_data =
        webauthn::parse_authenticator_data(&assertion.authenticator_data).map_err(|e| {
            tracing::warn!("Invalid authenticatorData: {e}");
            invalid_credentials()
        })?;

    if auth_data.flags & 0x01 == 0 {
        return Err(Status::unauthenticated("User presence flag not set"));
    }

    if (auth_data.sign_count > 0 || credential.sign_count > 0)
        && auth_data.sign_count <= credential.sign_count
    {
        tracing::warn!(
            "Sign counter regression: got {}, stored {}",
            auth_data.sign_count,
            credential.sign_count
        );
        return Err(invalid_credentials());
    }

    let message = webauthn::build_assertion_message(
        &assertion.authenticator_data,
        &assertion.client_data_json,
    );

    let ssh_pubkey = ssh_key::PublicKey::from_openssh(&credential.public_key).map_err(|e| {
        tracing::error!("Failed to parse stored public key: {e}");
        Status::internal("Internal server error")
    })?;

    match ssh_pubkey.key_data() {
        ssh_key::public::KeyData::Ecdsa(_) => {
            webauthn::verify_es256(&credential.public_key, &message, &assertion.signature)
                .map_err(|e| {
                    tracing::warn!("ES256 verification failed: {e}");
                    invalid_credentials()
                })?;
        }
        ssh_key::public::KeyData::Ed25519(_) => {
            webauthn::verify_ed25519(&credential.public_key, &message, &assertion.signature)
                .map_err(|e| {
                    tracing::warn!("Ed25519 verification failed: {e}");
                    invalid_credentials()
                })?;
        }
        _ => {
            return Err(Status::unauthenticated("Unsupported key algorithm"));
        }
    }

    pubkeys
        .update_sign_count(&credential.fingerprint, auth_data.sign_count)
        .await
        .map_err(|e| {
            tracing::error!("Failed to update sign count: {e}");
            Status::internal("Internal server error")
        })?;

    Ok(())
}
