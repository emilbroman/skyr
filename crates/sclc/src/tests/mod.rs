use std::collections::HashMap;

use ids::ResourceId;

use crate::{
    Effect, EvalCtx, MemSourceRepo, ModuleId, PackageId, Record, Resource, SourceRepo,
    TrackedValue, Value,
};

/// Format an effect in compact form.
fn format_effect(effect: &Effect) -> String {
    match effect {
        Effect::CreateResource {
            id,
            inputs,
            dependencies,
            source_trace: _,
        } => {
            let mut s = format!(
                "CreateResource ty={} name={} inputs={}",
                id.typ, id.name, inputs
            );
            if !dependencies.is_empty() {
                s.push_str(" deps=[");
                for (i, dep) in dependencies.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{dep}"));
                }
                s.push(']');
            }
            s
        }
        Effect::UpdateResource {
            id,
            inputs,
            dependencies,
            source_trace: _,
        } => {
            let mut s = format!(
                "UpdateResource ty={} name={} inputs={}",
                id.typ, id.name, inputs
            );
            if !dependencies.is_empty() {
                s.push_str(" deps=[");
                for (i, dep) in dependencies.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{dep}"));
                }
                s.push(']');
            }
            s
        }
        Effect::TouchResource {
            id,
            inputs,
            dependencies,
            source_trace: _,
        } => {
            let mut s = format!(
                "TouchResource ty={} name={} inputs={}",
                id.typ, id.name, inputs
            );
            if !dependencies.is_empty() {
                s.push_str(" deps=[");
                for (i, dep) in dependencies.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{dep}"));
                }
                s.push(']');
            }
            s
        }
    }
}

/// Convert a serde_json::Value into an SCL Value.
fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(ordered_float::NotNan::new(f).expect("NaN in fixture"))
            } else {
                panic!("unsupported number in rdb.json: {n}")
            }
        }
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Object(map) => {
            let mut record = Record::default();
            for (key, val) in map {
                record.insert(key.clone(), json_to_value(val));
            }
            Value::Record(record)
        }
        serde_json::Value::Array(arr) => Value::List(arr.iter().map(json_to_value).collect()),
    }
}

/// Convert a serde_json::Value (object) into an SCL Record.
fn json_to_record(json: &serde_json::Value) -> Record {
    match json_to_value(json) {
        Value::Record(r) => r,
        _ => panic!("expected JSON object for record, got: {json}"),
    }
}

/// Parse rdb.json into a list of (ResourceId, Resource) entries.
fn parse_rdb(json_str: &str) -> Vec<(ResourceId, Resource)> {
    let root: serde_json::Value =
        serde_json::from_str(json_str).expect("rdb.json must be valid JSON");
    let resources_obj = root
        .get("resources")
        .and_then(|v| v.as_object())
        .expect("rdb.json must have a \"resources\" object");

    let mut entries = Vec::new();
    for (resource_type, ids_obj) in resources_obj {
        let ids = ids_obj
            .as_object()
            .unwrap_or_else(|| panic!("resource type {resource_type} must map to an object"));
        for (resource_id, resource_obj) in ids {
            let inputs = resource_obj
                .get("inputs")
                .map(json_to_record)
                .unwrap_or_default();
            let outputs = resource_obj
                .get("outputs")
                .map(json_to_record)
                .unwrap_or_default();
            let markers = resource_obj
                .get("markers")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|m| match m.as_str().expect("marker must be a string") {
                            "Volatile" => crate::Marker::Volatile,
                            "Sticky" => crate::Marker::Sticky,
                            other => panic!("unknown marker: {other}"),
                        })
                        .collect()
                })
                .unwrap_or_default();

            entries.push((
                ResourceId {
                    typ: resource_type.clone(),
                    name: resource_id.clone(),
                },
                Resource {
                    inputs,
                    outputs,
                    dependencies: resource_obj
                        .get("dependencies")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|dep| {
                                    let obj =
                                        dep.as_object().expect("dependency must be an object");
                                    ResourceId {
                                        typ: obj["type"]
                                            .as_str()
                                            .expect("type must be a string")
                                            .to_string(),
                                        name: obj["name"]
                                            .as_str()
                                            .expect("name must be a string")
                                            .to_string(),
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    markers,
                },
            ));
        }
    }
    entries
}

