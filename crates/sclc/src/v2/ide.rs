use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{
    CompletionCandidate, Cursor, CursorInfo, ModuleId, PackageId, Position, TypeChecker, TypeEnv,
    ast, parse_file_mod_with_cursor,
};

use super::Asg;

/// Query cursor information (hover, go-to-definition, references) at a
/// position in a module.
///
/// The `source` should be the current editor content for the file (which may
/// differ from the version the ASG was built from). This function re-parses
/// the source with a cursor, then type-checks against the ASG's context to
/// populate `CursorInfo`.
pub fn cursor_info(
    asg: &Asg,
    module_id: &ModuleId,
    source: &str,
    position: Position,
) -> Arc<Mutex<CursorInfo>> {
    let cursor = Cursor::new(position);
    let cursor_info = cursor.info();

    // Parse with cursor attached
    let diagnosed = parse_file_mod_with_cursor(source, module_id, Some(cursor.clone()));
    let file_mod = diagnosed.into_inner();

    // Build module map and checker directly from the ASG.
    let modules: HashMap<ModuleId, ast::FileMod> = asg
        .modules()
        .map(|mn| (mn.module_id.clone(), mn.file_mod.clone()))
        .collect();
    let package_names: Vec<PackageId> = asg.packages().keys().cloned().collect();
    let checker = TypeChecker::from_modules(&modules, package_names);

    let type_env = TypeEnv::new().with_module_id(module_id).with_cursor(cursor);
    let _ = checker.check_file_mod(&type_env, &file_mod);

    cursor_info
}

/// Query completion candidates at a position in a module.
pub fn completions(
    asg: &Asg,
    module_id: &ModuleId,
    source: &str,
    position: Position,
) -> Vec<CompletionCandidate> {
    let info = cursor_info(asg, module_id, source, position);
    let locked = info.lock().unwrap();
    locked.completion_candidates.clone()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::v2::{InMemoryPackage, build_default_finder, compile};
    use crate::{ModuleId, PackageId, Position};

    #[tokio::test]
    async fn cursor_info_resolves_variable_type() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"export let x = 42".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        // Position cursor on "x" (line 1, col 12 — 1-based)
        let info = super::cursor_info(&asg, &module_id, "export let x = 42", Position::new(1, 12));
        let locked = info.lock().unwrap();
        assert!(
            locked.ty.is_some(),
            "expected a type for the variable at cursor"
        );
    }

    #[tokio::test]
    async fn completions_returns_candidates() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"let foo = 1\nlet bar = foo".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        let candidates = super::completions(
            &asg,
            &module_id,
            "let foo = 1\nlet bar = fo",
            Position::new(2, 13),
        );
        // Should have at least "foo" as a completion candidate
        let names: Vec<_> = candidates
            .iter()
            .filter_map(|c| match c {
                crate::CompletionCandidate::Var(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            names.contains(&"foo"),
            "expected 'foo' in completions, got: {names:?}"
        );
    }
}
