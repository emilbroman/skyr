use chrono::{TimeZone, Utc};

use crate::{Category, Incident, IncidentId, StatusSummary};

// ── Category ─────────────────────────────────────────────────────────

#[test]
fn category_round_trips_through_str() {
    for c in Category::ALL {
        let parsed: Category = c.as_str().parse().unwrap();
        assert_eq!(parsed, c);
    }
}

#[test]
fn category_severity_ordering() {
    let mut cats = Category::ALL.to_vec();
    cats.sort();
    // BadConfiguration is least severe; Crash is most severe.
    assert_eq!(
        cats,
        vec![
            Category::BadConfiguration,
            Category::CannotProgress,
            Category::InconsistentState,
            Category::SystemError,
            Category::Crash,
        ]
    );
}

#[test]
fn worst_open_category_via_max() {
    let open = [
        Category::CannotProgress,
        Category::SystemError,
        Category::BadConfiguration,
    ];
    let worst = open.iter().copied().max();
    assert_eq!(worst, Some(Category::SystemError));
}

// ── IncidentId ───────────────────────────────────────────────────────

#[test]
fn incident_id_random_is_unique_with_high_probability() {
    let a = IncidentId::new();
    let b = IncidentId::new();
    assert_ne!(a, b);
}

#[test]
fn incident_id_round_trips_through_string() {
    let id = IncidentId::new();
    let s = id.to_string();
    let parsed: IncidentId = s.parse().unwrap();
    assert_eq!(parsed, id);
}

#[test]
fn incident_id_lex_order_matches_creation_order() {
    // ULIDs are constructed with a millisecond-resolution timestamp prefix;
    // the canonical Crockford-base32 form sorts lexicographically with the
    // same order. A second-apart pair therefore strictly orders.
    let earlier = IncidentId::at(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
    let later = IncidentId::at(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 1).unwrap());
    assert!(earlier.to_string() < later.to_string());
}

// ── Incident ─────────────────────────────────────────────────────────

#[test]
fn incident_is_open_reflects_closed_at() {
    let mut inc = Incident {
        id: IncidentId::new(),
        entity_qid: "Org/Repo::env::Std/X:y".into(),
        category: Category::Crash,
        opened_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        closed_at: None,
        last_report_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        report_count: 1,
        summary: "boom".into(),
    };
    assert!(inc.is_open());
    inc.closed_at = Some(Utc.with_ymd_and_hms(2024, 1, 1, 1, 0, 0).unwrap());
    assert!(!inc.is_open());
}

// ── StatusSummary ────────────────────────────────────────────────────

#[test]
fn status_summary_default_open_state() {
    let s = StatusSummary {
        entity_qid: "Org/Repo::main::Std/X:y".into(),
        last_report_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        last_report_succeeded: true,
        open_incident_count: 0,
        worst_open_category: None,
        consecutive_failure_count: 0,
        operational_state: Some("DESIRED".into()),
    };
    assert_eq!(s.open_incident_count, 0);
    assert_eq!(s.worst_open_category, None);
}