struct Fixture {
    source: MemSourceRepo,
    rdb: Vec<(ResourceId, Resource)>,
    diag_log: Option<String>,
    exports_txt: Option<String>,
    effects_log: Option<String>,
}

/// Load fixture files and build a MemSourceRepo for a test case directory.
fn load_fixture(dir_name: &str) -> Fixture {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture_dir = format!("{manifest_dir}/src/tests/{dir_name}");
    let fixture_path = std::path::Path::new(&fixture_dir);

    assert!(
        fixture_path.exists(),
        "fixture directory does not exist: {fixture_dir}"
    );

    // Collect .scl and other fixture files (recursively, preserving relative paths).
    // Only top-level expectation files are skipped (exports.txt, effects.log, diag.log, rdb.json).
    let mut files = HashMap::new();
    fn collect_fixture_files(
        dir: &std::path::Path,
        base: &std::path::Path,
        files: &mut HashMap<String, Vec<u8>>,
        is_root: bool,
    ) {
        /// Top-level files that are test expectations, not source data.
        const EXPECTATION_FILES: &[&str] = &["exports.txt", "effects.log", "diag.log", "rdb.json"];

        for entry in std::fs::read_dir(dir).expect("read fixture dir") {
            let entry = entry.expect("read dir entry");
            let path = entry.path();
            if path.is_dir() {
                collect_fixture_files(&path, base, files, false);
            } else {
                // Skip known expectation files at the fixture root
                if is_root {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if EXPECTATION_FILES.contains(&name) {
                            continue;
                        }
                    }
                }
                let relative = path.strip_prefix(base).unwrap();
                let key = relative.to_string_lossy().replace('\\', "/");
                let content = std::fs::read(&path).expect("read fixture file");
                files.insert(key, content);
            }
        }
    }
    collect_fixture_files(fixture_path, fixture_path, &mut files, true);

    assert!(
        files.contains_key("Main.scl"),
        "fixture {dir_name} must contain Main.scl"
    );

    let source = MemSourceRepo::new([dir_name.to_string()].into_iter().collect(), files);

    // Load optional expectation files
    let diag_log = std::fs::read_to_string(fixture_path.join("diag.log")).ok();
    let exports_txt = std::fs::read_to_string(fixture_path.join("exports.txt")).ok();
    let effects_log = std::fs::read_to_string(fixture_path.join("effects.log")).ok();

    // Load optional rdb.json
    let rdb = std::fs::read_to_string(fixture_path.join("rdb.json"))
        .map(|s| parse_rdb(&s))
        .unwrap_or_default();

    Fixture {
        source,
        rdb,
        diag_log,
        exports_txt,
        effects_log,
    }
}

/// Run a single test case by directory name.
async fn run_test_case(dir_name: &str) {
    let Fixture {
        source,
        rdb,
        diag_log,
        exports_txt,
        effects_log,
    } = load_fixture(dir_name);

    // Compile
    let result = crate::compile(source)
        .await
        .unwrap_or_else(|e| panic!("compilation failed for {dir_name}: {e}"));

    // Format diagnostics as "ModuleId Span: message"
    let mut actual_diags: Vec<String> = result
        .diags()
        .iter()
        .map(|d| {
            let (module_id, span) = d.locate();
            format!("{module_id} {span}: {d}")
        })
        .collect();
    actual_diags.sort();

    // Expected diagnostics
    let mut expected_diags: Vec<String> = diag_log
        .as_deref()
        .unwrap_or("")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    expected_diags.sort();

    assert_eq!(
        actual_diags, expected_diags,
        "diagnostics mismatch for {dir_name}\n  actual: {actual_diags:#?}\n  expected: {expected_diags:#?}"
    );

    // If there are diagnostic errors, skip evaluation
    if result.diags().has_errors() {
        return;
    }

    // Set up evaluation
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let unit = result.into_inner();
    let mut eval_ctx = EvalCtx::new(tx, "test");

    // Load existing resources from rdb.json
    for (id, resource) in rdb {
        eval_ctx.add_resource(id, resource);
    }

    // Find the main module
    let main_module_id = ModuleId::new(
        PackageId::from([dir_name.to_string()]),
        vec!["Main".to_string()],
    );

    let tracked_value: TrackedValue = unit
        .eval(eval_ctx)
        .unwrap_or_else(|e| panic!("evaluation failed for {dir_name}: {e}"))
        .remove(&main_module_id)
        .unwrap_or_else(|| panic!("main module missing from evaluation results for {dir_name}"));

    // Check exports
    let expected_exports = exports_txt.as_deref().map(|s| s.trim()).unwrap_or("{}");
    let actual_exports = tracked_value.value.to_string();
    assert_eq!(
        actual_exports, expected_exports,
        "exports mismatch for {dir_name}\n  actual: {actual_exports}\n  expected: {expected_exports}"
    );

    // Collect and check effects
    let mut actual_effects = Vec::new();
    while let Ok(effect) = rx.try_recv() {
        actual_effects.push(format_effect(&effect));
    }

    let expected_effects: Vec<String> = effects_log
        .as_deref()
        .unwrap_or("")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    assert_eq!(
        actual_effects, expected_effects,
        "effects mismatch for {dir_name}\n  actual: {actual_effects:#?}\n  expected: {expected_effects:#?}"
    );
}

