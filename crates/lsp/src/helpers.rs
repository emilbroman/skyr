use std::path::PathBuf;

use sclc::{ModuleId, Program, SourceRepo, TypeChecker, TypeEnv};

use crate::overlay::OverlaySource;

/// Find a module (module ID + FileMod) by file path in the compiled program.
pub fn find_module_by_path<'a, S>(
    program: &'a Program<OverlaySource<S>>,
    root_path: &Option<PathBuf>,
    path: &std::path::Path,
) -> Option<(ModuleId, &'a sclc::FileMod)> {
    let root = root_path.as_deref().unwrap_or(std::path::Path::new("."));

    for (package_id, package) in program.packages() {
        for (module_path, file_mod) in package.modules() {
            let full_path = root.join(module_path);
            if full_path == path {
                let module_id = package_module_id(package_id, module_path);
                return Some((module_id, file_mod));
            }
        }
    }
    None
}

/// Find a FileMod by matching its ModuleId.
pub fn find_file_mod_in_program<'a, S>(
    program: &'a Program<OverlaySource<S>>,
    target_module_id: &ModuleId,
) -> Option<&'a sclc::FileMod> {
    for (package_id, package) in program.packages() {
        for (module_path, file_mod) in package.modules() {
            let mid = package_module_id(package_id, module_path);
            if mid == *target_module_id {
                return Some(file_mod);
            }
        }
    }
    None
}

/// Build a full ModuleId from package ID + module file path.
pub fn package_module_id(package_id: &ModuleId, module_path: &std::path::Path) -> ModuleId {
    let mut segments: Vec<String> = package_id.as_slice().to_vec();
    if let Some(parent) = module_path.parent() {
        for component in parent.components() {
            if let std::path::Component::Normal(part) = component {
                segments.push(part.to_string_lossy().into_owned());
            }
        }
    }
    if let Some(stem) = module_path.file_stem() {
        segments.push(stem.to_string_lossy().into_owned());
    }
    ModuleId::new(segments)
}

/// Convert a module ID back to a file path.
pub fn module_id_to_path(root_path: &std::path::Path, module_id: &ModuleId) -> PathBuf {
    let segments = module_id.as_slice();
    if segments.len() < 3 {
        return root_path.to_path_buf();
    }

    let file_segments = &segments[2..];
    let mut path = root_path.to_path_buf();
    for (i, segment) in file_segments.iter().enumerate() {
        if i == file_segments.len() - 1 {
            path.push(format!("{}.scl", segment));
        } else {
            path.push(segment);
        }
    }
    path
}

/// Get the type of a variable by running the type checker.
pub fn get_var_type<S: SourceRepo>(
    program: &Program<OverlaySource<S>>,
    module_id: &ModuleId,
    file_mod: &sclc::FileMod,
    var_name: &str,
) -> Option<sclc::Type> {
    let globals = file_mod.find_globals();
    let checker = TypeChecker::new(program);
    let imports = checker.find_imports(file_mod);
    let env = TypeEnv::new()
        .with_module_id(module_id)
        .with_globals(&globals)
        .with_imports(&imports);

    // Check globals: type-check the global's expression.
    if let Some(global_expr) = globals.get(var_name)
        && let Ok(diagnosed) = checker.check_expr(&env, global_expr, None)
    {
        return Some(diagnosed.into_inner());
    }

    // Check imports: type-check the imported module.
    if let Some((_, Some(import_file_mod))) = imports.get(var_name) {
        let import_env = TypeEnv::new().with_module_id(module_id);
        if let Ok(diagnosed) = checker.check_file_mod(&import_env, import_file_mod) {
            return Some(diagnosed.into_inner());
        }
    }

    None
}
