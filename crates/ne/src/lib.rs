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
//! 5. **Enqueue** the request into the per-recipient [`batcher`] for each
//!    resolved recipient and ack the delivery. The batcher coalesces
//!    notifications for the same recipient over a short window so that a
//!    runaway upstream cannot translate into an e-mail flood — see the
//!    [`batcher`] module docs for the semantics. SMTP failures inside the
//!    flusher are best-effort: they are logged and dropped because the
//!    delivery has already been acked. The durable record remains in SDB.
//!
//! NE has no knowledge of the incident state machine; it reacts only to events
//! on the queue.

pub mod batcher;
pub mod dedup;
pub mod recipients;
pub mod render;
pub mod sender;

use nq::Delivery;

use crate::{
    batcher::BatcherHandle,
    dedup::{ClaimOutcome, DedupStore},
    recipients::resolve_recipients,
};

/// Result of [`process_delivery`], reported to the caller for telemetry. The
/// caller does not need to act on this value — `process_delivery` already
/// performs the appropriate ack/nack on the underlying queue delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessOutcome {
    /// The notification was enqueued into the batcher (or judged a
    /// duplicate, or had no recipients). Delivery acked.
    Enqueued,
    /// The notification could not be processed for a transient reason
    /// (UDB lookup or dedup-store failure). Delivery has been nacked with
    /// requeue.
    Requeued,
    /// The notification failed permanently (malformed entity QID, etc.).
    /// Delivery has been nacked without requeue and routed to the DLX if
    /// one is configured.
    DeadLettered,
}

/// Orchestrates a single NQ delivery up to the point of enqueue into the
/// per-recipient batcher.
///
/// The function never panics on a queue-side error: it logs and selects
/// the appropriate ack/nack so the broker is left in a consistent state.
/// Errors returned from the underlying ack/nack itself are reported via
/// the returned `Result` for the caller to log.
///
/// SMTP delivery is **not** performed inline — that work is owned by the
/// batcher's flusher task, and SMTP failures discovered there are logged
/// and dropped because the AMQP delivery has already been acked. See the
/// [`batcher`] module docs for the rationale.
pub async fn process_delivery(
    delivery: Delivery,
    dedup: &DedupStore,
    udb: &udb::Client,
    batcher: &BatcherHandle,
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
            return Ok(ProcessOutcome::Enqueued);
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
            // Permanent: releasing the claim is unnecessary because the
            // message will never become deliverable, but we keep it claimed
            // so identical redeliveries (e.g. via DLX -> requeue mistakes)
            // don't re-process.
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
            // Treat as enqueued: there are zero valid recipients, and
            // that is not retryable from NE's side.
            ack_or_log(&delivery).await?;
            return Ok(ProcessOutcome::Enqueued);
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
            "no recipients resolved; nothing to enqueue",
        );
        ack_or_log(&delivery).await?;
        return Ok(ProcessOutcome::Enqueued);
    }

    // ---- 3. enqueue into the per-recipient batcher ------------------------
    tracing::info!(
        incident_id = %incident_id,
        event_type = ?event_type,
        recipient_count = recipients.len(),
        "enqueuing notification into per-recipient batcher",
    );
    for recipient in &recipients {
        batcher
            .enqueue(recipient.email.clone(), request.clone())
            .await;
    }

    // Best-effort confirm; failures are not fatal because the TTL on the
    // claim already protects future redeliveries.
    if let Err(err) = dedup.confirm(&idempotency_key).await {
        tracing::warn!(error = %err, "dedup confirm failed (non-fatal)");
    }
    ack_or_log(&delivery).await?;
    Ok(ProcessOutcome::Enqueued)
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