macro_rules! test_case {
    ($name:ident) => {
        #[allow(non_snake_case)]
        #[tokio::test]
        async fn $name() {
            run_test_case(stringify!($name)).await;
        }
    };
}

test_case!(BasicExport);
test_case!(MultiExport);
test_case!(EmptyModule);
test_case!(ImportModule);
test_case!(TransitiveImport);
test_case!(SelfImport);
test_case!(SelfImportSubdir);
test_case!(DiagUndefinedVar);
test_case!(DiagTypeMismatch);
test_case!(DiagInvalidImport);
test_case!(RandomInt);
test_case!(RandomIntUpdate);
test_case!(RandomIntTouch);

// Std/List
test_case!(ListRange);
test_case!(ListRangeEmpty);
test_case!(ListMap);
test_case!(ListMapInferred);
test_case!(ListMapInferredUntyped);
test_case!(ListAppend);
test_case!(ListConcat);
test_case!(ListFilter);
test_case!(ListFlatMap);
test_case!(ListMapEmpty);

// Std/Num
test_case!(NumToHex);

// Nil
test_case!(NilOptionalCheck);
test_case!(NilUnwrapInfer);

// Std/Option
test_case!(OptionIsNoneIsSome);
test_case!(OptionDefault);
test_case!(OptionUnwrapSome);

// Std/Encoding
test_case!(EncodingToJson);
test_case!(EncodingToJsonRecord);
test_case!(EncodingFromJson);

// Std/Time
test_case!(TimeToISO);
test_case!(TimeUtc);
test_case!(TimeAdd);
test_case!(TimeSubtract);
test_case!(TimeSchedule);
test_case!(TimeScheduleWithRdb);
test_case!(TimeAddMonths);

// Std/Artifact
test_case!(ArtifactFile);
test_case!(ArtifactFileWithMediaType);
test_case!(ArtifactFileWithRdb);

// Std/Container
test_case!(ContainerImage);
test_case!(ContainerPod);
test_case!(ContainerPodEnvCreate);
test_case!(ContainerPodContainerEnvCreate);
test_case!(ContainerPodEnvMergeCreate);
test_case!(ContainerAttachment);
test_case!(ContainerHost);

// Std/DNS
test_case!(DNSARecord);
test_case!(DNSARecordTouch);
test_case!(DNSARecordUpdate);

// Std/Crypto
test_case!(CryptoED25519);
test_case!(CryptoECDSA);
test_case!(CryptoECDSAWithCurve);
test_case!(CryptoRSA);
test_case!(CryptoRSAWithSize);
test_case!(CryptoCertReq);
test_case!(CryptoCertSign);

// Type cast tests
test_case!(TypeCast);
test_case!(TypeCastError);
test_case!(DiagTypeCastOptionalMismatch);

// Recursive globals
test_case!(RecursiveGlobalFn);
test_case!(MutuallyRecursiveFns);
test_case!(ForwardReference);
test_case!(DiagCyclicDependency);

// Diagnostic tests
test_case!(DiagNumToHexWrongType);
test_case!(DiagListMapWrongType);
test_case!(DiagRecordExtraField);

// Let type annotation tests
test_case!(LetTypeAnnotation);
test_case!(LetTypeAnnotationError);

