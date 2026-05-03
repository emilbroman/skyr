//! Per-recipient notification batcher.
//!
//! Coalesces notification e-mails so a sustained surge of events for a
//! single recipient produces at most one e-mail per batch window. Designed
//! as defense in depth against bugs that cause RE to emit notifications in
//! tight loops — see the watchdog flap incident write-up. Under normal
//! operations batches contain a single event and the only added cost is
//! the [`BatcherConfig::quiet`] flush latency.
//!
//! ## Semantics
//!
//! For each recipient (keyed on e-mail address):
//!
//! - The first event starts a new batch and arms two deadlines:
//!   * `quiet`: a sliding timer reset to `now + quiet` on every event.
//!   * `cap`: a fixed timer set to `first_arrival + cap`, never extended.
//! - The batch flushes when *either* deadline passes, whichever is first.
//! - When a batch reaches [`BatcherConfig::max_batch_size`], the quiet
//!   deadline is collapsed to `now` so the next flusher tick drains it
//!   without waiting out the timer.
//! - Flush concatenates the rendered bodies of every queued event into a
//!   single SMTP message; the subject reflects the count and the
//!   open/closed split. The batcher never reasons about incident state.
//!
//! ## Lifecycle and ownership
//!
//! [`Batcher::start`] spawns the background flusher task and returns a
//! [`Batcher`] that owns the flusher's join handle. Use
//! [`Batcher::handle`] to get a cheaply-cloneable [`BatcherHandle`] that
//! workers can use to call [`BatcherHandle::enqueue`]. Call
//! [`Batcher::shutdown`] from the supervising task on graceful exit; it
//! drains every in-flight batch synchronously before the flusher task
//! returns, then awaits the flusher to ensure SMTP has completed.
//!
//! Flush failures (transient SMTP errors, invalid recipient addresses)
//! are logged and dropped — the AMQP delivery has already been acked at
//! consume time per the design choice in the watchdog-flap mitigation
//! plan. The durable record remains in SDB; this batcher is responsible
//! for *notifications*, not for state of record.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nq::{NotificationEventType, NotificationRequest};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::render::{RenderedEmail, render};
use crate::sender::SmtpSender;

/// Configuration for the batcher.
#[derive(Clone, Debug)]
pub struct BatcherConfig {
    /// Sliding quiet timer; reset on every new event for the recipient.
    pub quiet: Duration,
    /// Hard cap on a batch's lifetime; measured from the first event,
    /// not extended by subsequent ones.
    pub cap: Duration,
    /// Hard upper bound on the number of events that may queue for a
    /// single recipient before the batch is forced to flush.
    pub max_batch_size: usize,
    /// Fallback poll interval when the flusher has nothing to do; the
    /// flusher is normally woken via `Notify` instead of timing out.
    pub poll_interval: Duration,
}

impl Default for BatcherConfig {
    fn default() -> Self {
        Self {
            quiet: Duration::from_secs(10),
            cap: Duration::from_secs(10 * 60),
            max_batch_size: 5_000,
            poll_interval: Duration::from_secs(1),
        }
    }
}

struct Batch {
    quiet_deadline: Instant,
    cap_deadline: Instant,
    events: Vec<NotificationRequest>,
}

#[derive(Default)]
struct BatcherState {
    batches: HashMap<String, Batch>,
}

/// Cheaply-cloneable handle workers use to push events into the
/// batcher. Holds only `Arc`'d references; cloning is essentially free.
#[derive(Clone)]
pub struct BatcherHandle {
    state: Arc<Mutex<BatcherState>>,
    config: BatcherConfig,
    notify: Arc<Notify>,
}

impl BatcherHandle {
    /// Pushes one event into the batch for `recipient`. Returns once the
    /// event is in the in-memory batch; the SMTP send happens later from
    /// the flusher task.
    pub async fn enqueue(&self, recipient: String, event: NotificationRequest) {
        let now = Instant::now();
        let cap_deadline = now + self.config.cap;
        let quiet_deadline = now + self.config.quiet;
        {
            let mut state = self.state.lock().await;
            let batch = state.batches.entry(recipient).or_insert_with(|| Batch {
                quiet_deadline,
                cap_deadline,
                events: Vec::new(),
            });
            batch.events.push(event);
            // The quiet timer slides; the cap timer is fixed from
            // first_arrival and is therefore not refreshed.
            batch.quiet_deadline = quiet_deadline;
            if batch.events.len() >= self.config.max_batch_size {
                // Collapse the quiet deadline so the next scan drains
                // this recipient. We avoid flushing inline so the
                // consume path is never blocked on SMTP.
                batch.quiet_deadline = now;
            }
        }
        // Wake the flusher so it can re-evaluate its sleep target — a
        // fresh batch may have set an earlier deadline than the flusher
        // currently knows about.
        self.notify.notify_one();
    }
}

