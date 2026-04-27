//! Integration tests against a live ScyllaDB.
//!
//! These tests share the `sdb` keyspace but isolate themselves by mixing a
//! per-test random suffix into all entity QIDs and scope keys, so they can
//! run in parallel without clobbering each other.
//!
//! They are gated behind both the `scylla-tests` feature and `#[ignore]`.

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::{
    Category, Client, ClientBuilder, CloseIncidentOutcome, IncidentFilter, OpenIncidentOutcome,
    Pagination, StatusSummary,
};

fn test_node() -> String {
    std::env::var("SDB_TEST_NODE").unwrap_or_else(|_| "127.0.0.1:9042".to_string())
}

async fn connect() -> Client {
    ClientBuilder::new()
        .known_node(test_node())
        .build()
        .await
        .expect("connect to scylla")
}

fn unique_suffix() -> String {
    // Use a fresh UUID as a per-test prefix; embed in QIDs and scope keys to
    // keep tests isolated.
    Uuid::new_v4().simple().to_string()
}

/// Build a synthetic deployment-shaped entity QID and matching scope keys.
fn deployment_fixture(suffix: &str) -> (String, String, String, String) {
    let entity_qid = format!(
        "ItOrg{suffix}/ItRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
    );
    let org_scope = format!("ItOrg{suffix}");
    let repo_scope = format!("ItOrg{suffix}/ItRepo");
    let env_scope = format!("ItOrg{suffix}/ItRepo::main");
    (entity_qid, org_scope, repo_scope, env_scope)
}

