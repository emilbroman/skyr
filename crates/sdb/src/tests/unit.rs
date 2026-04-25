use chrono::{TimeZone, Utc};

use crate::{
    Category, EntityRef, Incident, IncidentFilter, IncidentId, Pagination, ScopeKeys,
    StatusSummary, scope_keys_for_deployment, scope_keys_for_resource,
};

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
        last_error_message: "boom".into(),
        triggering_report_summary: None,
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

// ── Filter / pagination defaults ─────────────────────────────────────

#[test]
fn incident_filter_default_is_empty() {
    let f = IncidentFilter::default();
    assert!(f.category.is_none());
    assert!(!f.open_only);
    assert!(f.since.is_none());
    assert!(f.until.is_none());
}

#[test]
fn pagination_default_is_unlimited() {
    let p = Pagination::default();
    assert_eq!(p.offset, 0);
    assert!(p.limit.is_none());
}

// ── Scope keys ───────────────────────────────────────────────────────

#[test]
fn scope_keys_for_deployment_qid() {
    let qid: ids::DeploymentQid =
        "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
            .parse()
            .unwrap();
    let keys = scope_keys_for_deployment(&qid);
    assert_eq!(keys.org_scope, "MyOrg");
    assert_eq!(keys.repo_scope, "MyOrg/MyRepo");
    assert_eq!(keys.env_scope, "MyOrg/MyRepo::main");
}

#[test]
fn scope_keys_for_resource_qid() {
    let qid: ids::ResourceQid = "MyOrg/MyRepo::main::Std/Random.Int:seed".parse().unwrap();
    let keys = scope_keys_for_resource(&qid);
    assert_eq!(
        keys,
        ScopeKeys {
            org_scope: "MyOrg".into(),
            repo_scope: "MyOrg/MyRepo".into(),
            env_scope: "MyOrg/MyRepo::main".into(),
        }
    );
}

#[test]
fn scope_keys_via_entity_ref_match_typed_helpers() {
    let dep_qid: ids::DeploymentQid =
        "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
            .parse()
            .unwrap();
    let res_qid: ids::ResourceQid = "MyOrg/MyRepo::main::Std/Random.Int:seed".parse().unwrap();

    use crate::Client;
    assert_eq!(
        Client::scope_keys_for(EntityRef::Deployment(&dep_qid)),
        scope_keys_for_deployment(&dep_qid),
    );
    assert_eq!(
        Client::scope_keys_for(EntityRef::Resource(&res_qid)),
        scope_keys_for_resource(&res_qid),
    );
}