/// Owning handle to the batcher. Holds the flusher task's join handle
/// and the shutdown notifier; only one of these exists per running
/// flusher. Use [`Batcher::handle`] to mint cloneable [`BatcherHandle`]s
/// for worker tasks.
pub struct Batcher {
    handle: BatcherHandle,
    shutdown: Arc<Notify>,
    flusher: JoinHandle<()>,
}

impl Batcher {
    /// Starts the batcher and spawns the background flusher task.
    pub fn start(config: BatcherConfig, sender: Arc<SmtpSender>) -> Self {
        let state = Arc::new(Mutex::new(BatcherState::default()));
        let notify = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());

        let flusher = tokio::spawn(run_flusher(
            state.clone(),
            config.clone(),
            sender,
            notify.clone(),
            shutdown.clone(),
        ));

        let handle = BatcherHandle {
            state,
            config,
            notify,
        };

        Self {
            handle,
            shutdown,
            flusher,
        }
    }

    /// Returns a cheap, cloneable handle for worker tasks.
    pub fn handle(&self) -> BatcherHandle {
        self.handle.clone()
    }

    /// Drains every in-flight batch and waits for the flusher task to
    /// exit. Intended for graceful shutdown (SIGTERM / rolling deploy).
    pub async fn shutdown(self) {
        self.shutdown.notify_one();
        if let Err(error) = self.flusher.await {
            tracing::warn!(error = %error, "batcher flusher task did not exit cleanly");
        }
    }
}

async fn run_flusher(
    state: Arc<Mutex<BatcherState>>,
    config: BatcherConfig,
    sender: Arc<SmtpSender>,
    notify: Arc<Notify>,
    shutdown: Arc<Notify>,
) {
    loop {
        let sleep_for = compute_sleep(&state, &config).await;

        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {}
            _ = notify.notified() => {}
            _ = shutdown.notified() => {
                let drained = drain_all(&state).await;
                for (recipient, events) in drained {
                    flush_one(&recipient, events, &sender).await;
                }
                return;
            }
        }

        let due = drain_due(&state).await;
        for (recipient, events) in due {
            flush_one(&recipient, events, &sender).await;
        }
    }
}

async fn compute_sleep(state: &Mutex<BatcherState>, config: &BatcherConfig) -> Duration {
    let now = Instant::now();
    let s = state.lock().await;
    let mut earliest: Option<Instant> = None;
    for batch in s.batches.values() {
        let d = batch.quiet_deadline.min(batch.cap_deadline);
        if earliest.is_none_or(|e| d < e) {
            earliest = Some(d);
        }
    }
    drop(s);
    match earliest {
        Some(d) => d
            .saturating_duration_since(now)
            .max(Duration::from_millis(10)),
        None => config.poll_interval,
    }
}

async fn drain_due(state: &Mutex<BatcherState>) -> Vec<(String, Vec<NotificationRequest>)> {
    let now = Instant::now();
    let mut s = state.lock().await;
    let mut out = Vec::new();
    s.batches.retain(|recipient, batch| {
        let due = now >= batch.quiet_deadline || now >= batch.cap_deadline;
        if due {
            out.push((recipient.clone(), std::mem::take(&mut batch.events)));
            false
        } else {
            true
        }
    });
    out
}

async fn drain_all(state: &Mutex<BatcherState>) -> Vec<(String, Vec<NotificationRequest>)> {
    let mut s = state.lock().await;
    let mut out = Vec::with_capacity(s.batches.len());
    for (recipient, mut batch) in s.batches.drain() {
        let events = std::mem::take(&mut batch.events);
        if !events.is_empty() {
            out.push((recipient, events));
        }
    }
    out
}

async fn flush_one(recipient: &str, events: Vec<NotificationRequest>, sender: &SmtpSender) {
    if events.is_empty() {
        return;
    }
    let event_count = events.len();
    let email = render_batch(&events);
    let to = vec![recipient.to_string()];
    if let Err(error) = sender.send(&email, &to).await {
        tracing::error!(
            recipient = %recipient,
            event_count,
            error = %error,
            "failed to flush notification batch (events dropped — durable record remains in SDB)",
        );
    } else {
        tracing::debug!(
            recipient = %recipient,
            event_count,
            "flushed notification batch",
        );
    }
}