#[tokio::test]
#[ignore = "requires a live scylla; run with --features scylla-tests -- --ignored"]
async fn status_summary_lifecycle() {
    let client = connect().await;
    let suffix = unique_suffix();
    let (entity_qid, _, _, _) = deployment_fixture(&suffix);

    // First read returns None.
    assert!(client.status_summary(&entity_qid).await.unwrap().is_none());

    // Upsert a fresh summary.
    let now = Utc::now();
    let s = StatusSummary {
        entity_qid: entity_qid.clone(),
        last_report_at: now,
        last_report_succeeded: true,
        open_incident_count: 0,
        worst_open_category: None,
        consecutive_failure_count: 0,
        operational_state: Some("DESIRED".into()),
    };
    client.upsert_status_summary(&s).await.unwrap();
    let got = client.status_summary(&entity_qid).await.unwrap().unwrap();
    assert_eq!(got, s);

    // Update fields.
    let updated = StatusSummary {
        last_report_succeeded: false,
        consecutive_failure_count: 3,
        worst_open_category: Some(Category::SystemError),
        open_incident_count: 1,
        ..s.clone()
    };
    client.upsert_status_summary(&updated).await.unwrap();
    let got = client.status_summary(&entity_qid).await.unwrap().unwrap();
    assert_eq!(got, updated);

    // Delete.
    client.delete_status_summary(&entity_qid).await.unwrap();
    assert!(client.status_summary(&entity_qid).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires a live scylla; run with --features scylla-tests -- --ignored"]
async fn open_incident_lwt_at_most_one_per_pair() {
    let client = connect().await;
    let suffix = unique_suffix();
    let (entity_qid, org, repo, env) = deployment_fixture(&suffix);
    let now = Utc::now();

    // First open succeeds.
    let outcome = client
        .open_incident(&entity_qid, Category::Crash, now, "boom", &org, &repo, &env)
        .await
        .unwrap();
    let opened_id = match outcome {
        OpenIncidentOutcome::Opened(inc) => {
            assert_eq!(inc.category, Category::Crash);
            assert_eq!(inc.entity_qid, entity_qid);
            assert!(inc.is_open());
            assert_eq!(inc.report_count, 1);
            assert_eq!(inc.summary, "boom");
            inc.id
        }
        OpenIncidentOutcome::AlreadyOpen { .. } => panic!("expected Opened"),
    };

    // Second open of the same (entity, category) returns AlreadyOpen.
    let outcome = client
        .open_incident(
            &entity_qid,
            Category::Crash,
            now + Duration::seconds(10),
            "boom-again",
            &org,
            &repo,
            &env,
        )
        .await
        .unwrap();
    match outcome {
        OpenIncidentOutcome::AlreadyOpen { existing_id } => {
            assert_eq!(existing_id, opened_id);
        }
        OpenIncidentOutcome::Opened(_) => panic!("expected AlreadyOpen"),
    }

    // A different category opens a parallel incident.
    let outcome = client
        .open_incident(
            &entity_qid,
            Category::SystemError,
            now + Duration::seconds(20),
            "infra",
            &org,
            &repo,
            &env,
        )
        .await
        .unwrap();
    let other_id = match outcome {
        OpenIncidentOutcome::Opened(inc) => inc.id,
        OpenIncidentOutcome::AlreadyOpen { .. } => panic!("expected Opened"),
    };
    assert_ne!(opened_id, other_id);

    let open_pairs = client
        .list_open_incidents_for_entity(&entity_qid)
        .await
        .unwrap();
    let mut categories: Vec<_> = open_pairs.into_iter().map(|(c, _)| c).collect();
    categories.sort();
    assert_eq!(categories, vec![Category::SystemError, Category::Crash]);
}

#[tokio::test]
#[ignore = "requires a live scylla; run with --features scylla-tests -- --ignored"]
async fn append_and_close_lifecycle() {
    let client = connect().await;
    let suffix = unique_suffix();
    let (entity_qid, org, repo, env) = deployment_fixture(&suffix);

    let opened_at = Utc::now();
    let outcome = client
        .open_incident(
            &entity_qid,
            Category::CannotProgress,
            opened_at,
            "first",
            &org,
            &repo,
            &env,
        )
        .await
        .unwrap();
    let id = match outcome {
        OpenIncidentOutcome::Opened(inc) => inc.id,
        _ => panic!("expected Opened"),
    };

    // Append a couple of failures with distinct messages so the summary
    // captures both.
    let later = opened_at + Duration::seconds(30);
    let updated = client
        .append_failure_to_open_incident(
            id,
            &entity_qid,
            Category::CannotProgress,
            opened_at,
            later,
            5,
            "fifth",
            &org,
            &repo,
            &env,
        )
        .await
        .unwrap()
        .expect("incident exists");
    assert_eq!(updated.report_count, 5);
    assert_eq!(updated.summary, "first\n\nfifth");
    assert_eq!(updated.last_report_at, later);

    // Reports table is the source of truth.
    let mut reports = client.list_reports_for_incident(id).await.unwrap();
    reports.sort_by_key(|r| r.report_at);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].error_message, "first");
    assert_eq!(reports[1].error_message, "fifth");

    // Close.
    let closed_at = later + Duration::seconds(30);
    let outcome = client
        .close_incident(
            &entity_qid,
            Category::CannotProgress,
            closed_at,
            later,
            5,
            &org,
            &repo,
            &env,
        )
        .await
        .unwrap();
    match outcome {
        CloseIncidentOutcome::Closed(inc) => {
            assert_eq!(inc.id, id);
            assert_eq!(inc.closed_at, Some(closed_at));
            assert!(!inc.is_open());
            // Closure does not blank the summary.
            assert_eq!(inc.summary, "first\n\nfifth");
        }
        CloseIncidentOutcome::NotOpen => panic!("expected Closed"),
    }

    // Second close is a no-op.
    let outcome = client
        .close_incident(
            &entity_qid,
            Category::CannotProgress,
            closed_at + Duration::seconds(1),
            later,
            5,
            &org,
            &repo,
            &env,
        )
        .await
        .unwrap();
    matches!(outcome, CloseIncidentOutcome::NotOpen);

    // Recurrence opens a brand-new incident with a fresh id.
    let recur = client
        .open_incident(
            &entity_qid,
            Category::CannotProgress,
            closed_at + Duration::seconds(60),
            "recur",
            &org,
            &repo,
            &env,
        )
        .await
        .unwrap();
    let new_id = match recur {
        OpenIncidentOutcome::Opened(inc) => inc.id,
        _ => panic!("expected Opened"),
    };
    assert_ne!(new_id, id);
}

