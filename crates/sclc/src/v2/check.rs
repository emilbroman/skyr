use std::collections::HashMap;

use crate::{DiagList, Diagnosed, Type, TypeCheckError, TypeChecker, TypeEnv, ast};

use super::{Asg, NodeId, RawModuleId};

/// Results from the ASG-driven type checker.
pub struct CheckResults {
    /// Resolved types for globals (unwrapped, not IsoRec).
    pub global_types: HashMap<(RawModuleId, String), Type>,
    /// Resolved type declarations.
    pub type_decl_types: HashMap<(RawModuleId, String), Type>,
    /// Module export record types.
    pub module_types: HashMap<RawModuleId, Type>,
}

/// ASG-driven type checker that walks the ASG's global SCC ordering.
///
/// Replaces the per-module orchestration in `TypeChecker::check_program()` with
/// a global SCC walk driven by the ASG's dependency graph. Expression-level
/// checking (`check_expr`, `resolve_type_expr`, etc.) is delegated to the
/// existing `TypeChecker`.
pub struct AsgChecker<'a> {
    asg: &'a Asg,
}

impl<'a> AsgChecker<'a> {
    pub fn new(asg: &'a Asg) -> Self {
        Self { asg }
    }

    /// Type-check the entire program by walking the ASG's SCC ordering.
    pub fn check(&self) -> Result<Diagnosed<CheckResults>, TypeCheckError> {
        let mut diags = DiagList::new();

        // Transitional: create CompilationUnit for expression-level checking.
        // The TypeChecker needs a CompilationUnit for import resolution and
        // path validation. This will be removed once the AsgChecker handles
        // all import resolution natively.
        let unit = super::compile::asg_to_compilation_unit(self.asg);
        let checker = TypeChecker::new(&unit);

        // Process modules in SCC topological order. The ASG's SCCs contain
        // Module, Global, and TypeDecl nodes. Module nodes depend on all their
        // contained globals and type declarations (via containment edges), so
        // by the time a Module node appears in the walk, all its contents have
        // already been visited. We process Module nodes via check_file_mod,
        // which handles the full per-module pipeline (type env, intra-module
        // SCCs, exports).
        let sccs = self.asg.compute_sccs();
        for scc in &sccs {
            for node in scc {
                if let NodeId::Module(raw_id) = node
                    && let Some(module_node) = self.asg.module(raw_id)
                {
                    let env = TypeEnv::new().with_module_id(&module_node.module_id);
                    checker
                        .check_file_mod(&env, &module_node.file_mod)?
                        .unpack(&mut diags);
                }
            }
        }

        // Extract results from the TypeChecker's pointer-based caches into
        // name-keyed maps. We iterate the file_mod statements (not ASG
        // GlobalNodes) because check_file_mod caches pointers from the
        // file_mod's AST, which differ from the cloned stmts in GlobalNode.
        let mut global_types = HashMap::new();
        let type_decl_types = HashMap::new();
        let mut module_types = HashMap::new();

        for module_node in self.asg.modules() {
            // Extract global types from this module's statements.
            for stmt in &module_node.file_mod.statements {
                if let ast::ModStmt::Let(lb) | ast::ModStmt::Export(lb) = stmt {
                    let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
                    if let Some(ty) = checker.global_cache.borrow().get(&cache_key) {
                        global_types.insert(
                            (module_node.raw_id.clone(), lb.var.name.clone()),
                            ty.clone(),
                        );
                    }
                }
            }

            // Extract the module's export record type.
            let cache_key = &module_node.file_mod as *const ast::FileMod;
            if let Some(ty) = checker.import_cache.borrow().get(&cache_key) {
                module_types.insert(module_node.raw_id.clone(), ty.clone());
            }
        }

        Ok(Diagnosed::new(
            CheckResults {
                global_types,
                type_decl_types,
                module_types,
            },
            diags,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::PackageId;
    use crate::v2::{InMemoryPackage, Loader, build_default_finder};

    #[tokio::test]
    async fn checker_instantiation() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"export let x = 1".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let asg = loader.finish().into_inner();

        let result = AsgChecker::new(&asg).check().unwrap();
        assert!(!result.diags().has_errors());
    }

    #[tokio::test]
    async fn checker_resolves_global_types() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"export let x = 42".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let asg = loader.finish().into_inner();

        let result = AsgChecker::new(&asg).check().unwrap();
        let check = result.into_inner();

        let key = (vec!["Test".into(), "Main".into()], "x".into());
        assert!(check.global_types.contains_key(&key));
    }

    #[tokio::test]
    async fn checker_with_import() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nexport let x = Lib.foo".to_vec(),
        );
        files.insert(PathBuf::from("Lib.scl"), b"export let foo = 42".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let asg = loader.finish().into_inner();

        let result = AsgChecker::new(&asg).check().unwrap();
        assert!(!result.diags().has_errors());
    }
}
