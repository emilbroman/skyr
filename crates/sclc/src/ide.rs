use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::asg::ModuleBody;
use crate::{
    AsgChecker, CompletionCandidate, Cursor, CursorInfo, ModuleId, PackageId, Position,
    TypeChecker, TypeEnv, ast, parse_file_mod_with_cursor, parse_scle_with_cursor,
    resolve_cursor_refs,
};

use super::Asg;

/// Query cursor information (hover, go-to-definition, references) at a
/// position in a module.
///
/// The `source` should be the current editor content for the file (which may
/// differ from the version the ASG was built from). This function re-parses
/// the source with a cursor, then resolves declaration/reference edges via
/// the loader's AST walk, and finally type-checks for type info and
/// completions.
///
/// If the module is registered as `.scle` in the ASG, the source is parsed
/// as a SCLE module and only its imports / type expression / body expression
/// are checked.
pub fn cursor_info(
    asg: &Asg,
    module_id: &ModuleId,
    source: &str,
    position: Position,
) -> Arc<Mutex<CursorInfo>> {
    let cursor = Cursor::new(position);
    let cursor_info = cursor.info();

    // Detect SCLE-vs-SCL by consulting the ASG. If the module isn't in the
    // ASG (e.g. the workspace failed to load it), fall back to SCL parsing.
    let raw_id: Vec<String> = module_id
        .package
        .as_slice()
        .iter()
        .cloned()
        .chain(module_id.path.iter().cloned())
        .collect();
    let is_scle = asg
        .module(&raw_id)
        .map(|mn| mn.body.is_scle())
        .unwrap_or(false);

    let pkg_id = asg
        .module(&raw_id)
        .map(|mn| mn.package_id.clone())
        .unwrap_or_default();

    // Parse with cursor and resolve declaration/reference edges.
    if is_scle {
        let diagnosed = parse_scle_with_cursor(source, module_id, Some(cursor.clone()));
        if let Some(scle_mod) = diagnosed.into_inner() {
            // Resolve cursor refs via the loader's scope-aware AST walk.
            resolve_cursor_refs(asg, &raw_id, &pkg_id, &ModuleBody::Scle(scle_mod.clone()));

            // Type-check for type info and completions.
            let modules: HashMap<ModuleId, ast::FileMod> = asg
                .modules()
                .filter_map(|mn| {
                    mn.body
                        .as_file_mod()
                        .map(|fm| (mn.module_id.clone(), fm.clone()))
                })
                .collect();
            let package_names: Vec<PackageId> = asg.packages().keys().cloned().collect();
            let checker = TypeChecker::from_modules(&modules, package_names);

            let mut asg_checker = AsgChecker::new(asg);
            let _ = asg_checker.check();
            let ge = asg_checker.into_global_type_env();

            let type_env = TypeEnv::new(&ge)
                .with_module_id(module_id)
                .with_raw_module_id(&raw_id)
                .with_cursor(cursor);

            for import in &scle_mod.imports {
                let stmt = ast::ModStmt::Import(import.clone());
                let _ = checker.check_stmt(&type_env, &stmt);
            }
            match (&scle_mod.type_expr, &scle_mod.body) {
                (Some(type_expr), Some(body)) => {
                    let expected = checker.resolve_type_expr(&type_env, type_expr).into_inner();
                    let _ = checker.check_expr(&type_env, body, Some(&expected));
                }
                (Some(type_expr), None) => {
                    let _ = checker.resolve_type_expr(&type_env, type_expr);
                }
                (None, Some(body)) => {
                    let _ = checker.check_expr(&type_env, body, None);
                }
                (None, None) => {}
            }
        }
    } else {
        let diagnosed = parse_file_mod_with_cursor(source, module_id, Some(cursor.clone()));
        let file_mod = diagnosed.into_inner();

        // Resolve cursor refs via the loader's scope-aware AST walk.
        resolve_cursor_refs(asg, &raw_id, &pkg_id, &ModuleBody::File(file_mod.clone()));

        // Type-check for type info and completions.
        let modules: HashMap<ModuleId, ast::FileMod> = asg
            .modules()
            .filter_map(|mn| {
                mn.body
                    .as_file_mod()
                    .map(|fm| (mn.module_id.clone(), fm.clone()))
            })
            .collect();
        let package_names: Vec<PackageId> = asg.packages().keys().cloned().collect();
        let checker = TypeChecker::from_modules(&modules, package_names);

        let mut asg_checker = AsgChecker::new(asg);
        let _ = asg_checker.check();
        let ge = asg_checker.into_global_type_env();

        let type_env = TypeEnv::new(&ge)
            .with_module_id(module_id)
            .with_raw_module_id(&raw_id)
            .with_cursor(cursor);

        let _ = checker.check_file_mod(&type_env, &file_mod);
    }

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

    use crate::{InMemoryPackage, build_default_finder, compile};
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
    async fn cursor_info_resolves_imported_variable_type() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Lib.scl"), b"export let answer = 42".to_vec());
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nlet x = Lib.answer".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        // Cursor on `answer` in `Lib.answer` (line 2, col 14)
        let info = super::cursor_info(
            &asg,
            &module_id,
            "import Test/Lib\nlet x = Lib.answer",
            Position::new(2, 14),
        );
        let locked = info.lock().unwrap();
        assert!(
            locked.ty.is_some(),
            "expected a type for the imported variable at cursor"
        );
    }

    #[tokio::test]
    async fn cursor_info_lsp_like_main_entry() {
        // Mirrors the LSP setup: entry is always "Main", the file being
        // hovered (B.scl) is reached via Main importing B.
        let mut files = HashMap::new();
        files.insert(PathBuf::from("A.scl"), b"export let x = 123".to_vec());
        files.insert(
            PathBuf::from("B.scl"),
            b"import Self/A\n\nlet first: Str = A.x\nlet second = A.x\n".to_vec(),
        );
        files.insert(PathBuf::from("Main.scl"), b"import Self/B\n".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["B".to_string()]);
        let src = "import Self/A\n\nlet first: Str = A.x\nlet second = A.x\n";

        let info = super::cursor_info(&asg, &module_id, src, Position::new(4, 5));
        let locked = info.lock().unwrap();
        let ty = locked.ty.clone().expect("expected a type for 'second'");
        let printed = ty.to_string();
        eprintln!("LSP-like: type = {printed} (debug: {ty:?})");
        assert!(
            !printed.contains("Never"),
            "expected non-Never display for 'second' (= A.x), got: {printed}"
        );
        assert!(
            printed.contains("Int"),
            "expected Int in display for 'second' (= A.x), got: {printed}"
        );
    }

    #[tokio::test]
    async fn cursor_info_resolves_let_with_imported_rhs() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("A.scl"), b"export let x = 123".to_vec());
        files.insert(
            PathBuf::from("B.scl"),
            b"import Self/A\n\nlet first: Str = A.x\nlet second = A.x\n".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "B"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["B".to_string()]);
        let src = "import Self/A\n\nlet first: Str = A.x\nlet second = A.x\n";

        // Cursor on `second` identifier (line 4, col 5)
        let info = super::cursor_info(&asg, &module_id, src, Position::new(4, 5));
        let locked = info.lock().unwrap();
        let ty = locked.ty.clone().expect("expected a type for 'second'");
        let printed = ty.to_string();
        assert!(
            !printed.contains("Never"),
            "expected non-Never type display for 'second' (= A.x), got: {printed}"
        );
        assert!(
            printed.contains("Int"),
            "expected Int in display for 'second' (= A.x), got: {printed}"
        );
    }

    #[tokio::test]
    async fn cursor_info_resolves_scle_import_from_scl() {
        // Foo.scle imports a value from Bar.scl; hover on the imported
        // reference inside the SCLE body must resolve to its type.
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Bar.scl"), b"export let answer = 42".to_vec());
        files.insert(
            PathBuf::from("Foo.scle"),
            b"import Self/Bar\n\nBar.answer\n".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        // Drive the loader from Foo.scle so the workspace ASG covers both.
        let mut loader = crate::Loader::new(finder);
        loader.resolve(&["Test", "Foo"]).await.unwrap();
        let asg = loader.finish().into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Foo".to_string()]);
        let src = "import Self/Bar\n\nBar.answer\n";
        // Cursor on `answer` in `Bar.answer` (line 3, col 5)
        let info = super::cursor_info(&asg, &module_id, src, Position::new(3, 5));
        let locked = info.lock().unwrap();
        let ty = locked
            .ty
            .clone()
            .expect("expected a type for the imported value at cursor");
        let printed = ty.to_string();
        assert!(
            printed.contains("Int"),
            "expected Int in display for Bar.answer, got: {printed}"
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

    #[tokio::test]
    async fn goto_definition_cross_module_property() {
        // Cursor on `answer` in `Lib.answer` should declare into Lib.scl.
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Lib.scl"), b"export let answer = 42".to_vec());
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nlet x = Lib.answer".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);
        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        // Cursor on `answer` in `Lib.answer` (line 2, col 14)
        let info = super::cursor_info(
            &asg,
            &module_id,
            "import Test/Lib\nlet x = Lib.answer",
            Position::new(2, 14),
        );
        let locked = info.lock().unwrap();
        let (decl_module, decl_span) = locked
            .declaration
            .as_ref()
            .expect("expected a declaration for the imported property");
        // Declaration should be in the Lib module.
        assert_eq!(
            decl_module,
            &vec!["Test".to_string(), "Lib".to_string()],
            "expected declaration in Test/Lib"
        );
        // Declaration span should be the `answer` identifier in `export let answer = 42`
        // which is at line 1, col 12–17.
        assert_eq!(decl_span.start().line(), 1);
        assert_eq!(decl_span.start().character(), 12);
    }

    #[tokio::test]
    async fn goto_definition_import_alias() {
        // Cursor on `Lib` in `import Test/Lib` should point to top of Lib.scl.
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Lib.scl"), b"export let answer = 42".to_vec());
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nlet x = Lib.answer".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);
        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        // Cursor on `Lib` in `import Test/Lib` (line 1, col 13)
        let info = super::cursor_info(
            &asg,
            &module_id,
            "import Test/Lib\nlet x = Lib.answer",
            Position::new(1, 13),
        );
        let locked = info.lock().unwrap();
        let (decl_module, decl_span) = locked
            .declaration
            .as_ref()
            .expect("expected a declaration for the import alias");
        assert_eq!(
            decl_module,
            &vec!["Test".to_string(), "Lib".to_string()],
            "expected declaration in Test/Lib"
        );
        // Import alias should point to 1:1–1:1 (top of file).
        assert_eq!(decl_span.start().line(), 1);
        assert_eq!(decl_span.start().character(), 1);
        assert_eq!(decl_span.end().line(), 1);
        assert_eq!(decl_span.end().character(), 1);
    }

    #[tokio::test]
    async fn goto_definition_import_alias_in_expr() {
        // Cursor on `Lib` in `Lib.answer` (the expression, not the import statement).
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Lib.scl"), b"export let answer = 42".to_vec());
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nlet x = Lib.answer".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);
        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        // Cursor on `Lib` in `Lib.answer` (line 2, col 9)
        let info = super::cursor_info(
            &asg,
            &module_id,
            "import Test/Lib\nlet x = Lib.answer",
            Position::new(2, 9),
        );
        let locked = info.lock().unwrap();
        let (decl_module, _decl_span) = locked
            .declaration
            .as_ref()
            .expect("expected a declaration for the import alias in expression");
        assert_eq!(
            decl_module,
            &vec!["Test".to_string(), "Lib".to_string()],
            "expected declaration in Test/Lib"
        );
    }

    #[tokio::test]
    async fn find_references_cross_module() {
        // Cursor on `answer` declaration in Lib.scl should find reference from Main.scl.
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Lib.scl"), b"export let answer = 42".to_vec());
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nlet x = Lib.answer".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);
        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Lib".to_string()]);
        // Cursor on `answer` in `export let answer = 42` (line 1, col 12)
        let info = super::cursor_info(
            &asg,
            &module_id,
            "export let answer = 42",
            Position::new(1, 12),
        );
        let locked = info.lock().unwrap();
        assert!(
            !locked.references.is_empty(),
            "expected at least one reference to 'answer'"
        );
        // Should have a reference from Main.scl.
        let has_cross_module_ref = locked
            .references
            .iter()
            .any(|(module, _)| module == &vec!["Test".to_string(), "Main".to_string()]);
        assert!(
            has_cross_module_ref,
            "expected a cross-module reference from Test/Main, got: {:?}",
            locked.references
        );
    }

    #[tokio::test]
    async fn same_module_global_declaration() {
        // Cursor on a reference to a same-module global should point to its declaration.
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"let foo = 42\nlet bar = foo".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);
        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        // Cursor on `foo` in `let bar = foo` (line 2, col 11)
        let info = super::cursor_info(
            &asg,
            &module_id,
            "let foo = 42\nlet bar = foo",
            Position::new(2, 11),
        );
        let locked = info.lock().unwrap();
        let (decl_module, decl_span) = locked
            .declaration
            .as_ref()
            .expect("expected a declaration for same-module global reference");
        assert_eq!(
            decl_module,
            &vec!["Test".to_string(), "Main".to_string()],
            "expected declaration in same module"
        );
        // `foo` declaration is at line 1, col 5.
        assert_eq!(decl_span.start().line(), 1);
        assert_eq!(decl_span.start().character(), 5);
    }

    #[tokio::test]
    async fn local_shadowing_prevents_cross_module_ref() {
        // A local `let Other = ...` should shadow the import.
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Other.scl"), b"export let abc = 123".to_vec());
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Other\nlet f = fn()\n  let Other = {abc: 123}\n  Other.abc".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);
        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let asg = result.into_inner();

        let module_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        // Cursor on `Other` in `Other.abc` (line 4, col 3) — should be the local, not the import.
        let info = super::cursor_info(
            &asg,
            &module_id,
            "import Test/Other\nlet f = fn()\n  let Other = {abc: 123}\n  Other.abc",
            Position::new(4, 3),
        );
        let locked = info.lock().unwrap();
        if let Some((decl_module, decl_span)) = &locked.declaration {
            // If there's a declaration, it should be in Main (the local let), not Other.
            assert_eq!(
                decl_module,
                &vec!["Test".to_string(), "Main".to_string()],
                "local shadow should resolve to same module, not import"
            );
            // The local `let Other` is on line 3, col 7.
            assert_eq!(decl_span.start().line(), 3);
            assert_eq!(decl_span.start().character(), 7);
        }
    }
}
