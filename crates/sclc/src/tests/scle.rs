//! Tests for the SCLE (SCL Expression) format.
//!
//! SCLE is now a first-class module format: `.scle` files are discovered by
//! the loader alongside `.scl` files and processed through the standard
//! compile pipeline. These tests exercise SCLE modules in-package.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    AsgEvaluator, DiagList, EvalCtx, GlobalKey, InMemoryPackage, PackageId, Value,
    build_default_finder, compile,
};

fn assert_no_diags(diags: &DiagList) {
    let msgs: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
    assert!(msgs.is_empty(), "unexpected diagnostics: {msgs:#?}");
}

/// Compile and evaluate a single-file SCLE module at `<pkg>/Main.scle`.
/// Returns the module value and accumulated diagnostics.
async fn evaluate_scle_main(source: &str) -> (Option<crate::TrackedValue>, DiagList) {
    let pkg_name = "__ScleTestUser";
    let mut files = HashMap::new();
    files.insert(PathBuf::from("Main.scle"), source.as_bytes().to_vec());
    let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from([pkg_name]), files));
    let finder = build_default_finder(user_pkg);

    let mut diags = DiagList::new();
    let asg = match compile(finder, &[pkg_name, "Main"]).await {
        Ok(d) => d.unpack(&mut diags),
        Err(e) => panic!("compile failed: {e}"),
    };
    if diags.has_errors() {
        return (None, diags);
    }
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "scle-test", crate::placeholder_deployment_qid());
    let (_results, env) = AsgEvaluator::new(&asg, ctx)
        .eval()
        .expect("eval should succeed");
    let raw_id = vec![pkg_name.to_string(), "Main".to_string()];
    let value = env.get(&GlobalKey::ModuleValue(raw_id)).cloned();
    (value, diags)
}

/// When only a single expression is provided (no type expression), the
/// body's type is synthesized and evaluation produces its value.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn evaluates_body_only_synthesizes_type() {
    let source = "{ hello: \"world\", n: 42 }\n";
    let (value, diags) = evaluate_scle_main(source).await;
    assert_no_diags(&diags);
    let value = value.expect("expected a value");
    let Value::Record(rec) = &value.value else {
        panic!("expected record, got {:?}", value.value);
    };
    assert_eq!(rec.get("hello"), &Value::Str("world".to_string()));
    assert_eq!(rec.get("n"), &Value::Int(42));
}

/// An SCLE source containing only a type expression (no body) emits a
/// `MissingBody` diagnostic.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn type_only_emits_missing_body_diagnostic() {
    // A function type (`fn(Int) -> Int`) is syntactically a type expression
    // only — it cannot be parsed as an expression — so the parser reaches
    // the "type expression without body" alternative.
    let source = "fn(Int) -> Int\n";
    let (_value, diags) = evaluate_scle_main(source).await;
    let msgs: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("missing body")),
        "expected missing-body diagnostic, got {msgs:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn evaluates_basic_record() {
    let source = r#"
{ hello: Str, n: Int }

