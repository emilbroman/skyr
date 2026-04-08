use std::collections::HashMap;

use ids::ResourceId;

use crate::{Effect, Eval, MemSourceRepo, ModuleId, Record, Resource, TrackedValue, Value};

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
    let mut eval = Eval::new(&result, tx, "test");

    // Load existing resources from rdb.json
    for (id, resource) in rdb {
        eval.add_resource(id, resource);
    }

    // Find the main module
    let main_module_id: ModuleId = [dir_name.to_string(), "Main".to_string()]
        .into_iter()
        .collect();

    let tracked_value: TrackedValue = result
        .evaluate(&main_module_id, &eval)
        .unwrap_or_else(|e| panic!("evaluation failed for {dir_name}: {e}"))
        .into_inner();

    // Check exports
    let expected_exports = exports_txt.as_deref().map(|s| s.trim()).unwrap_or("{}");
    let actual_exports = tracked_value.value.to_string();
    assert_eq!(
        actual_exports, expected_exports,
        "exports mismatch for {dir_name}\n  actual: {actual_exports}\n  expected: {expected_exports}"
    );

    // Collect and check effects
    drop(eval); // Close the sender side
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
