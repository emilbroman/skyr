//! Notification Engine (NE) — consumes [`nq`] notification requests and
//! delivers them as e-mail.
//!
//! # Pipeline per delivery
//!
//! 1. Pull the next [`nq::Delivery`] from the queue.
//! 2. Compute the request's stable idempotency key.
//! 3. **Claim** the key in the [`dedup`] store (`SET NX EX`). If the key was
//!    already claimed, ack the redelivery without sending.
//! 4. **Resolve recipients** by extracting the owning organization from the
//!    entity QID and listing org members from UDB.
//! 5. **Render** the e-mail subject and body.
//! 6. **Send** via SMTP.
//!    - On success: ack the delivery.
//!    - On transient SMTP failure: release the dedup claim and
//!      `nack(requeue = true)` so a future redelivery can retry.
//!    - On permanent SMTP failure or invalid configuration: leave the dedup
//!      claim in place (preventing further retries that would also fail) and
//!      `nack(requeue = false)`. Operations should configure a DLX on the
//!      queue for postmortem inspection.
//!
//! NE has no knowledge of the incident state machine; it reacts only to events
//! on the queue.

pub mod dedup;
pub mod recipients;
pub mod render;
pub mod sender;

use nq::Delivery;

use crate::{
    dedup::{ClaimOutcome, DedupStore},
    recipients::resolve_recipients,
    render::render,
    sender::{SendError, SmtpSender},
};

/// Result of [`process_delivery`], reported to the caller for telemetry. The
/// caller does not need to act on this value — `process_delivery` already
/// performs the appropriate ack/nack on the underlying queue delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessOutcome {
    /// The notification was delivered (or judged a duplicate). Delivery acked.
    Delivered,
    /// The notification could not be delivered for a transient reason.
    /// Delivery has been nacked with requeue.
    Requeued,
    /// The notification failed permanently (bad address, malformed entity QID,
    /// etc.). Delivery has been nacked without requeue and routed to the DLX
    /// if one is configured.
    DeadLettered,
}

