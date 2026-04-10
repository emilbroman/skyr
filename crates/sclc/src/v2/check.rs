use std::collections::HashMap;

use crate::{
    DiagList, Diagnosed, GlobalTypeEnv, RecordType, Type, TypeCheckError, TypeChecker, TypeEnv,
    ast, checker::CyclicDependency, ty::TypeKind, v2::GlobalKey,
};

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
/// Processes individual TypeDecl and Global nodes in topological SCC order
/// rather than delegating entire modules to `TypeChecker::check_file_mod`.
/// Expression-level checking (`check_expr`, `resolve_type_expr`, etc.) is
/// delegated to the existing `TypeChecker`.
pub struct AsgChecker<'a> {
    asg: &'a Asg,
    /// Accumulated global type environment built up across SCC iterations.
    global_type_env: GlobalTypeEnv,
}

impl<'a> AsgChecker<'a> {
    pub fn new(asg: &'a Asg) -> Self {
        Self {
            asg,
            global_type_env: GlobalTypeEnv::new(build_import_maps(asg)),
        }
    }

    /// Type-check the entire program by walking the ASG's SCC ordering.
    pub fn check(&mut self) -> Result<Diagnosed<CheckResults>, TypeCheckError> {
        let mut diags = DiagList::new();

        let modules: HashMap<crate::ModuleId, ast::FileMod> = self
            .asg
            .modules()
            .map(|mn| (mn.module_id.clone(), mn.file_mod.clone()))
            .collect();
        let package_names: Vec<crate::PackageId> = self.asg.packages().keys().cloned().collect();
        let checker = TypeChecker::from_modules(&modules, package_names);

        let sccs = self.asg.compute_sccs();
        for scc in &sccs {
            self.process_scc(&checker, scc, &mut diags)?;
        }

        // Check bare expression statements.
        for (raw_id, stmt) in self.asg.global_exprs() {
            if let ast::ModStmt::Expr(expr) = stmt
                && let Some(mn) = self.asg.module(raw_id)
            {
                let env = TypeEnv::new(&self.global_type_env)
                    .with_module_id(&mn.module_id)
                    .with_raw_module_id(&mn.raw_id);
                checker.check_expr(&env, expr, None)?.unpack(&mut diags);
            }
        }

        // Check import statements for cursor info.
        for mn in self.asg.modules() {
            let env = TypeEnv::new(&self.global_type_env)
                .with_module_id(&mn.module_id)
                .with_raw_module_id(&mn.raw_id);
            for stmt in &mn.file_mod.statements {
                if matches!(stmt, ast::ModStmt::Import(_)) {
                    checker.check_stmt(&env, stmt)?.unpack(&mut diags);
                }
            }
        }

        // Build CheckResults from global_type_env.
        let mut global_types = HashMap::new();
        let mut type_decl_types = HashMap::new();
        let mut module_types = HashMap::new();
        for (key, ty) in self.global_type_env.iter() {
            match key {
                GlobalKey::Global(raw_id, name) => {
                    global_types.insert((raw_id.clone(), name.clone()), ty.clone());
                }
                GlobalKey::TypeDecl(raw_id, name) => {
                    type_decl_types.insert((raw_id.clone(), name.clone()), ty.clone());
                }
                GlobalKey::ModuleValue(raw_id) => {
                    module_types.insert(raw_id.clone(), ty.clone());
                }
                GlobalKey::ModuleTypeLevel(_) => {}
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

    fn process_scc(
        &mut self,
        checker: &TypeChecker<'_>,
        scc: &[NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        let type_decl_nodes: Vec<&NodeId> = scc
            .iter()
            .filter(|n| matches!(n, NodeId::TypeDecl(..)))
            .collect();
        let global_nodes: Vec<&NodeId> = scc
            .iter()
            .filter(|n| matches!(n, NodeId::Global(..)))
            .collect();
        let module_nodes: Vec<&NodeId> = scc
            .iter()
            .filter(|n| matches!(n, NodeId::Module(..)))
            .collect();

        if !type_decl_nodes.is_empty() {
            self.process_type_decls(checker, &type_decl_nodes, diags)?;
        }

        if !global_nodes.is_empty() {
            self.process_globals(checker, &global_nodes, diags)?;
        }

        for node in &module_nodes {
            let NodeId::Module(raw_id) = node else {
                continue;
            };
            self.assemble_module(checker, raw_id);
        }

        Ok(())
    }

    // ─── TypeDecl processing ────────────────────────────────────────────────────

    fn process_type_decls(
        &mut self,
        checker: &TypeChecker<'_>,
        type_decl_nodes: &[&NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        if type_decl_nodes.len() == 1 && !self.asg.has_self_edge(type_decl_nodes[0]) {
            let NodeId::TypeDecl(raw_id, name) = type_decl_nodes[0] else {
                return Ok(());
            };
            let td = self.asg.type_decl(raw_id, name).unwrap();
            let mn = self.asg.module(raw_id).unwrap();

            let env = TypeEnv::new(&self.global_type_env)
                .with_module_id(&mn.module_id)
                .with_raw_module_id(&mn.raw_id);

            let ty = checker.resolve_type_def(&env, &td.type_def).unpack(diags);
            self.global_type_env
                .insert(GlobalKey::TypeDecl(raw_id.clone(), name.clone()), ty);
        } else {
            // Recursive group: register all with Never, iterate until stable.
            for node in type_decl_nodes {
                let NodeId::TypeDecl(raw_id, name) = node else {
                    continue;
                };
                self.global_type_env.insert(
                    GlobalKey::TypeDecl(raw_id.clone(), name.clone()),
                    Type::Never,
                );
            }
            for _ in 0..3 {
                for node in type_decl_nodes {
                    let NodeId::TypeDecl(raw_id, name) = node else {
                        continue;
                    };
                    let td = self.asg.type_decl(raw_id, name).unwrap();
                    let mn = self.asg.module(raw_id).unwrap();

                    let env = TypeEnv::new(&self.global_type_env)
                        .with_module_id(&mn.module_id)
                        .with_raw_module_id(&mn.raw_id);

                    let ty = checker.resolve_type_def(&env, &td.type_def).unpack(diags);
                    self.global_type_env
                        .insert(GlobalKey::TypeDecl(raw_id.clone(), name.clone()), ty);
                }
            }
        }
        Ok(())
    }

    // ─── Global processing ──────────────────────────────────────────────────────

    fn process_globals(
        &mut self,
        checker: &TypeChecker<'_>,
        global_nodes: &[&NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        if global_nodes.len() == 1 {
            self.check_singleton_global(checker, global_nodes[0], diags)?;
        } else {
            self.check_multi_global_scc(checker, global_nodes, diags)?;
        }
        Ok(())
    }

    fn check_singleton_global(
        &mut self,
        checker: &TypeChecker<'_>,
        node: &NodeId,
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        let NodeId::Global(raw_id, name) = node else {
            return Ok(());
        };
        let has_self_edge = self.asg.has_self_edge(node);
        let mn = self.asg.module(raw_id).unwrap();
        let lb = find_let_bind(&mn.file_mod, name).unwrap();

        if has_self_edge && !matches!(lb.expr.as_ref().as_ref(), ast::Expr::Fn(_)) {
            diags.push(CyclicDependency {
                module_id: mn.module_id.clone(),
                names: format!("`{name}`"),
                span: lb.var.span(),
            });
            let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
            checker
                .global_cache
                .borrow_mut()
                .insert(cache_key, Type::Never);
            self.global_type_env.insert(
                GlobalKey::Global(raw_id.clone(), name.to_string()),
                Type::Never,
            );
            return Ok(());
        }

        let env = TypeEnv::new(&self.global_type_env)
            .with_module_id(&mn.module_id)
            .with_raw_module_id(&mn.raw_id);

        let ty = checker.check_global_let_bind(&env, lb)?.unpack(diags);
        let unwrapped = match &ty.kind {
            TypeKind::IsoRec(_, inner) => inner.as_ref().clone(),
            _ => ty,
        };
        self.global_type_env.insert(
            GlobalKey::Global(raw_id.clone(), name.to_string()),
            unwrapped,
        );
        Ok(())
    }

    fn check_multi_global_scc(
        &mut self,
        checker: &TypeChecker<'_>,
        global_nodes: &[&NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        let all_fns = global_nodes.iter().all(|n| {
            if let NodeId::Global(raw_id, name) = n {
                self.asg
                    .module(raw_id)
                    .and_then(|mn| find_let_bind(&mn.file_mod, name))
                    .map(|lb| matches!(lb.expr.as_ref().as_ref(), ast::Expr::Fn(_)))
                    .unwrap_or(false)
            } else {
                true
            }
        });

        if !all_fns {
            return self.diagnose_cyclic_non_fns(checker, global_nodes, diags);
        }

        let all_same_module = {
            let first = global_nodes.iter().find_map(|n| {
                if let NodeId::Global(raw_id, _) = n {
                    Some(raw_id)
                } else {
                    None
                }
            });
            first.is_some_and(|first_id| {
                global_nodes.iter().all(|n| {
                    if let NodeId::Global(raw_id, _) = n {
                        raw_id == first_id
                    } else {
                        true
                    }
                })
            })
        };

        if all_same_module {
            self.check_same_module_recursive_group(checker, global_nodes, diags)
        } else {
            self.check_cross_module_recursive_group(checker, global_nodes, diags)
        }
    }

    fn diagnose_cyclic_non_fns(
        &mut self,
        checker: &TypeChecker<'_>,
        global_nodes: &[&NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        let mut sorted_names: Vec<&str> = global_nodes
            .iter()
            .filter_map(|n| {
                if let NodeId::Global(_, name) = n {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect();
        sorted_names.sort();
        let names_str = sorted_names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ");

        for node in global_nodes {
            let NodeId::Global(raw_id, name) = node else {
                continue;
            };
            let mn = self.asg.module(raw_id).unwrap();
            let lb = find_let_bind(&mn.file_mod, name).unwrap();
            diags.push(CyclicDependency {
                module_id: mn.module_id.clone(),
                names: names_str.clone(),
                span: lb.var.span(),
            });
            let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
            checker
                .global_cache
                .borrow_mut()
                .insert(cache_key, Type::Never);
            self.global_type_env
                .insert(GlobalKey::Global(raw_id.clone(), name.clone()), Type::Never);
        }
        Ok(())
    }

    fn check_same_module_recursive_group(
        &mut self,
        checker: &TypeChecker<'_>,
        global_nodes: &[&NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        let first_raw_id = global_nodes.iter().find_map(|n| {
            if let NodeId::Global(raw_id, _) = n {
                Some(raw_id)
            } else {
                None
            }
        });
        let Some(first_raw_id) = first_raw_id else {
            return Ok(());
        };
        let mn = self.asg.module(first_raw_id).unwrap();

        let env = TypeEnv::new(&self.global_type_env)
            .with_module_id(&mn.module_id)
            .with_raw_module_id(&mn.raw_id);

        let scc_binding_ids: Vec<crate::dep_graph::BindingId> = global_nodes
            .iter()
            .filter_map(|n| {
                if let NodeId::Global(raw_id, name) = n {
                    let mn = self.asg.module(raw_id)?;
                    Some(crate::dep_graph::BindingId {
                        module_id: mn.module_id.clone(),
                        name: name.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();

        let binding_by_name: HashMap<&str, &ast::LetBind> = global_nodes
            .iter()
            .filter_map(|n| {
                if let NodeId::Global(raw_id, name) = n {
                    let mn = self.asg.module(raw_id)?;
                    let lb = find_let_bind(&mn.file_mod, name)?;
                    Some((name.as_str(), lb))
                } else {
                    None
                }
            })
            .collect();

        checker
            .check_recursive_scc_group(&env, &scc_binding_ids, &binding_by_name)?
            .unpack(diags);

        for node in global_nodes {
            let NodeId::Global(raw_id, name) = node else {
                continue;
            };
            if let Some(mn) = self.asg.module(raw_id)
                && let Some(lb) = find_let_bind(&mn.file_mod, name)
            {
                let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
                if let Some(ty) = checker.global_cache.borrow().get(&cache_key) {
                    self.global_type_env
                        .insert(GlobalKey::Global(raw_id.clone(), name.clone()), ty.clone());
                }
            }
        }
        Ok(())
    }

    fn check_cross_module_recursive_group(
        &mut self,
        checker: &TypeChecker<'_>,
        global_nodes: &[&NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        for node in global_nodes {
            let NodeId::Global(raw_id, name) = node else {
                continue;
            };
            let has_self_edge = self.asg.has_self_edge(node);
            let mn = self.asg.module(raw_id).unwrap();
            let lb = find_let_bind(&mn.file_mod, name).unwrap();

            if has_self_edge && !matches!(lb.expr.as_ref().as_ref(), ast::Expr::Fn(_)) {
                diags.push(CyclicDependency {
                    module_id: mn.module_id.clone(),
                    names: format!("`{name}`"),
                    span: lb.var.span(),
                });
                let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
                checker
                    .global_cache
                    .borrow_mut()
                    .insert(cache_key, Type::Never);
                self.global_type_env.insert(
                    GlobalKey::Global(raw_id.clone(), name.to_string()),
                    Type::Never,
                );
                continue;
            }

            let env = TypeEnv::new(&self.global_type_env)
                .with_module_id(&mn.module_id)
                .with_raw_module_id(&mn.raw_id);

            let ty = checker.check_global_let_bind(&env, lb)?.unpack(diags);
            let unwrapped = match &ty.kind {
                TypeKind::IsoRec(_, inner) => inner.as_ref().clone(),
                _ => ty,
            };
            self.global_type_env.insert(
                GlobalKey::Global(raw_id.clone(), name.to_string()),
                unwrapped,
            );
        }
        Ok(())
    }

    // ─── Module assembly ────────────────────────────────────────────────────────

    fn assemble_module(&mut self, checker: &TypeChecker<'_>, raw_id: &RawModuleId) {
        let Some(mn) = self.asg.module(raw_id) else {
            return;
        };

        // Value-level export record.
        let mut exports = RecordType::default();
        for stmt in &mn.file_mod.statements {
            if let ast::ModStmt::Export(lb) = stmt {
                let key = GlobalKey::Global(raw_id.clone(), lb.var.name.clone());
                if let Some(ty) = self.global_type_env.get(&key) {
                    let ty = Type::IsoRec(crate::checker::next_type_id(), Box::new(ty.clone()));
                    exports.insert_with_doc(lb.var.name.clone(), ty, lb.doc_comment.clone());
                }
            }
        }
        let module_ty = Type::Record(exports);
        self.global_type_env
            .insert(GlobalKey::ModuleValue(raw_id.clone()), module_ty.clone());

        // Seed import_cache for expression-level resolution.
        if let Some(unit_fm) = checker.modules.get(&mn.module_id) {
            let key = unit_fm as *const ast::FileMod;
            checker
                .import_cache
                .borrow_mut()
                .insert(key, module_ty.clone());
        }
        let asg_key = &mn.file_mod as *const ast::FileMod;
        checker
            .import_cache
            .borrow_mut()
            .entry(asg_key)
            .or_insert_with(|| module_ty);

        // Type-level export record.
        let mut type_exports = RecordType::default();
        for stmt in &mn.file_mod.statements {
            if let ast::ModStmt::ExportTypeDef(td) = stmt {
                let key = GlobalKey::TypeDecl(raw_id.clone(), td.var.name.clone());
                if let Some(ty) = self.global_type_env.get(&key) {
                    type_exports.insert_with_doc(
                        td.var.name.clone(),
                        ty.clone(),
                        td.doc_comment.clone(),
                    );
                }
            }
        }
        self.global_type_env.insert(
            GlobalKey::ModuleTypeLevel(raw_id.clone()),
            Type::Record(type_exports.clone()),
        );

        if let Some(unit_fm) = checker.modules.get(&mn.module_id) {
            let key = unit_fm as *const ast::FileMod;
            checker
                .type_level_cache
                .borrow_mut()
                .insert(key, type_exports);
        }
    }
}

/// Build per-module import alias → RawModuleId maps from the ASG.
fn build_import_maps(asg: &Asg) -> HashMap<RawModuleId, HashMap<String, RawModuleId>> {
    let mut maps = HashMap::new();
    for module_node in asg.modules() {
        let mut aliases = HashMap::new();
        for stmt in &module_node.file_mod.statements {
            if let ast::ModStmt::Import(import) = stmt {
                let vars = &import.as_ref().vars;
                if !vars.is_empty() {
                    let alias = vars.last().unwrap().name.clone();
                    let import_raw_id = resolve_import_path(vars, &module_node.package_id);
                    aliases.insert(alias, import_raw_id);
                }
            }
        }
        maps.insert(module_node.raw_id.clone(), aliases);
    }
    maps
}

fn resolve_import_path(vars: &[crate::Loc<ast::Var>], pkg_id: &crate::PackageId) -> RawModuleId {
    let segments: Vec<String> = vars.iter().map(|v| v.name.clone()).collect();
    if segments.first().is_some_and(|s| s == "Self") {
        let mut resolved: Vec<String> = pkg_id.as_slice().to_vec();
        resolved.extend(segments[1..].iter().cloned());
        resolved
    } else {
        segments
    }
}

fn find_let_bind<'m>(file_mod: &'m ast::FileMod, name: &str) -> Option<&'m ast::LetBind> {
    file_mod.statements.iter().find_map(|stmt| match stmt {
        ast::ModStmt::Let(lb) | ast::ModStmt::Export(lb) if lb.var.name == name => Some(lb),
        _ => None,
    })
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
