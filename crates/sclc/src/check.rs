use std::collections::HashMap;

use crate::{
    DiagList, Diagnosed, GlobalKey, GlobalTypeEnv, RecordType, Type, TypeCheckError, TypeChecker,
    TypeEnv, ast,
    checker::{CyclicDependency, FreeVarConstraints, next_type_id},
    ty::TypeKind,
};

use crate::{Asg, NodeId, RawModuleId};

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
        let mut global_type_env = GlobalTypeEnv::new(build_import_maps(asg));
        for mn in asg.modules() {
            if mn.body.is_scle() {
                global_type_env.mark_scle_module(mn.raw_id.clone());
            }
        }
        Self {
            asg,
            global_type_env,
        }
    }

    /// Return the accumulated `GlobalTypeEnv` after checking.
    pub fn into_global_type_env(self) -> GlobalTypeEnv {
        self.global_type_env
    }

    /// Type-check the entire program by walking the ASG's SCC ordering.
    pub fn check(&mut self) -> Result<Diagnosed<CheckResults>, TypeCheckError> {
        let mut diags = DiagList::new();

        let modules: HashMap<crate::ModuleId, ast::FileMod> = self
            .asg
            .modules()
            .filter_map(|mn| {
                mn.body
                    .as_file_mod()
                    .map(|fm| (mn.module_id.clone(), fm.clone()))
            })
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
            for stmt in mn.body.statements() {
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
            self.assemble_module(checker, raw_id, diags)?;
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
            // Recursive group: allocate a type variable for each member,
            // bootstrap with Var(type_id), resolve once, then wrap with
            // IsoRec where the variable actually appears in the body.
            let scc_vars: Vec<(&NodeId, usize)> = type_decl_nodes
                .iter()
                .map(|node| (*node, next_type_id()))
                .collect();

            // Bootstrap: register each type name as its type variable
            for (node, type_id) in &scc_vars {
                let NodeId::TypeDecl(raw_id, name) = node else {
                    continue;
                };
                self.global_type_env.insert(
                    GlobalKey::TypeDecl(raw_id.clone(), name.clone()),
                    Type::Var(*type_id),
                );
            }

            // Resolve each type body once (references to SCC members
            // will appear as Var(type_id) in the resolved type).
            let mut resolved: Vec<(&NodeId, usize, Type)> = Vec::with_capacity(scc_vars.len());
            for (node, type_id) in &scc_vars {
                let NodeId::TypeDecl(raw_id, name) = node else {
                    continue;
                };
                let td = self.asg.type_decl(raw_id, name).unwrap();
                let mn = self.asg.module(raw_id).unwrap();

                let env = TypeEnv::new(&self.global_type_env)
                    .with_module_id(&mn.module_id)
                    .with_raw_module_id(&mn.raw_id);

                let ty = checker.resolve_type_def(&env, &td.type_def).unpack(diags);
                resolved.push((*node, *type_id, ty));
            }

            // Wrap with IsoRec where the body actually references the variable
            for (node, type_id, body) in resolved {
                let NodeId::TypeDecl(raw_id, name) = node else {
                    continue;
                };
                let ty = if body.contains_var(type_id) {
                    Type::IsoRec(type_id, Box::new(body))
                } else {
                    body
                };
                self.global_type_env
                    .insert(GlobalKey::TypeDecl(raw_id.clone(), name.clone()), ty);
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
        let lb = find_let_bind(&mn.body, name).unwrap();

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
                .insert(cache_key, Type::Never());
            self.global_type_env.insert(
                GlobalKey::Global(raw_id.clone(), name.to_string()),
                Type::Never(),
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
                    .and_then(|mn| find_let_bind(&mn.body, name))
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
            let lb = find_let_bind(&mn.body, name).unwrap();
            diags.push(CyclicDependency {
                module_id: mn.module_id.clone(),
                names: names_str.clone(),
                span: lb.var.span(),
            });
            let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
            checker
                .global_cache
                .borrow_mut()
                .insert(cache_key, Type::Never());
            self.global_type_env.insert(
                GlobalKey::Global(raw_id.clone(), name.clone()),
                Type::Never(),
            );
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
                    let lb = find_let_bind(&mn.body, name)?;
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
                && let Some(lb) = find_let_bind(&mn.body, name)
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
        use std::cell::RefCell;
        use std::rc::Rc;

        // Separate non-fn cyclic globals (which get CyclicDependency errors)
        // from fn globals that participate in the recursive group.
        let mut fn_nodes: Vec<(&RawModuleId, &str, &ast::LetBind, usize)> = Vec::new();
        let constraints = Rc::new(RefCell::new(FreeVarConstraints::new()));

        for node in global_nodes {
            let NodeId::Global(raw_id, name) = node else {
                continue;
            };
            let has_self_edge = self.asg.has_self_edge(node);
            let mn = self.asg.module(raw_id).unwrap();
            let lb = find_let_bind(&mn.body, name).unwrap();

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
                    .insert(cache_key, Type::Never());
                self.global_type_env.insert(
                    GlobalKey::Global(raw_id.clone(), name.to_string()),
                    Type::Never(),
                );
                continue;
            }

            // Assign a free type variable and pre-populate the global type env
            // so that cross-module property access (e.g. Other.f) can find it.
            let type_id = next_type_id();
            constraints.borrow_mut().register(type_id);
            self.global_type_env.insert(
                GlobalKey::Global(raw_id.clone(), name.to_string()),
                Type::Var(type_id),
            );
            fn_nodes.push((raw_id, name, lb, type_id));
        }

        // Check all bodies with the free type variables visible.
        let mut body_results: Vec<(&RawModuleId, &str, &ast::LetBind, usize, Type)> = Vec::new();
        for &(raw_id, name, lb, type_id) in &fn_nodes {
            let mn = self.asg.module(raw_id).unwrap();
            let env = TypeEnv::new(&self.global_type_env)
                .with_module_id(&mn.module_id)
                .with_raw_module_id(&mn.raw_id);

            // Set up self-recursion free variable as a local binding.
            let self_env = env.with_free_var(
                lb.var.name.as_str(),
                lb.var.span(),
                type_id,
                constraints.clone(),
            );

            let annotation_ty = lb
                .ty
                .as_ref()
                .map(|te| checker.resolve_type_expr(&self_env, te).unpack(diags));

            let resolved_ty = checker
                .check_expr(&self_env, lb.expr.as_ref(), annotation_ty.as_ref())?
                .unpack(diags);

            let binding_ty = annotation_ty.unwrap_or(resolved_ty);
            body_results.push((raw_id, name, lb, type_id, binding_ty));
        }

        // Build a substitution that maps each member's type variable to its
        // body type.  This resolves cross-references between group members
        // while leaving self-references as Var(type_id), which IsoRec wraps.
        let subst: Vec<(usize, Type)> = body_results
            .iter()
            .map(|(_, _, _, id, ty)| (*id, ty.clone()))
            .collect();

        // Apply substitutions and store final types.
        for (raw_id, name, lb, type_id, body_ty) in &body_results {
            let resolved_ty = body_ty.substitute(&subst);
            let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
            checker
                .global_cache
                .borrow_mut()
                .insert(cache_key, resolved_ty.clone());
            let ty = Type::IsoRec(*type_id, Box::new(resolved_ty.clone()));
            if let Some((cursor, _)) = &lb.var.cursor {
                cursor.set_type(ty.clone());
                cursor.set_identifier(crate::CursorIdentifier::Let(lb.var.name.clone()));
                if let Some(doc) = &lb.doc_comment {
                    cursor.set_description(doc.clone());
                }
            }
            // Store the IsoRec-wrapped type so that references from outside
            // the recursive group see the µ-binder.
            self.global_type_env
                .insert(GlobalKey::Global((*raw_id).clone(), name.to_string()), ty);
        }

        Ok(())
    }

    // ─── Module assembly ────────────────────────────────────────────────────────

    fn assemble_module(
        &mut self,
        checker: &TypeChecker<'_>,
        raw_id: &RawModuleId,
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        let Some(mn) = self.asg.module(raw_id) else {
            return Ok(());
        };

        match &mn.body {
            crate::ModuleBody::Scle(scle) => {
                // Resolve the declared type expression (if any) and check the
                // body against it; if no type_expr is given, synthesize the
                // body's type instead. The resulting type IS the module's
                // value-level type; no `Record` wrapping, no iso-recursion.
                let env = TypeEnv::new(&self.global_type_env)
                    .with_module_id(&mn.module_id)
                    .with_raw_module_id(&mn.raw_id);
                let expected_ty = match (&scle.type_expr, &scle.body) {
                    (Some(type_expr), Some(body)) => {
                        let expected = checker.resolve_type_expr(&env, type_expr).unpack(diags);
                        checker
                            .check_expr(&env, body, Some(&expected))?
                            .unpack(diags);
                        expected
                    }
                    (Some(type_expr), None) => {
                        checker.resolve_type_expr(&env, type_expr).unpack(diags)
                    }
                    (None, Some(body)) => checker.check_expr(&env, body, None)?.unpack(diags),
                    (None, None) => Type::Any(),
                };

                self.global_type_env
                    .insert(GlobalKey::ModuleValue(raw_id.clone()), expected_ty);
                // SCLE modules export no types.
                self.global_type_env.insert(
                    GlobalKey::ModuleTypeLevel(raw_id.clone()),
                    Type::Record(RecordType::default()),
                );
            }
            crate::ModuleBody::File(file_mod) => {
                // Value-level export record.
                let mut exports = RecordType::default();
                for stmt in &file_mod.statements {
                    if let ast::ModStmt::Export(lb) = stmt {
                        let key = GlobalKey::Global(raw_id.clone(), lb.var.name.clone());
                        if let Some(ty) = self.global_type_env.get(&key) {
                            let ty =
                                Type::IsoRec(crate::checker::next_type_id(), Box::new(ty.clone()));
                            exports.insert_with_doc(
                                lb.var.name.clone(),
                                ty,
                                lb.doc_comment.clone(),
                            );
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
                let asg_key = file_mod as *const ast::FileMod;
                checker
                    .import_cache
                    .borrow_mut()
                    .entry(asg_key)
                    .or_insert_with(|| module_ty);

                // Type-level export record.
                let mut type_exports = RecordType::default();
                for stmt in &file_mod.statements {
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
        Ok(())
    }
}

/// Build per-module import alias → RawModuleId maps from the ASG.
fn build_import_maps(asg: &Asg) -> HashMap<RawModuleId, HashMap<String, RawModuleId>> {
    let mut maps = HashMap::new();
    for module_node in asg.modules() {
        let mut aliases = HashMap::new();
        for import in module_node.body.imports() {
            let vars = &import.as_ref().vars;
            if !vars.is_empty() {
                let alias = vars.last().unwrap().name.clone();
                let import_raw_id = resolve_import_path(vars, &module_node.package_id);
                aliases.insert(alias, import_raw_id);
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

fn find_let_bind<'m>(body: &'m crate::ModuleBody, name: &str) -> Option<&'m ast::LetBind> {
    let file_mod = body.as_file_mod()?;
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
    use crate::{InMemoryPackage, Loader, build_default_finder};

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
