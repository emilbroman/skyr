use std::collections::HashMap;
use std::convert::Infallible;
use std::path::Path;

use crate::{Effect, Eval, EvalEnv, ModuleId, SourceRepo, TrackedValue};

/// An in-memory source repository for testing.
struct MemSourceRepo {
    package_id: ModuleId,
    files: HashMap<String, Vec<u8>>,
}

impl SourceRepo for MemSourceRepo {
    type Err = Infallible;

    fn package_id(&self) -> ModuleId {
        self.package_id.clone()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        let key = path.to_string_lossy().replace('\\', "/");
        Ok(self.files.get(&key).cloned())
    }
}

/// Format an effect in compact form.
fn format_effect(effect: &Effect) -> String {
    match effect {
        Effect::CreateResource {
            id,
            inputs,
            dependencies,
        } => {
            let mut s = format!("CreateResource ty={} id={} inputs={}", id.ty, id.id, inputs);
            if !dependencies.is_empty() {
                s.push_str(" deps=[");
                for (i, dep) in dependencies.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{}:{}", dep.ty, dep.id));
                }
                s.push(']');
            }
            s
        }
        Effect::UpdateResource {
            id,
            inputs,
            dependencies,
        } => {
            let mut s = format!("UpdateResource ty={} id={} inputs={}", id.ty, id.id, inputs);
            if !dependencies.is_empty() {
                s.push_str(" deps=[");
                for (i, dep) in dependencies.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{}:{}", dep.ty, dep.id));
                }
                s.push(']');
            }
            s
        }
        Effect::TouchResource {
            id,
            inputs,
            dependencies,
        } => {
            let mut s = format!("TouchResource ty={} id={} inputs={}", id.ty, id.id, inputs);
            if !dependencies.is_empty() {
                s.push_str(" deps=[");
                for (i, dep) in dependencies.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{}:{}", dep.ty, dep.id));
                }
                s.push(']');
            }
            s
        }
    }
}

/// Load fixture files and build a MemSourceRepo for a test case directory.
fn load_fixture(
    dir_name: &str,
) -> (
    MemSourceRepo,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture_dir = format!("{manifest_dir}/src/tests/{dir_name}");
    let fixture_path = std::path::Path::new(&fixture_dir);

    assert!(
        fixture_path.exists(),
        "fixture directory does not exist: {fixture_dir}"
    );

    // Collect .scl files
    let mut files = HashMap::new();
    for entry in std::fs::read_dir(fixture_path).expect("read fixture dir") {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "scl") {
            let filename = path.file_name().unwrap().to_string_lossy().to_string();
            let content = std::fs::read(&path).expect("read .scl file");
            files.insert(filename, content);
        }
    }

    assert!(
        files.contains_key("Main.scl"),
        "fixture {dir_name} must contain Main.scl"
    );

    let source = MemSourceRepo {
        package_id: [dir_name.to_string()].into_iter().collect(),
        files,
    };

    // Load optional expectation files
    let diag_log = std::fs::read_to_string(fixture_path.join("diag.log")).ok();
    let exports_txt = std::fs::read_to_string(fixture_path.join("exports.txt")).ok();
    let effects_log = std::fs::read_to_string(fixture_path.join("effects.log")).ok();

    (source, diag_log, exports_txt, effects_log)
}

/// Run a single test case by directory name.
async fn run_test_case(dir_name: &str) {
    let (source, diag_log, exports_txt, effects_log) = load_fixture(dir_name);

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
    let eval = Eval::new::<MemSourceRepo>(tx, "test");

    // Find the user package and Main module
    let package_id: ModuleId = [dir_name.to_string()].into_iter().collect();
    let main_module_id: ModuleId = [dir_name.to_string(), "Main".to_string()]
        .into_iter()
        .collect();

    // Get Main.scl FileMod from the compiled program
    let main_file_mod = result
        .packages()
        .find(|(name, _)| **name == package_id)
        .and_then(|(_, package)| {
            package
                .modules()
                .find(|(path, _)| path.to_string_lossy() == "Main.scl")
                .map(|(_, file_mod)| file_mod)
        })
        .unwrap_or_else(|| panic!("Main.scl not found in compiled program for {dir_name}"));

    // Build import map by scanning Main.scl import statements
    let imports: HashMap<&str, (ModuleId, &crate::ast::FileMod)> = main_file_mod
        .statements
        .iter()
        .filter_map(|stmt| {
            if let crate::ast::ModStmt::Import(import_stmt) = stmt {
                let alias = import_stmt.as_ref().vars.last()?;
                let import_path: ModuleId = import_stmt
                    .as_ref()
                    .vars
                    .iter()
                    .map(|var| var.as_ref().name.clone())
                    .collect();
                // Resolve: look through all packages for matching module
                let destination = resolve_import(&result, &import_path)?;
                Some((alias.as_ref().name.as_str(), (import_path, destination)))
            } else {
                None
            }
        })
        .collect();

    let env = EvalEnv::new()
        .with_module_id(&main_module_id)
        .with_imports(&imports);

    let tracked_value: TrackedValue = eval
        .eval_file_mod(&env, main_file_mod)
        .unwrap_or_else(|e| panic!("evaluation failed for {dir_name}: {e}"));

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

/// Resolve an import path to a FileMod within the compiled program.
fn resolve_import<'a>(
    program: &'a crate::Program<MemSourceRepo>,
    import_path: &ModuleId,
) -> Option<&'a crate::ast::FileMod> {
    // Find matching package by longest prefix
    let package_name = program
        .packages()
        .map(|(name, _)| name)
        .filter(|name| import_path.starts_with(name))
        .max_by_key(|name| name.len())?;

    let (_, package) = program.packages().find(|(name, _)| *name == package_name)?;

    let suffix = import_path.suffix_after(package_name)?;
    if suffix.is_empty() {
        return None;
    }

    let module_path = suffix
        .iter()
        .cloned()
        .collect::<ModuleId>()
        .to_path_buf_with_extension("scl");

    package
        .modules()
        .find(|(path, _)| **path == module_path)
        .map(|(_, file_mod)| file_mod)
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
test_case!(DiagUndefinedVar);
test_case!(DiagTypeMismatch);
test_case!(RandomInt);
