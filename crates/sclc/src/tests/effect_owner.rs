//! Tests for the `owner` field on [`Effect`] and the owner-stack plumbing on
//! [`EvalCtx`]. Phase 1 only ever pushes the local owner, so every effect
//! emitted from a single-package program should carry the local deployment
//! QID.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    DiagList, Effect, EvalCtx, InMemoryPackage, PackageId, build_default_finder, compile, eval,
    placeholder_deployment_qid,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn create_effect_carries_local_owner() {
    let mut files = HashMap::new();
    files.insert(
        PathBuf::from("Main.scl"),
        b"import Std/Random\n\
          export let x = Random.Int({ name: \"seed\", min: 0, max: 10 })\n"
            .to_vec(),
    );
    let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
    let finder = build_default_finder(user_pkg);

    let mut diags = DiagList::new();
    let asg = compile(finder, &["Test", "Main"])
        .await
        .expect("compile failed")
        .unpack(&mut diags);
    assert!(
        !diags.has_errors(),
        "unexpected diagnostics: {:?}",
        diags.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );

    let local_owner = placeholder_deployment_qid();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "test", local_owner.clone());
    let _ = eval(&asg, ctx).expect("eval failed");

    let mut saw_create = false;
    while let Ok(effect) = rx.try_recv() {
        assert_eq!(
            effect.owner(),
            Some(&local_owner),
            "every emitted effect should carry the local owner"
        );
        if matches!(effect, Effect::CreateResource { .. }) {
            saw_create = true;
        }
    }
    assert!(saw_create, "expected a CreateResource effect");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn current_owner_falls_back_to_local() {
    let local_owner = placeholder_deployment_qid();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "test", local_owner.clone());
    assert_eq!(ctx.current_owner(), Some(local_owner));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn with_owner_pushes_and_pops() {
    let local_owner = placeholder_deployment_qid();
    let foreign: ids::DeploymentQid =
        "foreign/repo::main@1111111111111111111111111111111111111111.0000000000000000"
            .parse()
            .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "test", local_owner.clone());

    let observed = ctx.with_owner(Some(foreign.clone()), || ctx.current_owner());
    assert_eq!(observed, Some(foreign));
    // After the closure returns, we're back to the local owner.
    assert_eq!(ctx.current_owner(), Some(local_owner));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn with_owner_orphan_scope_yields_no_owner() {
    let local_owner = placeholder_deployment_qid();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "test", local_owner.clone());

    let observed = ctx.with_owner(None, || ctx.current_owner());
    assert_eq!(observed, None);
    // After the closure returns, we're back to the local owner.
    assert_eq!(ctx.current_owner(), Some(local_owner));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn package_owner_falls_back_to_local() {
    let local_owner = placeholder_deployment_qid();
    let foreign: ids::DeploymentQid =
        "foreign/repo::main@2222222222222222222222222222222222222222.0000000000000000"
            .parse()
            .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ctx = EvalCtx::new(tx, "test", local_owner.clone());

    let foreign_pkg = PackageId::from(["foreign", "repo"]);
    ctx.set_package_owner(foreign_pkg.clone(), foreign.clone());

    assert_eq!(ctx.owner_for_package(&foreign_pkg), Some(foreign));
    assert_eq!(
        ctx.owner_for_package(&PackageId::from(["something", "else"])),
        Some(local_owner)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn orphan_package_owner_is_none() {
    let local_owner = placeholder_deployment_qid();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ctx = EvalCtx::new(tx, "test", local_owner);

    let pkg = PackageId::from(["pinned", "by", "hash"]);
    ctx.set_package_orphan(pkg.clone());

    assert_eq!(ctx.owner_for_package(&pkg), None);
}
