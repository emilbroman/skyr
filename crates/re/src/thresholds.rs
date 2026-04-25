//! Per-category threshold rules and a worker-local sliding-window tracker
//! for failures that have not yet opened an incident.
//!
//! Threshold rules answer the question: given the next failure report for
//! `(entity, category)`, has the entity now produced enough same-category
//! failures within a configured window to warrant opening an incident?
//!
//! The tracker is **not** persisted. A worker restart drops the in-flight
//! pre-open counts, which only delays opening an incident — it never causes
//! incorrect counting because:
//!
//! - Incidents themselves are durable in SDB; this tracker only governs the
//!   pre-open accumulation phase.
//! - At-least-once redelivery may resubmit reports the worker already
//!   accounted for. Re-counting the same report only makes the threshold
//!   trigger sooner, which is acceptable; it cannot create duplicate incidents
//!   because the SDB LWT on the open-slot ensures at-most-one-open per
//!   `(entity, category)` regardless of how many times we cross the threshold.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rq::IncidentCategory;

/// Threshold rule for a single category.
#[derive(Clone, Copy, Debug)]
pub struct ThresholdRule {
    /// Minimum number of failure reports observed within `window` for the
    /// threshold to trip. `Crash` uses `1` (hair-trigger).
    pub min_reports: u32,
    /// Sliding window. Failure reports older than this are evicted before the
    /// count is taken.
    pub window: Duration,
}

impl ThresholdRule {
    pub const fn new(min_reports: u32, window: Duration) -> Self {
        Self {
            min_reports,
            window,
        }
    }
}

/// Threshold configuration for all categories. Defaults are conservative —
/// `Crash` is hair-trigger; the rest require multiple reports within an
/// interval. Operators may override via env vars (see [`ThresholdConfig::from_env`]).
#[derive(Clone, Debug)]
pub struct ThresholdConfig {
    pub bad_configuration: ThresholdRule,
    pub cannot_progress: ThresholdRule,
    pub inconsistent_state: ThresholdRule,
    pub system_error: ThresholdRule,
    pub crash: ThresholdRule,
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            // Bad configuration is producer-side classified and rarely
            // self-corrects; require sustained signal so a single transient
            // mis-parse doesn't page everyone.
            bad_configuration: ThresholdRule::new(5, Duration::from_secs(5 * 60)),
            // Cannot progress and inconsistent state are mid-severity — a few
            // reports within a short window.
            cannot_progress: ThresholdRule::new(5, Duration::from_secs(5 * 60)),
            inconsistent_state: ThresholdRule::new(3, Duration::from_secs(2 * 60)),
            // System error is a Skyr-internal failure; we want to know fast,
            // but not on a single transient blip.
            system_error: ThresholdRule::new(3, Duration::from_secs(60)),
            // Crash means user-visible downtime: hair-trigger.
            crash: ThresholdRule::new(1, Duration::from_secs(60 * 60)),
        }
    }
}

impl ThresholdConfig {
    pub fn rule_for(&self, category: IncidentCategory) -> ThresholdRule {
        match category {
            IncidentCategory::BadConfiguration => self.bad_configuration,
            IncidentCategory::CannotProgress => self.cannot_progress,
            IncidentCategory::InconsistentState => self.inconsistent_state,
            IncidentCategory::SystemError => self.system_error,
            IncidentCategory::Crash => self.crash,
        }
    }

    /// Reads optional env-var overrides:
    ///
    /// - `RE_THRESHOLD_<CATEGORY>_MIN` (u32)
    /// - `RE_THRESHOLD_<CATEGORY>_WINDOW_SECS` (u64)
    ///
    /// where `<CATEGORY>` is one of `BAD_CONFIGURATION`, `CANNOT_PROGRESS`,
    /// `INCONSISTENT_STATE`, `SYSTEM_ERROR`, `CRASH`. Missing or unparseable
    /// values fall back to the default for that field.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        cfg.bad_configuration = override_rule(cfg.bad_configuration, "BAD_CONFIGURATION");
        cfg.cannot_progress = override_rule(cfg.cannot_progress, "CANNOT_PROGRESS");
        cfg.inconsistent_state = override_rule(cfg.inconsistent_state, "INCONSISTENT_STATE");
        cfg.system_error = override_rule(cfg.system_error, "SYSTEM_ERROR");
        cfg.crash = override_rule(cfg.crash, "CRASH");
        cfg
    }
}