/// Orchestrates a single NQ delivery end-to-end.
///
/// The function never panics on a queue-side error: it logs and selects the
/// appropriate ack/nack so the broker is left in a consistent state. Errors
/// returned from the underlying ack/nack itself are reported via the returned
/// `Result` for the caller to log.
pub async fn process_delivery(
    delivery: Delivery,
    dedup: &DedupStore,
    udb: &udb::Client,
    sender: &SmtpSender,
) -> Result<ProcessOutcome, AckError> {
    let request = delivery.request.clone();
    let idempotency_key = request.idempotency_key();
    let event_type = request.event_type;
    let incident_id = request.incident_id.clone();
    let entity_qid = request.entity_qid.clone();
    let redelivered = delivery.redelivered();

    // ---- 1. dedup claim ---------------------------------------------------
    match dedup.try_claim(&idempotency_key).await {
        Ok(ClaimOutcome::Claimed) => {
            // proceed
        }
        Ok(ClaimOutcome::AlreadyClaimed) => {
            tracing::info!(
                incident_id = %incident_id,
                event_type = ?event_type,
                idempotency_key = %idempotency_key,
                redelivered,
                "dropping duplicate notification (already claimed)",
            );
            ack_or_log(&delivery).await?;
            return Ok(ProcessOutcome::Delivered);
        }
        Err(error) => {
            tracing::warn!(
                incident_id = %incident_id,
                event_type = ?event_type,
                error = %error,
                "dedup claim failed; nacking with requeue",
            );
            nack_or_log(&delivery, true).await?;
            return Ok(ProcessOutcome::Requeued);
        }
    }

    // ---- 2. resolve recipients --------------------------------------------
    let recipients = match resolve_recipients(udb, &entity_qid).await {
        Ok(list) => list,
        Err(recipients::ResolveError::InvalidEntityQid(_)) => {
            tracing::error!(
                incident_id = %incident_id,
                entity_qid = %entity_qid,
                "notification entity_qid is unparseable; dead-lettering",
            );
            // Permanent: release the claim is unnecessary because the message
            // will never become deliverable, but we keep it claimed so identical
            // redeliveries (e.g. via DLX -> requeue mistakes) don't re-process.
            nack_or_log(&delivery, false).await?;
            return Ok(ProcessOutcome::DeadLettered);
        }
        Err(recipients::ResolveError::OrgNotFound(org)) => {
            tracing::warn!(
                incident_id = %incident_id,
                entity_qid = %entity_qid,
                org = %org,
                "organization not found in UDB; nothing to send",
            );
            // Treat as delivered: there are zero valid recipients, and that is
            // not retryable from NE's side.
            ack_or_log(&delivery).await?;
            return Ok(ProcessOutcome::Delivered);
        }
        Err(recipients::ResolveError::Udb(error)) => {
            tracing::warn!(
                incident_id = %incident_id,
                entity_qid = %entity_qid,
                error = %error,
                "udb lookup failed; releasing dedup claim and requeuing",
            );
            release_or_log(dedup, &idempotency_key).await;
            nack_or_log(&delivery, true).await?;
            return Ok(ProcessOutcome::Requeued);
        }
    };

    if recipients.is_empty() {
        tracing::info!(
            incident_id = %incident_id,
            entity_qid = %entity_qid,
            "no recipients resolved; nothing to send",
        );
        ack_or_log(&delivery).await?;
        return Ok(ProcessOutcome::Delivered);
    }

    // ---- 3. render --------------------------------------------------------
    let email = render(&request);

    // ---- 4. send ----------------------------------------------------------
    let recipient_addresses: Vec<String> = recipients.iter().map(|r| r.email.clone()).collect();
    tracing::info!(
        incident_id = %incident_id,
        event_type = ?event_type,
        recipient_count = recipient_addresses.len(),
        "sending notification email",
    );

    match sender.send(&email, &recipient_addresses).await {
        Ok(()) => {
            // Best-effort confirm; failures are not fatal because the TTL on
            // the claim already protects future redeliveries.
            if let Err(err) = dedup.confirm(&idempotency_key).await {
                tracing::warn!(error = %err, "dedup confirm failed (non-fatal)");
            }
            ack_or_log(&delivery).await?;
            Ok(ProcessOutcome::Delivered)
        }
        Err(error) if error.is_transient() => {
            tracing::warn!(
                incident_id = %incident_id,
                event_type = ?event_type,
                error = %error,
                "transient smtp failure; releasing dedup claim and requeuing",
            );
            release_or_log(dedup, &idempotency_key).await;
            nack_or_log(&delivery, true).await?;
            Ok(ProcessOutcome::Requeued)
        }
        Err(SendError::InvalidRecipientAddress { address, reason }) => {
            tracing::error!(
                incident_id = %incident_id,
                address = %address,
                error = %reason,
                "permanent: invalid recipient address; dead-lettering",
            );
            nack_or_log(&delivery, false).await?;
            Ok(ProcessOutcome::DeadLettered)
        }
        Err(error) => {
            tracing::error!(
                incident_id = %incident_id,
                event_type = ?event_type,
                error = %error,
                "permanent smtp failure; dead-lettering",
            );
            nack_or_log(&delivery, false).await?;
            Ok(ProcessOutcome::DeadLettered)
        }
    }
}

/// Error returned by [`process_delivery`] when an `ack`/`nack` itself fails.
/// All other errors are recovered internally — see the `ProcessOutcome` for the
/// observed status.
#[derive(Debug, thiserror::Error)]
pub enum AckError {
    #[error("failed to ack notification delivery: {0}")]
    Ack(#[source] lapin::Error),

    #[error("failed to nack notification delivery: {0}")]
    Nack(#[source] lapin::Error),
}

async fn ack_or_log(delivery: &Delivery) -> Result<(), AckError> {
    delivery.ack().await.map_err(AckError::Ack)
}

async fn nack_or_log(delivery: &Delivery, requeue: bool) -> Result<(), AckError> {
    delivery.nack(requeue).await.map_err(AckError::Nack)
}

async fn release_or_log(dedup: &DedupStore, key: &str) {
    if let Err(err) = dedup.release(key).await {
        tracing::warn!(error = %err, "dedup release failed (non-fatal)");
    }
}
