//! Tests for the cross-repo evaluation surface on [`EvalCtx`]: foreign
//! ownership scopes, foreign-resource lookups, and cross-deployment
//! dependency tracking.
//!
//! These exercise the eval-side mechanics independently of the DE — no CDB
//! is needed. End-to-end tests against a real CDB belong in `de`.

use std::collections::BTreeSet;

use ids::{DeploymentQid, ResourceId};

use crate::{Effect, EvalCtx, PackageId, Record, Resource, Value, placeholder_deployment_qid};

fn foreign_qid(suffix: char) -> DeploymentQid {
    let hash: String = std::iter::repeat_n(suffix, 40).collect();
    format!("foreign/repo::main@{hash}.0000000000000000")
        .parse()
        .unwrap()
}

fn record_with(name: &str, value: Value) -> Record {
    let mut r = Record::default();
    r.insert(name.to_string(), value);
    r
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn foreign_global_emits_foreign_owned_create_when_unmaterialised() {
    let local = placeholder_deployment_qid();
    let foreign = foreign_qid('a');
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ctx = EvalCtx::new(tx, "test", local.clone(), crate::placeholder_region());
    let foreign_pkg = PackageId::from(["foreign", "repo"]);
    ctx.set_package_owner(foreign_pkg.clone(), foreign.clone());

    // Simulate the evaluator entering a foreign global expression.
    let inputs = record_with("min", Value::Int(0));
    let result = ctx
        .with_owner(ctx.owner_for_package(&foreign_pkg), || {
            ctx.resource(None, "Std/Random.Int", "seed", &inputs, BTreeSet::new())
        })
        .expect("resource call should succeed");

    // No materialised foreign resource yet → pending output, foreign-owned
    // Create effect emitted.
    assert!(result.is_none(), "expected pending output");
    let effect = rx.try_recv().expect("expected an effect");
    assert!(matches!(effect, Effect::CreateResource { .. }));
    assert_eq!(effect.owner(), Some(&foreign));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn foreign_global_returns_concrete_outputs_when_materialised() {
    let local = placeholder_deployment_qid();
    let foreign = foreign_qid('b');
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ctx = EvalCtx::new(tx, "test", local.clone(), crate::placeholder_region());
    let foreign_pkg = PackageId::from(["foreign", "repo"]);
    ctx.set_package_owner(foreign_pkg.clone(), foreign.clone());

    let inputs = record_with("min", Value::Int(0));
    ctx.add_foreign_resource(
        foreign.clone(),
        ResourceId {
            region: crate::placeholder_region(),
            typ: "Std/Random.Int".into(),
            name: "seed".into(),
        },
        Resource {
            inputs: inputs.clone(),
            outputs: record_with("result", Value::Int(42)),
            dependencies: Vec::new(),
            markers: Default::default(),
        },
    );

    let result = ctx
        .with_owner(ctx.owner_for_package(&foreign_pkg), || {
            ctx.resource(None, "Std/Random.Int", "seed", &inputs, BTreeSet::new())
        })
        .expect("resource call should succeed");

    let outputs = result.expect("expected concrete outputs");
    assert_eq!(outputs.get("result"), &Value::Int(42));

    // A foreign-owned Touch effect is still emitted (the DE drops it) so the
    // dependency is observable on the event stream.
    let effect = rx.try_recv().expect("expected an effect");
    assert!(
        matches!(effect, Effect::TouchResource { .. }),
        "got {effect:?}"
    );
    assert_eq!(effect.owner(), Some(&foreign));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn foreign_resource_with_changed_inputs_yields_pending_and_update() {
    let local = placeholder_deployment_qid();
    let foreign = foreign_qid('c');
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ctx = EvalCtx::new(tx, "test", local.clone(), crate::placeholder_region());
    let foreign_pkg = PackageId::from(["foreign", "repo"]);
    ctx.set_package_owner(foreign_pkg.clone(), foreign.clone());

    let stored_inputs = record_with("min", Value::Int(0));
    ctx.add_foreign_resource(
        foreign.clone(),
        ResourceId {
            region: crate::placeholder_region(),
            typ: "Std/Random.Int".into(),
            name: "seed".into(),
        },
        Resource {
            inputs: stored_inputs,
            outputs: record_with("result", Value::Int(7)),
            dependencies: Vec::new(),
            markers: Default::default(),
        },
    );

    let new_inputs = record_with("min", Value::Int(99));
    let result = ctx
        .with_owner(ctx.owner_for_package(&foreign_pkg), || {
            ctx.resource(None, "Std/Random.Int", "seed", &new_inputs, BTreeSet::new())
        })
        .expect("resource call should succeed");

    assert!(result.is_none(), "input change should yield pending output");
    let effect = rx.try_recv().expect("expected an effect");
    assert!(
        matches!(effect, Effect::UpdateResource { .. }),
        "got {effect:?}"
    );
    assert_eq!(effect.owner(), Some(&foreign));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn foreign_reads_record_cross_deployment_dependencies() {
    let local = placeholder_deployment_qid();
    let foreign = foreign_qid('d');
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ctx = EvalCtx::new(tx, "test", local.clone(), crate::placeholder_region());
    let foreign_pkg = PackageId::from(["foreign", "repo"]);
    ctx.set_package_owner(foreign_pkg.clone(), foreign.clone());

    let inputs = record_with("min", Value::Int(0));
    let _ = ctx
        .with_owner(ctx.owner_for_package(&foreign_pkg), || {
            ctx.resource(None, "Std/Random.Int", "seed", &inputs, BTreeSet::new())
        })
        .expect("resource call should succeed");

    let deps = ctx.take_foreign_dependencies();
    assert_eq!(deps.len(), 1);
    let (env_qid, resource_id) = deps.into_iter().next().unwrap();
    assert_eq!(env_qid, foreign.environment_qid().clone());
    assert_eq!(resource_id.typ, "Std/Random.Int");
    assert_eq!(resource_id.name, "seed");

    // Drained — second call yields empty.
    assert!(ctx.take_foreign_dependencies().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn local_resource_unaffected_by_foreign_scope_outside_it() {
    let local = placeholder_deployment_qid();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "test", local.clone(), crate::placeholder_region());

    let inputs = record_with("min", Value::Int(0));
    let _ = ctx
        .resource(None, "Std/Random.Int", "seed", &inputs, BTreeSet::new())
        .expect("local resource call");

    let effect = rx.try_recv().expect("expected an effect");
    assert!(matches!(effect, Effect::CreateResource { .. }));
    assert_eq!(effect.owner(), Some(&local));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn orphan_package_emits_unowned_create_and_pending_output() {
    let local = placeholder_deployment_qid();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ctx = EvalCtx::new(tx, "test", local, crate::placeholder_region());
    let orphan_pkg = PackageId::from(["pinned", "by", "hash"]);
    ctx.set_package_orphan(orphan_pkg.clone());

    let inputs = record_with("min", Value::Int(0));
    let result = ctx
        .with_owner(ctx.owner_for_package(&orphan_pkg), || {
            ctx.resource(None, "Std/Random.Int", "seed", &inputs, BTreeSet::new())
        })
        .expect("orphan resource call should not error");

    // Orphan emits never have backing state and never resolve to concrete
    // outputs — the importer reads `<pending>` indefinitely.
    assert!(result.is_none(), "orphan reads must be pending");
    let effect = rx.try_recv().expect("expected an orphan effect");
    assert!(
        matches!(effect, Effect::CreateResource { .. }),
        "got {effect:?}"
    );
    assert_eq!(effect.owner(), None, "orphan effect carries no owner");

    // No cross-deployment dependency is recorded for orphan reads — there is
    // no foreign deployment to depend on.
    assert!(ctx.take_foreign_dependencies().is_empty());
}