// Indexed access tests
test_case!(DictIndexedAccess);
test_case!(ListIndexedAccess);
test_case!(IndexedAccessOutOfBounds);
test_case!(IndexedAccessMissingKey);
test_case!(IndexedAccessTypeError);

// Named type diagnostics
test_case!(DiagNamedTypeAlias);
test_case!(DiagStructuralType);
test_case!(DiagInferredTypeNotNamed);
test_case!(DiagFnParamAlias);
test_case!(DiagGenericAliasApp);
test_case!(DiagGenericInferConflict);
test_case!(DiagNestedNamedType);
test_case!(DiagRecordFieldNamedType);
test_case!(DiagNamedRecordInFnType);
test_case!(DiagUntypedParam);
test_case!(UntypedParamCheck);

// Path validation
test_case!(PathValid);
test_case!(PathInvalid);
test_case!(PathTraverseFile);

// Optional chaining and nil coalescing
test_case!(OptionalChainNil);
test_case!(OptionalChainSome);
test_case!(NilCoalesceNil);
test_case!(NilCoalesceSome);
test_case!(OptionalChainCoalesce);
test_case!(DiagOptionalChainNonOptional);
test_case!(DiagNilCoalesceNonOptional);
test_case!(NilCoalesceReExportedOptional);

// ═══════════════════════════════════════════════════════════════════════════════
// v2 pipeline fixture tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Run a single test case through the v2 pipeline.
async fn run_test_case_v2(dir_name: &str) {
    use std::path::PathBuf;
    use std::sync::Arc;

    let Fixture {
        source,
        rdb,
        diag_log,
        exports_txt,
        effects_log,
    } = load_fixture(dir_name);

    // Convert MemSourceRepo files into an InMemoryPackage.
    let pkg_id = source.package_id();
    let files: HashMap<PathBuf, Vec<u8>> = source
        .into_files()
        .into_iter()
        .map(|(k, v)| (PathBuf::from(k), v))
        .collect();
    let user_pkg = Arc::new(crate::v2::InMemoryPackage::new(pkg_id.clone(), files));
    let finder = crate::v2::build_default_finder(user_pkg);

    // Compile via v2 pipeline.
    let entry: Vec<&str> = {
        let mut segments: Vec<&str> = pkg_id.as_slice().iter().map(String::as_str).collect();
        segments.push("Main");
        segments
    };

    let result = crate::v2::compile(finder, &entry)
        .await
        .unwrap_or_else(|e| panic!("[v2] compilation failed for {dir_name}: {e}"));

    // Format diagnostics.
    let mut actual_diags: Vec<String> = result
        .diags()
        .iter()
        .map(|d| {
            let (module_id, span) = d.locate();
            format!("{module_id} {span}: {d}")
        })
        .collect();
    actual_diags.sort();

    let mut expected_diags: Vec<String> = diag_log
        .as_deref()
        .unwrap_or("")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    expected_diags.sort();

    assert_eq!(
        actual_diags, expected_diags,
        "[v2] diagnostics mismatch for {dir_name}\n  actual: {actual_diags:#?}\n  expected: {expected_diags:#?}"
    );

    // If there are diagnostic errors, skip evaluation.
    if result.diags().has_errors() {
        return;
    }

    // Set up evaluation.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let asg = result.into_inner();
    let mut eval_ctx = EvalCtx::new(tx, "test");

    for (id, resource) in rdb {
        eval_ctx.add_resource(id, resource);
    }

    let main_module_id = ModuleId::new(pkg_id, vec!["Main".to_string()]);

    let tracked_value: TrackedValue = crate::v2::eval(&asg, eval_ctx)
        .unwrap_or_else(|e| panic!("[v2] evaluation failed for {dir_name}: {e}"))
        .modules
        .remove(&main_module_id)
        .unwrap_or_else(|| {
            panic!("[v2] main module missing from evaluation results for {dir_name}")
        });

    // Check exports.
    let expected_exports = exports_txt.as_deref().map(|s| s.trim()).unwrap_or("{}");
    let actual_exports = tracked_value.value.to_string();
    assert_eq!(
        actual_exports, expected_exports,
        "[v2] exports mismatch for {dir_name}\n  actual: {actual_exports}\n  expected: {expected_exports}"
    );

    // Collect and check effects.
    let mut actual_effects = Vec::new();
    while let Ok(effect) = rx.try_recv() {
        actual_effects.push(format_effect(&effect));
    }

    let expected_effects: Vec<String> = effects_log
        .as_deref()
        .unwrap_or("")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    assert_eq!(
        actual_effects, expected_effects,
        "[v2] effects mismatch for {dir_name}\n  actual: {actual_effects:#?}\n  expected: {expected_effects:#?}"
    );
}

