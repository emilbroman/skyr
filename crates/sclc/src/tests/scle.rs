//! Tests for the SCLE (SCL Expression) format.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{InMemoryPackage, PackageId, Value, build_default_finder, evaluate_scle};

/// Build a finder over an empty user package + the standard library. SCLE
/// adds its synthetic `__Scle__` package on top.
fn finder() -> Arc<dyn crate::PackageFinder> {
    let user_pkg = Arc::new(InMemoryPackage::new(
        PackageId::from(["__ScleTestUser"]),
        HashMap::new(),
    ));
    build_default_finder(user_pkg)
}

fn assert_no_diags(diags: &crate::DiagList) {
    let msgs: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
    assert!(msgs.is_empty(), "unexpected diagnostics: {msgs:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn evaluates_basic_record() {
    let source = r#"
{ hello: Str, n: Int }

{ hello: "world", n: 42 }
"#;
    let result = evaluate_scle(finder(), source).await.unwrap();
    let mut diags = crate::DiagList::new();
    let value = result.unpack(&mut diags).expect("expected a value");
    assert_no_diags(&diags);

    let Value::Record(rec) = &value.value else {
        panic!("expected record, got {:?}", value.value);
    };
    assert_eq!(rec.get("hello"), &Value::Str("world".to_string()));
    assert_eq!(rec.get("n"), &Value::Int(42));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn evaluates_with_imports() {
    // Use a stdlib import to exercise the import-resolution path.
    let source = r#"
import Std/Option

{ result: Int }

{ result: Option.default(42, 0) }
"#;
    let result = evaluate_scle(finder(), source).await.unwrap();
    let mut diags = crate::DiagList::new();
    let value = result.unpack(&mut diags).expect("expected a value");
    assert_no_diags(&diags);

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
    let result = evaluate_scle(finder(), source).await.unwrap();
    let mut diags = crate::DiagList::new();
    let _ = result.unpack(&mut diags);
    assert!(
        diags.has_errors(),
        "expected at least one error diagnostic, got {:?}",
        diags.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn syntax_error_is_diagnosed() {
    let source = "{ this is not valid";
    let result = evaluate_scle(finder(), source).await.unwrap();
    let mut diags = crate::DiagList::new();
    let value = result.unpack(&mut diags);
    assert!(value.is_none(), "expected no value on syntax error");
    assert!(!diags.is_empty(), "expected diagnostics on syntax error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn references_std_package_manifest_type() {
    // Std/Package.Manifest should typecheck against a literal record value.
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
    let result = evaluate_scle(finder(), source).await.unwrap();
    let mut diags = crate::DiagList::new();
    let value = result.unpack(&mut diags).expect("expected a value");
    assert_no_diags(&diags);

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
    let result = evaluate_scle(finder(), source).await.unwrap();
    let mut diags = crate::DiagList::new();
    let value = result.unpack(&mut diags).expect("expected a value");
    assert_no_diags(&diags);

    let Value::Record(rec) = &value.value else {
        panic!("expected record");
    };
    assert_eq!(rec.get("doubled"), &Value::Int(42));
}

// Suppress dead-code warnings on imports we may want to use as the test
// surface grows.
#[allow(dead_code)]
fn _touch(_p: PathBuf) {}