#[tokio::test]
#[ignore = "requires a live scylla; run with --features scylla-tests -- --ignored"]
async fn listings_filter_and_paginate() {
    let client = connect().await;
    let suffix = unique_suffix();
    let (entity_qid, org, repo, env) = deployment_fixture(&suffix);

    let base = Utc::now();
    // Open four incidents at different times and categories.
    let cats = [
        Category::BadConfiguration,
        Category::CannotProgress,
        Category::SystemError,
        Category::Crash,
    ];
    for (i, cat) in cats.iter().enumerate() {
        let opened_at = base + Duration::seconds(i as i64 * 10);
        let outcome = client
            .open_incident(
                &entity_qid,
                *cat,
                opened_at,
                format!("err-{i}"),
                &org,
                &repo,
                &env,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, OpenIncidentOutcome::Opened(_)));
    }

    // Close the SystemError one.
    client
        .close_incident(
            &entity_qid,
            Category::SystemError,
            base + Duration::seconds(120),
            base + Duration::seconds(60),
            1,
            &org,
            &repo,
            &env,
        )
        .await
        .unwrap();

    // Per-entity listing returns all four.
    let all = client
        .incidents_by_entity(
            &entity_qid,
            &IncidentFilter::default(),
            Pagination::default(),
        )
        .await
        .unwrap();
    assert_eq!(all.len(), 4);

    // openOnly filter excludes the closed one.
    let open_only = client
        .incidents_by_entity(
            &entity_qid,
            &IncidentFilter {
                open_only: true,
                ..Default::default()
            },
            Pagination::default(),
        )
        .await
        .unwrap();
    assert_eq!(open_only.len(), 3);
    assert!(open_only.iter().all(|i| i.closed_at.is_none()));

    // Category filter.
    let only_crash = client
        .incidents_by_entity(
            &entity_qid,
            &IncidentFilter {
                category: Some(Category::Crash),
                ..Default::default()
            },
            Pagination::default(),
        )
        .await
        .unwrap();
    assert_eq!(only_crash.len(), 1);
    assert_eq!(only_crash[0].category, Category::Crash);

    // since/until.
    let mid = client
        .incidents_by_entity(
            &entity_qid,
            &IncidentFilter {
                since: Some(base + Duration::seconds(15)),
                until: Some(base + Duration::seconds(25)),
                ..Default::default()
            },
            Pagination::default(),
        )
        .await
        .unwrap();
    assert_eq!(mid.len(), 1);
    assert_eq!(mid[0].category, Category::SystemError);

    // Pagination: clustering order is opened_at DESC.
    let page1 = client
        .incidents_by_entity(
            &entity_qid,
            &IncidentFilter::default(),
            Pagination {
                offset: 0,
                limit: Some(2),
            },
        )
        .await
        .unwrap();
    let page2 = client
        .incidents_by_entity(
            &entity_qid,
            &IncidentFilter::default(),
            Pagination {
                offset: 2,
                limit: Some(2),
            },
        )
        .await
        .unwrap();
    assert_eq!(page1.len(), 2);
    assert_eq!(page2.len(), 2);
    let mut combined: Vec<_> = page1.iter().chain(page2.iter()).map(|i| i.id).collect();
    combined.sort();
    let mut all_ids: Vec<_> = all.iter().map(|i| i.id).collect();
    all_ids.sort();
    assert_eq!(combined, all_ids);

    // Scope listings see the same incidents (filter to suffix-matching scope).
    let by_org = client
        .incidents_by_org(&org, &IncidentFilter::default(), Pagination::default())
        .await
        .unwrap();
    assert_eq!(by_org.len(), 4);

    let by_repo = client
        .incidents_by_repo(&repo, &IncidentFilter::default(), Pagination::default())
        .await
        .unwrap();
    assert_eq!(by_repo.len(), 4);

    let by_env = client
        .incidents_by_env(&env, &IncidentFilter::default(), Pagination::default())
        .await
        .unwrap();
    assert_eq!(by_env.len(), 4);
}

#[tokio::test]
#[ignore = "requires a live scylla; run with --features scylla-tests -- --ignored"]
async fn get_incident_returns_none_for_unknown_id() {
    let client = connect().await;
    let unknown = crate::IncidentId::new();
    assert!(client.get_incident(unknown).await.unwrap().is_none());
}