/// Concatenates rendered notification bodies into a single batched
/// e-mail. The subject reflects the count and the open/closed split,
/// derived purely from event_type counters — the batcher never reasons
/// about incident state.
///
/// A single-event batch renders identically to a non-batched email so
/// isolated notifications look the same as before; only sustained
/// activity gets the count-style batched subject.
pub fn render_batch(events: &[NotificationRequest]) -> RenderedEmail {
    if events.len() == 1 {
        return render(&events[0]);
    }

    let n_open = events
        .iter()
        .filter(|r| matches!(r.event_type, NotificationEventType::Opened))
        .count();
    let n_closed = events
        .iter()
        .filter(|r| matches!(r.event_type, NotificationEventType::Closed))
        .count();

    let subject = format!(
        "[Skyr] {} incident events ({} opened, {} closed)",
        events.len(),
        n_open,
        n_closed,
    );

    let mut body = format!(
        "{} incident events were observed for this recipient within the batch window.\n\n",
        events.len(),
    );

    let separator = "----------------------------------------\n\n";
    for (i, event) in events.iter().enumerate() {
        if i > 0 {
            body.push_str(separator);
        }
        // Each rendered body already ends in its own automated-message
        // footer; strip everything from the footer onward so the batched
        // body carries a single footer at the bottom rather than one per
        // event.
        let rendered = render(event);
        body.push_str(strip_trailing_footer(&rendered.body));
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push('\n');
    }

    body.push_str("-- \nThis is an automated message from Skyr.\n");

    RenderedEmail { subject, body }
}