macro_rules! test_case_v2 {
    ($name:ident, $v2_name:ident) => {
        #[allow(non_snake_case)]
        #[tokio::test]
        async fn $v2_name() {
            run_test_case_v2(stringify!($name)).await;
        }
    };
}

mod v2_fixtures {
    use super::*;

    test_case_v2!(BasicExport, BasicExport);
    test_case_v2!(MultiExport, MultiExport);
    test_case_v2!(EmptyModule, EmptyModule);
    test_case_v2!(ImportModule, ImportModule);
    test_case_v2!(TransitiveImport, TransitiveImport);
    test_case_v2!(SelfImport, SelfImport);
    test_case_v2!(SelfImportSubdir, SelfImportSubdir);
    test_case_v2!(DiagUndefinedVar, DiagUndefinedVar);
    test_case_v2!(DiagTypeMismatch, DiagTypeMismatch);
    test_case_v2!(DiagInvalidImport, DiagInvalidImport);
    test_case_v2!(ForwardReference, ForwardReference);

    // Std/List
    test_case_v2!(ListRange, ListRange);
    test_case_v2!(ListRangeEmpty, ListRangeEmpty);
    test_case_v2!(ListMap, ListMap);
    test_case_v2!(ListMapInferred, ListMapInferred);
    test_case_v2!(ListMapInferredUntyped, ListMapInferredUntyped);
    test_case_v2!(ListAppend, ListAppend);
    test_case_v2!(ListConcat, ListConcat);
    test_case_v2!(ListFilter, ListFilter);
    test_case_v2!(ListFlatMap, ListFlatMap);
    test_case_v2!(ListMapEmpty, ListMapEmpty);

    // Std/Num
    test_case_v2!(NumToHex, NumToHex);

    // Nil
    test_case_v2!(NilOptionalCheck, NilOptionalCheck);
    test_case_v2!(NilUnwrapInfer, NilUnwrapInfer);

    // Std/Option
    test_case_v2!(OptionIsNoneIsSome, OptionIsNoneIsSome);
    test_case_v2!(OptionDefault, OptionDefault);
    test_case_v2!(OptionUnwrapSome, OptionUnwrapSome);

    // Std/Encoding
    test_case_v2!(EncodingToJson, EncodingToJson);
    test_case_v2!(EncodingToJsonRecord, EncodingToJsonRecord);
    test_case_v2!(EncodingFromJson, EncodingFromJson);

    // Std/Time
    test_case_v2!(TimeToISO, TimeToISO);
    test_case_v2!(TimeUtc, TimeUtc);
    test_case_v2!(TimeAdd, TimeAdd);
    test_case_v2!(TimeSubtract, TimeSubtract);
    test_case_v2!(TimeSchedule, TimeSchedule);
    test_case_v2!(TimeScheduleWithRdb, TimeScheduleWithRdb);
    test_case_v2!(TimeAddMonths, TimeAddMonths);

    // Resource tests (Artifact, Container, DNS, Crypto, Random)
    test_case_v2!(RandomInt, RandomInt);
    test_case_v2!(RandomIntUpdate, RandomIntUpdate);
    test_case_v2!(RandomIntTouch, RandomIntTouch);
    test_case_v2!(ArtifactFile, ArtifactFile);
    test_case_v2!(ArtifactFileWithMediaType, ArtifactFileWithMediaType);
    test_case_v2!(ArtifactFileWithRdb, ArtifactFileWithRdb);
    test_case_v2!(ContainerImage, ContainerImage);
    test_case_v2!(ContainerPod, ContainerPod);
    test_case_v2!(ContainerPodEnvCreate, ContainerPodEnvCreate);
    test_case_v2!(
        ContainerPodContainerEnvCreate,
        ContainerPodContainerEnvCreate
    );
    test_case_v2!(ContainerPodEnvMergeCreate, ContainerPodEnvMergeCreate);
    test_case_v2!(ContainerAttachment, ContainerAttachment);
    test_case_v2!(ContainerHost, ContainerHost);
    test_case_v2!(DNSARecord, DNSARecord);
    test_case_v2!(DNSARecordTouch, DNSARecordTouch);
    test_case_v2!(DNSARecordUpdate, DNSARecordUpdate);
    test_case_v2!(CryptoED25519, CryptoED25519);
    test_case_v2!(CryptoECDSA, CryptoECDSA);
    test_case_v2!(CryptoECDSAWithCurve, CryptoECDSAWithCurve);
    test_case_v2!(CryptoRSA, CryptoRSA);
    test_case_v2!(CryptoRSAWithSize, CryptoRSAWithSize);
    test_case_v2!(CryptoCertReq, CryptoCertReq);
    test_case_v2!(CryptoCertSign, CryptoCertSign);