fn override_rule(default: ThresholdRule, name: &str) -> ThresholdRule {
    let min = std::env::var(format!("RE_THRESHOLD_{name}_MIN"))
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|v| *v >= 1)
        .unwrap_or(default.min_reports);
    let window_secs = std::env::var(format!("RE_THRESHOLD_{name}_WINDOW_SECS"))
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default.window.as_secs());
    ThresholdRule::new(min, Duration::from_secs(window_secs))
}

/// Bookkeeping for failure reports that have not yet opened an incident.
///
/// Failures are recorded keyed on `(entity_qid, category)`. The tracker holds
/// only the timestamps of recent failures; on every observation, expired
/// entries (older than the rule's window) are evicted. `record_and_check`
/// returns `true` exactly when adding the new failure pushes the count to
/// `>= rule.min_reports` *for the first time* — i.e. when the threshold has
/// just tripped.
///
/// Once the threshold trips, the entry is cleared so subsequent failures for
/// the now-open incident are handled by the bump path, not by re-tripping.
#[derive(Default)]
pub struct ThresholdTracker {
    inner: HashMap<TrackerKey, VecDeque<DateTime<Utc>>>,
}

#[derive(Clone, Hash, Eq, PartialEq, Debug)]
struct TrackerKey {
    entity_qid: String,
    category: IncidentCategory,
}

