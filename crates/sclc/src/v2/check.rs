use std::collections::HashMap;

use crate::{
    DiagList, Diagnosed, GlobalTypeEnv, RecordType, Span, Type, TypeCheckError, TypeChecker,
    TypeEnv, ast, checker::CyclicDependency, ty::TypeKind,
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
    /// Accumulated global type environment for the v2 pipeline.
    global_type_env: GlobalTypeEnv,
    /// Resolved types for value-level globals.
    global_types: HashMap<(RawModuleId, String), Type>,
    /// Resolved type declarations.
    type_decl_types: HashMap<(RawModuleId, String), Type>,
    /// Module export record types (for import resolution).
    module_types: HashMap<RawModuleId, Type>,
    /// Module type-level export records (for `Import.TypeName` resolution).
    module_type_levels: HashMap<RawModuleId, RecordType>,
}

impl<'a> AsgChecker<'a> {
    pub fn new(asg: &'a Asg) -> Self {
        Self {
            asg,
            global_type_env: GlobalTypeEnv::default(),
            global_types: HashMap::new(),
            type_decl_types: HashMap::new(),
            module_types: HashMap::new(),
            module_type_levels: HashMap::new(),
        }
    }

    /// Type-check the entire program by walking the ASG's SCC ordering.
    pub fn check(&mut self) -> Result<Diagnosed<CheckResults>, TypeCheckError> {
        let mut diags = DiagList::new();

        // Build module map and package names directly from the Asg.
        let modules: HashMap<crate::ModuleId, ast::FileMod> = self
            .asg
            .modules()
            .map(|mn| (mn.module_id.clone(), mn.file_mod.clone()))
            .collect();
        let package_names: Vec<crate::PackageId> = self.asg.packages().keys().cloned().collect();
        let checker = TypeChecker::from_modules(&modules, package_names);

        let import_maps = build_import_maps(self.asg);

        let sccs = self.asg.compute_sccs();
        for scc in &sccs {
            self.process_scc(&checker, &import_maps, scc, &mut diags)?;
        }

        // Check bare expression statements (skip modules already checked via check_file_mod).
        for (raw_id, stmt) in self.asg.global_exprs() {
            if let ast::ModStmt::Expr(expr) = stmt
                && let Some(module_node) = self.asg.module(raw_id)
            {
                let globals = module_node.file_mod.find_globals();
                let imports =
                    checker.find_imports(&module_node.file_mod, &module_node.module_id.package);
                let mut env = TypeEnv::new(&self.global_type_env)
                    .with_module_id(&module_node.module_id)
                    .with_globals(&globals)
                    .with_imports(&imports);
                self.populate_type_env(&mut env, &import_maps, raw_id);
                self.add_checked_globals_as_locals(&mut env, raw_id);
                self.preseed_import_caches(&checker, &import_maps, raw_id);
                checker.check_expr(&env, expr, None)?.unpack(&mut diags);
            }
        }

        // Also check import statements for cursor info.
        for module_node in self.asg.modules() {
            let globals = module_node.file_mod.find_globals();
            let imports =
                checker.find_imports(&module_node.file_mod, &module_node.module_id.package);
            let env = TypeEnv::new(&self.global_type_env)
                .with_module_id(&module_node.module_id)
                .with_globals(&globals)
                .with_imports(&imports);
            for stmt in &module_node.file_mod.statements {
                if matches!(stmt, ast::ModStmt::Import(_)) {
                    checker.check_stmt(&env, stmt)?.unpack(&mut diags);
                }
            }
        }

        Ok(Diagnosed::new(
            CheckResults {
                global_types: self.global_types.clone(),
                type_decl_types: self.type_decl_types.clone(),
                module_types: self.module_types.clone(),
            },
            diags,
        ))
    }