    // Type cast tests
    test_case_v2!(TypeCast, TypeCast);
    test_case_v2!(TypeCastError, TypeCastError);
    test_case_v2!(DiagTypeCastOptionalMismatch, DiagTypeCastOptionalMismatch);

    // Recursive globals
    test_case_v2!(RecursiveGlobalFn, RecursiveGlobalFn);
    test_case_v2!(MutuallyRecursiveFns, MutuallyRecursiveFns);
    test_case_v2!(DiagCyclicDependency, DiagCyclicDependency);

    // Diagnostic tests
    test_case_v2!(DiagNumToHexWrongType, DiagNumToHexWrongType);
    test_case_v2!(DiagListMapWrongType, DiagListMapWrongType);
    test_case_v2!(DiagRecordExtraField, DiagRecordExtraField);

    // Let type annotation tests
    test_case_v2!(LetTypeAnnotation, LetTypeAnnotation);
    test_case_v2!(LetTypeAnnotationError, LetTypeAnnotationError);

    // Indexed access tests
    test_case_v2!(DictIndexedAccess, DictIndexedAccess);
    test_case_v2!(ListIndexedAccess, ListIndexedAccess);
    test_case_v2!(IndexedAccessOutOfBounds, IndexedAccessOutOfBounds);
    test_case_v2!(IndexedAccessMissingKey, IndexedAccessMissingKey);
    test_case_v2!(IndexedAccessTypeError, IndexedAccessTypeError);

    // Named type diagnostics
    test_case_v2!(DiagNamedTypeAlias, DiagNamedTypeAlias);
    test_case_v2!(DiagStructuralType, DiagStructuralType);
    test_case_v2!(DiagInferredTypeNotNamed, DiagInferredTypeNotNamed);
    test_case_v2!(DiagFnParamAlias, DiagFnParamAlias);
    test_case_v2!(DiagGenericAliasApp, DiagGenericAliasApp);
    test_case_v2!(DiagGenericInferConflict, DiagGenericInferConflict);
    test_case_v2!(DiagNestedNamedType, DiagNestedNamedType);
    test_case_v2!(DiagRecordFieldNamedType, DiagRecordFieldNamedType);
    test_case_v2!(DiagNamedRecordInFnType, DiagNamedRecordInFnType);
    test_case_v2!(DiagUntypedParam, DiagUntypedParam);
    test_case_v2!(UntypedParamCheck, UntypedParamCheck);

    // Optional chaining and nil coalescing
    test_case_v2!(OptionalChainNil, OptionalChainNil);
    test_case_v2!(OptionalChainSome, OptionalChainSome);
    test_case_v2!(NilCoalesceNil, NilCoalesceNil);
    test_case_v2!(NilCoalesceSome, NilCoalesceSome);
    test_case_v2!(OptionalChainCoalesce, OptionalChainCoalesce);
    test_case_v2!(DiagOptionalChainNonOptional, DiagOptionalChainNonOptional);
    test_case_v2!(DiagNilCoalesceNonOptional, DiagNilCoalesceNonOptional);
    test_case_v2!(NilCoalesceReExportedOptional, NilCoalesceReExportedOptional);

    // Path validation (skipped — requires path hash preloading not yet implemented in v2)
    // test_case_v2!(PathValid, PathValid);
    // test_case_v2!(PathInvalid, PathInvalid);
    // test_case_v2!(PathTraverseFile, PathTraverseFile);
}