impl ThresholdTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a failure report and returns `true` if the threshold just
    /// tripped — meaning the caller should attempt to open a new incident.
    ///
    /// `now` is the current wall-clock time (used to expire stale entries
    /// against `rule.window`); `report_at` is the timestamp the producer
    /// stamped on the report. Both are typically very close, but we use the
    /// report timestamp to age the entry so out-of-order or backlog deliveries
    /// behave intuitively.
    pub fn record_and_check(
        &mut self,
        entity_qid: &str,
        category: IncidentCategory,
        rule: ThresholdRule,
        report_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> bool {
        let key = TrackerKey {
            entity_qid: entity_qid.to_string(),
            category,
        };
        let deque = self.inner.entry(key.clone()).or_default();

        let window = chrono::Duration::from_std(rule.window)
            .unwrap_or_else(|_| chrono::Duration::seconds(i64::MAX / 1000));
        let cutoff = now - window;
        while let Some(front) = deque.front() {
            if *front < cutoff {
                deque.pop_front();
            } else {
                break;
            }
        }

        deque.push_back(report_at);

        if deque.len() as u32 >= rule.min_reports {
            // Clear the entry: from this point onward, failures of the same
            // category go through the "bump existing open incident" path
            // until the incident closes. Forgetting the timestamps prevents a
            // future close+reopen from inheriting stale pre-open history.
            self.inner.remove(&key);
            true
        } else {
            false
        }
    }

    /// Drop all bookkeeping for an entity. Called on success reports (which
    /// reset the producer's consecutive-failure run) and on terminal reports
    /// (which delete the entity entirely).
    pub fn forget_entity(&mut self, entity_qid: &str) {
        self.inner.retain(|k, _| k.entity_qid != entity_qid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn defaults_have_crash_at_one_report() {
        let cfg = ThresholdConfig::default();
        assert_eq!(cfg.rule_for(IncidentCategory::Crash).min_reports, 1);
    }

    #[test]
    fn defaults_require_more_than_one_for_other_categories() {
        let cfg = ThresholdConfig::default();
        for c in [
            IncidentCategory::BadConfiguration,
            IncidentCategory::CannotProgress,
            IncidentCategory::InconsistentState,
            IncidentCategory::SystemError,
        ] {
            assert!(
                cfg.rule_for(c).min_reports > 1,
                "expected category {c:?} to require >1 reports, got {}",
                cfg.rule_for(c).min_reports
            );
        }
    }

    #[test]
    fn crash_trips_on_first_report() {
        let mut tracker = ThresholdTracker::new();
        let rule = ThresholdRule::new(1, Duration::from_secs(60));
        let now = ts("2026-01-01T00:00:00Z");
        assert!(tracker.record_and_check(
            "Org/Repo::env@dep.0000000000000001",
            IncidentCategory::Crash,
            rule,
            now,
            now,
        ));
    }

    #[test]
    fn three_within_window_trips_on_third() {
        let mut tracker = ThresholdTracker::new();
        let rule = ThresholdRule::new(3, Duration::from_secs(60));
        let entity = "Org/Repo::env@dep.0000000000000001";
        let t0 = ts("2026-01-01T00:00:00Z");
        let t1 = ts("2026-01-01T00:00:10Z");
        let t2 = ts("2026-01-01T00:00:20Z");
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t0, t0));
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t1, t1));
        assert!(tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t2, t2));
    }

    #[test]
    fn window_evicts_older_reports() {
        let mut tracker = ThresholdTracker::new();
        let rule = ThresholdRule::new(3, Duration::from_secs(30));
        let entity = "Org/Repo::env@dep.0000000000000001";
        let t0 = ts("2026-01-01T00:00:00Z");
        let t1 = ts("2026-01-01T00:00:20Z");
        let t2 = ts("2026-01-01T00:01:00Z"); // 60s after t0; t0 expired.
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t0, t0));
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t1, t1));
        // t0 is expired by t2-t2-window, t1 is also expired (40s before t2);
        // only t2 remains in the window after eviction.
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t2, t2));
    }

    #[test]
    fn trip_clears_entry_so_next_failure_does_not_retrip() {
        let mut tracker = ThresholdTracker::new();
        let rule = ThresholdRule::new(2, Duration::from_secs(60));
        let entity = "Org/Repo::env@dep.0000000000000001";
        let t0 = ts("2026-01-01T00:00:00Z");
        let t1 = ts("2026-01-01T00:00:01Z");
        let t2 = ts("2026-01-01T00:00:02Z");
        assert!(!tracker.record_and_check(entity, IncidentCategory::CannotProgress, rule, t0, t0));
        assert!(tracker.record_and_check(entity, IncidentCategory::CannotProgress, rule, t1, t1));
        // Now an open incident exists; the next failure should *not* trip
        // again — the caller is expected to route to "bump existing incident".
        assert!(!tracker.record_and_check(entity, IncidentCategory::CannotProgress, rule, t2, t2));
    }

    #[test]
    fn forget_entity_drops_pre_open_state() {
        let mut tracker = ThresholdTracker::new();
        let rule = ThresholdRule::new(3, Duration::from_secs(60));
        let entity = "Org/Repo::env@dep.0000000000000001";
        let t0 = ts("2026-01-01T00:00:00Z");
        let t1 = ts("2026-01-01T00:00:10Z");
        let t2 = ts("2026-01-01T00:00:20Z");
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t0, t0));
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t1, t1));
        tracker.forget_entity(entity);
        // After forgetting, the third failure starts a fresh count and does
        // not trip yet.
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t2, t2));
    }

    #[test]
    fn entries_are_independent_per_category() {
        let mut tracker = ThresholdTracker::new();
        let rule = ThresholdRule::new(2, Duration::from_secs(60));
        let entity = "Org/Repo::env@dep.0000000000000001";
        let t0 = ts("2026-01-01T00:00:00Z");
        let t1 = ts("2026-01-01T00:00:01Z");
        // Different category: each only sees one occurrence.
        assert!(!tracker.record_and_check(entity, IncidentCategory::SystemError, rule, t0, t0));
        assert!(!tracker.record_and_check(entity, IncidentCategory::CannotProgress, rule, t1, t1));
    }

    #[test]
    fn entries_are_independent_per_entity() {
        let mut tracker = ThresholdTracker::new();
        let rule = ThresholdRule::new(2, Duration::from_secs(60));
        let t0 = ts("2026-01-01T00:00:00Z");
        let t1 = ts("2026-01-01T00:00:01Z");
        assert!(!tracker.record_and_check(
            "Org/Repo::env@dep.0000000000000001",
            IncidentCategory::SystemError,
            rule,
            t0,
            t0
        ));
        assert!(!tracker.record_and_check(
            "Org/Repo::env@dep.0000000000000002",
            IncidentCategory::SystemError,
            rule,
            t1,
            t1
        ));
    }
}