    fn process_scc(
        &mut self,
        checker: &TypeChecker<'_>,
        import_maps: &HashMap<RawModuleId, HashMap<String, RawModuleId>>,
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

        // For multi-module SCCs (circular imports), pre-seed import/type-level
        // caches with empty records so that expression-level import resolution
        // doesn't recurse infinitely.
        if module_nodes.len() > 1 {
            for node in &module_nodes {
                let NodeId::Module(raw_id) = node else {
                    continue;
                };
                if let Some(mn) = self.asg.module(raw_id)
                    && let Some(module_fm) = checker.modules.get(&mn.module_id)
                {
                    let key = module_fm as *const ast::FileMod;
                    checker
                        .import_cache
                        .borrow_mut()
                        .insert(key, Type::Record(RecordType::default()));
                    checker
                        .type_level_cache
                        .borrow_mut()
                        .insert(key, RecordType::default());
                }
            }
        }

        // Process TypeDecl nodes.
        if !type_decl_nodes.is_empty() {
            self.process_type_decls(checker, import_maps, &type_decl_nodes, diags)?;
        }

        // Process Global nodes.
        if !global_nodes.is_empty() {
            self.process_globals(checker, import_maps, &global_nodes, diags)?;
        }

        // Assemble Module nodes.
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
        import_maps: &HashMap<RawModuleId, HashMap<String, RawModuleId>>,
        type_decl_nodes: &[&NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        if type_decl_nodes.len() == 1 && !self.asg.has_self_edge(type_decl_nodes[0]) {
            // Non-recursive singleton.
            let NodeId::TypeDecl(raw_id, name) = type_decl_nodes[0] else {
                return Ok(());
            };
            let td_node = self.asg.type_decl(raw_id, name).unwrap();
            let module_node = self.asg.module(raw_id).unwrap();

            let globals = module_node.file_mod.find_globals();
            let imports =
                checker.find_imports(&module_node.file_mod, &module_node.module_id.package);
            let mut env = TypeEnv::new(&self.global_type_env)
                .with_module_id(&module_node.module_id)
                .with_globals(&globals)
                .with_imports(&imports);
            self.populate_type_env(&mut env, import_maps, raw_id);
            self.preseed_import_caches(checker, import_maps, raw_id);

            let ty = checker
                .resolve_type_def(&env, &td_node.type_def)
                .unpack(diags);
            self.type_decl_types
                .insert((raw_id.clone(), name.clone()), ty);
        } else {
            // Recursive group: register all with Never, iterate until stable.
            for node in type_decl_nodes {
                let NodeId::TypeDecl(raw_id, name) = node else {
                    continue;
                };
                self.type_decl_types
                    .insert((raw_id.clone(), name.clone()), Type::Never);
            }
            for _ in 0..3 {
                for node in type_decl_nodes {
                    let NodeId::TypeDecl(raw_id, name) = node else {
                        continue;
                    };
                    let td_node = self.asg.type_decl(raw_id, name).unwrap();
                    let module_node = self.asg.module(raw_id).unwrap();

                    let globals = module_node.file_mod.find_globals();
                    let imports =
                        checker.find_imports(&module_node.file_mod, &module_node.module_id.package);
                    let mut env = TypeEnv::new(&self.global_type_env)
                        .with_module_id(&module_node.module_id)
                        .with_globals(&globals)
                        .with_imports(&imports);
                    self.populate_type_env(&mut env, import_maps, raw_id);
                    self.preseed_import_caches(checker, import_maps, raw_id);

                    let ty = checker
                        .resolve_type_def(&env, &td_node.type_def)
                        .unpack(diags);
                    self.type_decl_types
                        .insert((raw_id.clone(), name.clone()), ty);
                }
            }
        }

        Ok(())
    }

    // ─── Global processing ──────────────────────────────────────────────────────

    fn process_globals(
        &mut self,
        checker: &TypeChecker<'_>,
        import_maps: &HashMap<RawModuleId, HashMap<String, RawModuleId>>,
        global_nodes: &[&NodeId],
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        if global_nodes.len() == 1 {
            let NodeId::Global(raw_id, name) = global_nodes[0] else {
                return Ok(());
            };
            let has_self_edge = self.asg.has_self_edge(global_nodes[0]);
            let module_node = self.asg.module(raw_id).unwrap();
            let let_bind = find_let_bind(&module_node.file_mod, name).unwrap();

            if has_self_edge && !matches!(let_bind.expr.as_ref().as_ref(), ast::Expr::Fn(_)) {
                diags.push(CyclicDependency {
                    module_id: module_node.module_id.clone(),
                    names: format!("`{name}`"),
                    span: let_bind.var.span(),
                });
                let cache_key = let_bind.expr.as_ref() as *const crate::Loc<ast::Expr>;
                checker
                    .global_cache
                    .borrow_mut()
                    .insert(cache_key, Type::Never);
                self.global_types
                    .insert((raw_id.clone(), name.to_string()), Type::Never);
                return Ok(());
            }

            let globals = module_node.file_mod.find_globals();
            let imports =
                checker.find_imports(&module_node.file_mod, &module_node.module_id.package);
            let mut env = TypeEnv::new(&self.global_type_env)
                .with_module_id(&module_node.module_id)
                .with_globals(&globals)
                .with_imports(&imports);
            self.populate_type_env(&mut env, import_maps, raw_id);
            self.add_checked_globals_as_locals(&mut env, raw_id);
            self.preseed_import_caches(checker, import_maps, raw_id);

            let ty = checker.check_global_let_bind(&env, let_bind)?.unpack(diags);

            // Extract unwrapped type (strip IsoRec).
            let unwrapped = match &ty.kind {
                TypeKind::IsoRec(_, inner) => inner.as_ref().clone(),
                _ => ty,
            };
            self.global_types
                .insert((raw_id.clone(), name.to_string()), unwrapped);
        } else {
            // Multi-node SCC: validate all are functions.
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
                    let module_node = self.asg.module(raw_id).unwrap();
                    let let_bind = find_let_bind(&module_node.file_mod, name).unwrap();
                    diags.push(CyclicDependency {
                        module_id: module_node.module_id.clone(),
                        names: names_str.clone(),
                        span: let_bind.var.span(),
                    });
                    let cache_key = let_bind.expr.as_ref() as *const crate::Loc<ast::Expr>;
                    checker
                        .global_cache
                        .borrow_mut()
                        .insert(cache_key, Type::Never);
                    self.global_types
                        .insert((raw_id.clone(), name.clone()), Type::Never);
                }
                return Ok(());
            }

            // All are functions: check as recursive group.
            // Check if all globals are from the same module.
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
                // Intra-module recursive group: use check_recursive_scc_group.
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
                let first_module = self.asg.module(first_raw_id).unwrap();

                let globals = first_module.file_mod.find_globals();
                let imports =
                    checker.find_imports(&first_module.file_mod, &first_module.module_id.package);
                let mut env = TypeEnv::new(&self.global_type_env)
                    .with_module_id(&first_module.module_id)
                    .with_globals(&globals)
                    .with_imports(&imports);
                self.populate_type_env(&mut env, import_maps, first_raw_id);
                self.add_checked_globals_as_locals(&mut env, first_raw_id);
                self.preseed_import_caches(checker, import_maps, first_raw_id);

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

                // Extract results from global_cache.
                for node in global_nodes {
                    let NodeId::Global(raw_id, name) = node else {
                        continue;
                    };
                    if let Some(mn) = self.asg.module(raw_id)
                        && let Some(lb) = find_let_bind(&mn.file_mod, name)
                    {
                        let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
                        if let Some(ty) = checker.global_cache.borrow().get(&cache_key) {
                            self.global_types
                                .insert((raw_id.clone(), name.clone()), ty.clone());
                        }
                    }
                }
            } else {
                // Cross-module recursive group: check each global individually.
                // Cross-module references go through function bodies (lazy edges),
                // so each global is checked with its own module's env. Import
                // references resolve to pre-seeded empty records for not-yet-assembled
                // modules.
                for node in global_nodes {
                    let NodeId::Global(raw_id, name) = node else {
                        continue;
                    };
                    let has_self_edge = self.asg.has_self_edge(node);
                    let module_node = self.asg.module(raw_id).unwrap();
                    let let_bind = find_let_bind(&module_node.file_mod, name).unwrap();

                    if has_self_edge && !matches!(let_bind.expr.as_ref().as_ref(), ast::Expr::Fn(_))
                    {
                        diags.push(CyclicDependency {
                            module_id: module_node.module_id.clone(),
                            names: format!("`{name}`"),
                            span: let_bind.var.span(),
                        });
                        let cache_key = let_bind.expr.as_ref() as *const crate::Loc<ast::Expr>;
                        checker
                            .global_cache
                            .borrow_mut()
                            .insert(cache_key, Type::Never);
                        self.global_types
                            .insert((raw_id.clone(), name.to_string()), Type::Never);
                        continue;
                    }

                    let globals = module_node.file_mod.find_globals();
                    let imports =
                        checker.find_imports(&module_node.file_mod, &module_node.module_id.package);
                    let mut env = TypeEnv::new(&self.global_type_env)
                        .with_module_id(&module_node.module_id)
                        .with_globals(&globals)
                        .with_imports(&imports);
                    self.populate_type_env(&mut env, import_maps, raw_id);
                    self.add_checked_globals_as_locals(&mut env, raw_id);
                    self.preseed_import_caches(checker, import_maps, raw_id);

                    let ty = checker.check_global_let_bind(&env, let_bind)?.unpack(diags);
                    let unwrapped = match &ty.kind {
                        TypeKind::IsoRec(_, inner) => inner.as_ref().clone(),
                        _ => ty,
                    };
                    self.global_types
                        .insert((raw_id.clone(), name.to_string()), unwrapped);
                }
            }
        }

        Ok(())
    }

    // ─── Module assembly ────────────────────────────────────────────────────────

    fn assemble_module(&mut self, checker: &TypeChecker<'_>, raw_id: &RawModuleId) {
        let Some(module_node) = self.asg.module(raw_id) else {
            return;
        };

        // Assemble value-level export record.
        let mut exports = RecordType::default();
        for stmt in &module_node.file_mod.statements {
            if let ast::ModStmt::Export(let_bind) = stmt {
                let key = (raw_id.clone(), let_bind.var.name.clone());
                if let Some(ty) = self.global_types.get(&key) {
                    let ty = Type::IsoRec(crate::checker::next_type_id(), Box::new(ty.clone()));
                    exports.insert_with_doc(
                        let_bind.var.name.clone(),
                        ty,
                        let_bind.doc_comment.clone(),
                    );
                }
            }
        }
        let module_ty = Type::Record(exports);
        self.module_types.insert(raw_id.clone(), module_ty.clone());

        // Seed import_cache so expression-level import resolution returns immediately.
        if let Some(unit_fm) = checker.modules.get(&module_node.module_id) {
            let key = unit_fm as *const ast::FileMod;
            checker
                .import_cache
                .borrow_mut()
                .insert(key, module_ty.clone());
        }
        // Also cache using ASG's FileMod pointer.
        let asg_key = &module_node.file_mod as *const ast::FileMod;
        checker
            .import_cache
            .borrow_mut()
            .entry(asg_key)
            .or_insert_with(|| module_ty);

        // Assemble type-level export record.
        let mut type_exports = RecordType::default();
        for stmt in &module_node.file_mod.statements {
            if let ast::ModStmt::ExportTypeDef(type_def) = stmt {
                let key = (raw_id.clone(), type_def.var.name.clone());
                if let Some(ty) = self.type_decl_types.get(&key) {
                    type_exports.insert_with_doc(
                        type_def.var.name.clone(),
                        ty.clone(),
                        type_def.doc_comment.clone(),
                    );
                }
            }
        }
        self.module_type_levels
            .insert(raw_id.clone(), type_exports.clone());

        // Seed type_level_cache.
        if let Some(unit_fm) = checker.modules.get(&module_node.module_id) {
            let key = unit_fm as *const ast::FileMod;
            checker
                .type_level_cache
                .borrow_mut()
                .insert(key, type_exports);
        }
    }

    // ─── Helpers ────────────────────────────────────────────────────────────────

    /// Add type-level bindings and already-resolved type decls/imports to `env`.
    fn populate_type_env(
        &self,
        env: &mut TypeEnv<'_>,
        import_maps: &HashMap<RawModuleId, HashMap<String, RawModuleId>>,
        raw_id: &RawModuleId,
    ) {
        // Same-module type decls.
        for ((rid, name), ty) in &self.type_decl_types {
            if rid == raw_id {
                *env = env.with_type_level(name.clone(), ty.clone(), None);
            }
        }
        // Import type-level exports.
        if let Some(imports) = import_maps.get(raw_id) {
            for (alias, import_raw_id) in imports {
                if let Some(rt) = self.module_type_levels.get(import_raw_id)
                    && rt.iter().next().is_some()
                {
                    *env = env.with_type_level(alias.clone(), Type::Record(rt.clone()), None);
                }
            }
        }
    }

    /// Add already-checked same-module globals as locals in `env`.
    fn add_checked_globals_as_locals<'e>(&'e self, env: &mut TypeEnv<'e>, raw_id: &RawModuleId) {
        for ((rid, name), ty) in &self.global_types {
            if rid == raw_id {
                *env = env.with_local(name.as_str(), Span::default(), ty.clone());
            }
        }
    }

    /// Pre-seed the TypeChecker's import/type-level caches with already-resolved
    /// module types so expression-level import resolution returns immediately.
    /// For modules not yet assembled (circular imports), seeds with empty records
    /// to prevent infinite recursion in check_file_mod.
    fn preseed_import_caches(
        &self,
        checker: &TypeChecker<'_>,
        import_maps: &HashMap<RawModuleId, HashMap<String, RawModuleId>>,
        raw_id: &RawModuleId,
    ) {
        if let Some(imports) = import_maps.get(raw_id) {
            for import_raw_id in imports.values() {
                if let Some(import_mn) = self.asg.module(import_raw_id)
                    && let Some(unit_fm) = checker.modules.get(&import_mn.module_id)
                {
                    let key = unit_fm as *const ast::FileMod;
                    // Use already-resolved type, or empty record for not-yet-assembled modules.
                    let default_ty = Type::Record(RecordType::default());
                    checker
                        .import_cache
                        .borrow_mut()
                        .entry(key)
                        .or_insert_with(|| {
                            self.module_types
                                .get(import_raw_id)
                                .cloned()
                                .unwrap_or_else(|| default_ty.clone())
                        });
                    checker
                        .type_level_cache
                        .borrow_mut()
                        .entry(key)
                        .or_insert_with(|| {
                            self.module_type_levels
                                .get(import_raw_id)
                                .cloned()
                                .unwrap_or_default()
                        });
                }
            }
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

/// Resolve an import path to a raw module ID.
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

/// Find a LetBind by name in a module's statements.
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