/// Removes the trailing "-- \nThis is an automated message…" footer
/// emitted by [`render`] so the batched body can carry a single footer
/// at the bottom rather than one after every event.
fn strip_trailing_footer(body: &str) -> &str {
    const FOOTER_MARKER: &str = "\n-- \nThis is an automated message";
    match body.rfind(FOOTER_MARKER) {
        Some(idx) => &body[..idx],
        None => body,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use nq::SeverityCategory;

    use super::*;

    fn open_request(id: &str) -> NotificationRequest {
        NotificationRequest {
            incident_id: id.to_string(),
            event_type: NotificationEventType::Opened,
            entity_qid:
                "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
                    .to_string(),
            category: SeverityCategory::SystemError,
            opened_at: ts("2026-05-03T10:00:00Z"),
            closed_at: None,
            summary: Some("watchdog: no report".to_string()),
        }
    }

    fn close_request(id: &str) -> NotificationRequest {
        NotificationRequest {
            incident_id: id.to_string(),
            event_type: NotificationEventType::Closed,
            entity_qid:
                "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
                    .to_string(),
            category: SeverityCategory::SystemError,
            opened_at: ts("2026-05-03T10:00:00Z"),
            closed_at: Some(ts("2026-05-03T10:00:30Z")),
            summary: Some("watchdog: no report".to_string()),
        }
    }

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn render_batch_with_single_event_matches_render() {
        let req = open_request("01HZX9P5K2JN7YQVJ3Q6T4ZB8N");
        let single = render_batch(std::slice::from_ref(&req));
        let direct = render(&req);
        assert_eq!(single, direct);
    }

    #[test]
    fn render_batch_subject_lists_counts() {
        let events = [
            open_request("A"),
            close_request("A"),
            open_request("B"),
            close_request("B"),
            open_request("C"),
        ];
        let batched = render_batch(&events);
        assert!(
            batched.subject.contains("5 incident events"),
            "subject: {}",
            batched.subject
        );
        assert!(
            batched.subject.contains("3 opened"),
            "subject: {}",
            batched.subject
        );
        assert!(
            batched.subject.contains("2 closed"),
            "subject: {}",
            batched.subject
        );
    }

    #[test]
    fn render_batch_body_contains_each_event_and_one_footer() {
        let events = [open_request("A"), close_request("A")];
        let batched = render_batch(&events);
        // Each event's content shows up.
        assert!(batched.body.contains("incident has been opened"));
        assert!(batched.body.contains("incident has been closed"));
        // Exactly one footer at the bottom — not one per event.
        let footer_count = batched.body.matches("This is an automated message").count();
        assert_eq!(
            footer_count, 1,
            "expected exactly one footer, got {footer_count}\n{}",
            batched.body
        );
    }

    #[test]
    fn render_batch_with_no_events_is_safe() {
        // Defensive: render_batch should never crash on an empty slice
        // even though flush_one short-circuits before reaching it.
        let batched = render_batch(&[]);
        assert!(batched.subject.contains("0 incident events"));
    }

    #[tokio::test]
    async fn drain_due_returns_only_expired_batches() {
        let state = Arc::new(Mutex::new(BatcherState::default()));
        let now = Instant::now();
        {
            let mut s = state.lock().await;
            s.batches.insert(
                "expired@example.com".to_string(),
                Batch {
                    quiet_deadline: now - Duration::from_millis(1),
                    cap_deadline: now + Duration::from_secs(600),
                    events: vec![open_request("A")],
                },
            );
            s.batches.insert(
                "fresh@example.com".to_string(),
                Batch {
                    quiet_deadline: now + Duration::from_secs(10),
                    cap_deadline: now + Duration::from_secs(600),
                    events: vec![open_request("B")],
                },
            );
        }
        let drained = drain_due(&state).await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0, "expired@example.com");
        let s = state.lock().await;
        assert!(s.batches.contains_key("fresh@example.com"));
        assert!(!s.batches.contains_key("expired@example.com"));
    }

    #[tokio::test]
    async fn drain_all_takes_every_non_empty_batch() {
        let state = Arc::new(Mutex::new(BatcherState::default()));
        let now = Instant::now();
        {
            let mut s = state.lock().await;
            s.batches.insert(
                "a@example.com".to_string(),
                Batch {
                    quiet_deadline: now + Duration::from_secs(10),
                    cap_deadline: now + Duration::from_secs(600),
                    events: vec![open_request("A")],
                },
            );
            s.batches.insert(
                "b@example.com".to_string(),
                Batch {
                    quiet_deadline: now + Duration::from_secs(10),
                    cap_deadline: now + Duration::from_secs(600),
                    events: vec![open_request("B"), close_request("B")],
                },
            );
            // An empty entry — should not be returned.
            s.batches.insert(
                "empty@example.com".to_string(),
                Batch {
                    quiet_deadline: now + Duration::from_secs(10),
                    cap_deadline: now + Duration::from_secs(600),
                    events: vec![],
                },
            );
        }
        let drained = drain_all(&state).await;
        assert_eq!(drained.len(), 2);
        let recipients: Vec<&String> = drained.iter().map(|(r, _)| r).collect();
        assert!(recipients.contains(&&"a@example.com".to_string()));
        assert!(recipients.contains(&&"b@example.com".to_string()));
        let s = state.lock().await;
        assert!(s.batches.is_empty());
    }

    #[tokio::test]
    async fn handle_enqueue_appends_and_slides_quiet_deadline() {
        let state = Arc::new(Mutex::new(BatcherState::default()));
        let config = BatcherConfig::default();
        let handle = BatcherHandle {
            state: state.clone(),
            config: config.clone(),
            notify: Arc::new(Notify::new()),
        };
        handle
            .enqueue("user@example.com".into(), open_request("A"))
            .await;
        // Capture the quiet deadline after the first enqueue; sleep
        // briefly so the second enqueue's `now` is strictly later.
        let first_quiet = state.lock().await.batches["user@example.com"].quiet_deadline;
        tokio::time::sleep(Duration::from_millis(20)).await;
        handle
            .enqueue("user@example.com".into(), close_request("A"))
            .await;
        let s = state.lock().await;
        let batch = &s.batches["user@example.com"];
        assert_eq!(batch.events.len(), 2);
        assert!(
            batch.quiet_deadline > first_quiet,
            "quiet deadline should slide on each enqueue",
        );
    }

    #[tokio::test]
    async fn handle_enqueue_collapses_quiet_deadline_at_size_cap() {
        let state = Arc::new(Mutex::new(BatcherState::default()));
        let config = BatcherConfig {
            quiet: Duration::from_secs(10),
            cap: Duration::from_secs(600),
            max_batch_size: 3,
            poll_interval: Duration::from_secs(1),
        };
        let handle = BatcherHandle {
            state: state.clone(),
            config: config.clone(),
            notify: Arc::new(Notify::new()),
        };
        for i in 0..3 {
            handle
                .enqueue("user@example.com".into(), open_request(&format!("E{i}")))
                .await;
        }
        let s = state.lock().await;
        let batch = &s.batches["user@example.com"];
        // After hitting the cap, the quiet deadline collapses to ~now so
        // the flusher's next scan drains the batch.
        assert!(
            batch.quiet_deadline <= Instant::now(),
            "size-cap hit should collapse quiet deadline",
        );
    }
}