{ hello: "world", n: 42 }
"#;
    let (value, diags) = evaluate_scle_main(source).await;
    assert_no_diags(&diags);
    let value = value.expect("expected a value");
    let Value::Record(rec) = &value.value else {
        panic!("expected record, got {:?}", value.value);
    };
    assert_eq!(rec.get("hello"), &Value::Str("world".to_string()));
    assert_eq!(rec.get("n"), &Value::Int(42));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn evaluates_with_imports() {
    let source = r#"
import Std/Option

{ result: Int }

{ result: Option.default(42, 0) }
"#;
    let (value, diags) = evaluate_scle_main(source).await;
    assert_no_diags(&diags);
    let value = value.expect("expected a value");
    let Value::Record(rec) = &value.value else {
        panic!("expected record, got {:?}", value.value);
    };
    assert_eq!(rec.get("result"), &Value::Int(42));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn type_mismatch_is_diagnosed() {
    let source = r#"
{ hello: Str }

{ hello: 42 }
"#;
    let (_value, diags) = evaluate_scle_main(source).await;
    assert!(
        diags.has_errors(),
        "expected at least one error diagnostic, got {:?}",
        diags.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn syntax_error_is_diagnosed() {
    let source = "{ this is not valid";
    let (_value, diags) = evaluate_scle_main(source).await;
    assert!(!diags.is_empty(), "expected diagnostics on syntax error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn references_std_package_manifest_type() {
    let source = r#"
import Std/Package

Package.Manifest

{
    dependencies: #{
        "MyOrg/MyRepo": "main",
        "MyOrg/Other": "tag:v1.0.0",
    },
}
"#;
    let (value, diags) = evaluate_scle_main(source).await;
    assert_no_diags(&diags);
    let value = value.expect("expected a value");
    let Value::Record(rec) = &value.value else {
        panic!("expected record, got {:?}", value.value);
    };
    let Value::Dict(deps) = rec.get("dependencies") else {
        panic!("expected dict for dependencies");
    };
    assert_eq!(
        deps.get(&Value::Str("MyOrg/MyRepo".to_string())),
        Some(&Value::Str("main".to_string()))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn evaluates_with_let_in_body() {
    let source = r#"
{ doubled: Int }

let n = 21;
{ doubled: n + n }
"#;
    let (value, diags) = evaluate_scle_main(source).await;
    assert_no_diags(&diags);
    let value = value.expect("expected a value");
    let Value::Record(rec) = &value.value else {
        panic!("expected record");
    };
    assert_eq!(rec.get("doubled"), &Value::Int(42));
}

/// `.scl` module importing an `.scle` module: `Y` resolves to the body
/// value of `Y.scle` and `Main.y` takes that value directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn scl_imports_scle_as_value() {
    let pkg = "__ScleUserA";
    let mut files = HashMap::new();
    files.insert(
        PathBuf::from("Main.scl"),
        b"import Self/Y\nexport let y = Y".to_vec(),
    );
    files.insert(PathBuf::from("Y.scle"), b"Str\n\"hello\"".to_vec());
    let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from([pkg]), files));
    let finder = build_default_finder(user_pkg);

    let mut diags = DiagList::new();
    let asg = compile(finder, &[pkg, "Main"])
        .await
        .expect("compile should succeed")
        .unpack(&mut diags);
    assert_no_diags(&diags);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "test", crate::placeholder_deployment_qid());
    let (_results, env) = AsgEvaluator::new(&asg, ctx).eval().unwrap();

    let y = env
        .get(&GlobalKey::Global(
            vec![pkg.to_string(), "Main".to_string()],
            "y".to_string(),
        ))
        .expect("Main.y should be evaluated");
    assert_eq!(y.value, Value::Str("hello".to_string()));
}

/// Member access on an SCLE import resolves via ordinary property access on
/// the SCLE module's body value (no global-shortcut).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn scl_accesses_scle_member() {
    let pkg = "__ScleUserB";
    let mut files = HashMap::new();
    files.insert(
        PathBuf::from("Main.scl"),
        b"import Self/Cfg\nexport let g = Cfg.greeting".to_vec(),
    );
    files.insert(
        PathBuf::from("Cfg.scle"),
        b"{ greeting: Str }\n\n{ greeting: \"hi\" }".to_vec(),
    );
    let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from([pkg]), files));
    let finder = build_default_finder(user_pkg);

    let mut diags = DiagList::new();
    let asg = compile(finder, &[pkg, "Main"])
        .await
        .expect("compile should succeed")
        .unpack(&mut diags);
    assert_no_diags(&diags);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "test", crate::placeholder_deployment_qid());
    let (_results, env) = AsgEvaluator::new(&asg, ctx).eval().unwrap();
    let g = env
        .get(&GlobalKey::Global(
            vec![pkg.to_string(), "Main".to_string()],
            "g".to_string(),
        ))
        .expect("Main.g should be evaluated");
    assert_eq!(g.value, Value::Str("hi".to_string()));
}

/// A module existing as both `.scl` and `.scle` in the same package is
/// ambiguous; the loader emits a diagnostic attributed to the import site.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn scl_and_scle_conflict_emits_diagnostic() {
    let pkg = "__ScleUserC";
    let mut files = HashMap::new();
    files.insert(
        PathBuf::from("Main.scl"),
        b"import Self/Foo\nexport let v = Foo".to_vec(),
    );
    files.insert(PathBuf::from("Foo.scl"), b"export let x = 1".to_vec());
    files.insert(PathBuf::from("Foo.scle"), b"Int\n1".to_vec());
    let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from([pkg]), files));
    let finder = build_default_finder(user_pkg);

    let mut diags = DiagList::new();
    let _ = compile(finder, &[pkg, "Main"])
        .await
        .expect("compile should not hard-error")
        .unpack(&mut diags);
    let msgs: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("ambiguous module")),
        "expected an ambiguous-module diagnostic, got {msgs:?}"
    );
}

/// When both `Main.scl` and `Main.scle` exist, the loader returns a fatal
/// error (no import site to attribute the diagnostic to).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn scl_and_scle_conflict_at_entrypoint_is_fatal() {
    let pkg = "__ScleUserD";
    let mut files = HashMap::new();
    files.insert(PathBuf::from("Main.scl"), b"export let x = 1".to_vec());
    files.insert(PathBuf::from("Main.scle"), b"Int\n1".to_vec());
    let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from([pkg]), files));
    let finder = build_default_finder(user_pkg);
    let err = compile(finder, &[pkg, "Main"])
        .await
        .expect_err("expected a compile error");
    let msg = err.to_string();
    assert!(
        msg.contains("ambiguous"),
        "expected ambiguous-module error, got {msg}"
    );
}

/// SCLE as the entrypoint: `compile(pkg/Main)` discovers `Main.scle` and
/// assembles the body value into `ModuleValue`.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn scle_as_entrypoint() {
    let pkg = "__ScleUserE";
    let mut files = HashMap::new();
    files.insert(PathBuf::from("Main.scle"), b"Int\n7".to_vec());
    let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from([pkg]), files));
    let finder = build_default_finder(user_pkg);

    let mut diags = DiagList::new();
    let asg = compile(finder, &[pkg, "Main"])
        .await
        .expect("compile should succeed")
        .unpack(&mut diags);
    assert_no_diags(&diags);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(tx, "test", crate::placeholder_deployment_qid());
    let (_results, env) = AsgEvaluator::new(&asg, ctx).eval().unwrap();
    let val = env
        .get(&GlobalKey::ModuleValue(vec![
            pkg.to_string(),
            "Main".to_string(),
        ]))
        .expect("Main module value should exist");
    assert_eq!(val.value, Value::Int(7));
}
