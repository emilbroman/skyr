use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::v2::{Asg, Loader, PackageFinder, build_default_finder};
use crate::{
    DiagList, Diagnosed, Effect, Eval, EvalCtx, EvalEnv, EvalError, GlobalEvalEnv, GlobalTypeEnv,
    ModuleId, PackageId, RecordType, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv,
    Value, ast,
};

#[derive(Clone)]
pub struct Repl {
    line_number: usize,
    finder: Arc<dyn PackageFinder>,
    /// Cached ASG built from all resolved imports so far.
    cached_asg: Asg,
    /// Module map derived from the ASG (rebuilt when imports change).
    modules: HashMap<ModuleId, ast::FileMod>,
    /// Package names from the ASG.
    package_names: Vec<PackageId>,
    effects_tx: mpsc::UnboundedSender<Effect>,
    bindings: HashMap<String, (Type, TrackedValue)>,
    type_defs: HashMap<String, Type>,
    global_type_env: GlobalTypeEnv,
    global_eval_env: GlobalEvalEnv,
    namespace: String,
    package_id: PackageId,
    /// Import entry points accumulated across REPL lines.
    import_entries: Vec<Vec<String>>,
}

pub enum ReplOutcome {
    Binding { name: String, ty: Type },
    Value { value: TrackedValue },
    TypeDef { name: String },
    Import { module_id: ModuleId },
}

pub enum ReplError {
    Diagnostics(DiagList),
    TypeCheck(TypeCheckError),
    Eval(EvalError),
}

impl From<TypeCheckError> for ReplError {
    fn from(err: TypeCheckError) -> Self {
        ReplError::TypeCheck(err)
    }
}

impl From<EvalError> for ReplError {
    fn from(err: EvalError) -> Self {
        ReplError::Eval(err)
    }
}

impl Repl {
    pub fn new(
        finder: Arc<dyn PackageFinder>,
        package_id: PackageId,
        effects_tx: mpsc::UnboundedSender<Effect>,
        namespace: String,
    ) -> Self {
        Self {
            line_number: 0,
            finder,
            cached_asg: Asg::new(),
            modules: HashMap::new(),
            package_names: Vec::new(),
            effects_tx,
            bindings: HashMap::new(),
            type_defs: HashMap::new(),
            global_type_env: GlobalTypeEnv::default(),
            global_eval_env: GlobalEvalEnv::default(),
            namespace,
            package_id,
            import_entries: Vec::new(),
        }
    }

    /// Get a reference to the module map (for creating TypeCheckers).
    pub fn modules(&self) -> &HashMap<ModuleId, ast::FileMod> {
        &self.modules
    }

    /// Get the package names.
    pub fn package_names(&self) -> &[PackageId] {
        &self.package_names
    }

    pub fn effects_tx(&self) -> &mpsc::UnboundedSender<Effect> {
        &self.effects_tx
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn bindings(&self) -> &HashMap<String, (Type, TrackedValue)> {
        &self.bindings
    }

    pub fn type_defs(&self) -> &HashMap<String, Type> {
        &self.type_defs
    }

    /// Replace the user package in the finder. Rebuilds the finder from the
    /// new package while preserving the standard library.
    pub fn replace_user_package(&mut self, user_package: Arc<dyn crate::v2::Package>) {
        self.package_id = user_package.id();
        self.finder = build_default_finder(user_package);
    }

    pub fn package_id(&self) -> &PackageId {
        &self.package_id
    }

    pub fn next_line_module_id(&mut self) -> ModuleId {
        self.line_number += 1;
        ModuleId::new(
            self.package_id.clone(),
            vec![format!("Repl{}", self.line_number)],
        )
    }

    pub fn type_env<'a>(&'a self, module_id: &'a ModuleId) -> TypeEnv<'a> {
        let env = self.bindings.iter().fold(
            TypeEnv::new(&self.global_type_env).with_module_id(module_id),
            |env, (name, (ty, _))| {
                env.with_local(name.as_str(), crate::Span::default(), ty.clone())
            },
        );
        self.type_defs.iter().fold(env, |env, (name, ty)| {
            env.with_type_level(name.clone(), ty.clone(), None)
        })
    }

    pub fn eval_env<'a>(&'a self, module_id: &'a ModuleId) -> EvalEnv<'a> {
        self.bindings.iter().fold(
            EvalEnv::new(&self.global_eval_env).with_module_id(module_id),
            |env, (name, (_, value))| env.with_local(name.as_str(), value.clone()),
        )
    }

    pub async fn process(&mut self, line: String) -> Result<Option<ReplOutcome>, ReplError> {
        let module_id = self.next_line_module_id();
        let parsed = crate::parse_repl_line(&line, &module_id);
        let repl_line = check_diagnosed(parsed)?;

        let Some(repl_line) = repl_line else {
            return Ok(None);
        };
        let Some(statement) = &repl_line.statement else {
            return Ok(None);
        };

        match statement {
            crate::ModStmt::Import(import_stmt) => {
                self.process_import(&module_id, import_stmt).await.map(Some)
            }
            _ => {
                self.preload_paths_for_statement(statement, &module_id)
                    .await;
                self.process_statement(statement, &module_id).map(Some)
            }
        }
    }

    async fn process_import(
        &mut self,
        source_module_id: &ModuleId,
        import_stmt: &crate::Loc<crate::ImportStmt>,
    ) -> Result<ReplOutcome, ReplError> {
        let raw_segments: Vec<String> = import_stmt
            .as_ref()
            .vars
            .iter()
            .map(|var| var.as_ref().name.clone())
            .collect();
        let resolved_segments =
            resolve_self_import_segments(raw_segments.clone(), &self.package_id);
        let alias = import_stmt
            .as_ref()
            .vars
            .last()
            .expect("import path contains at least one segment")
            .as_ref()
            .name
            .clone();

        // Add this import to our accumulated entries.
        self.import_entries.push(resolved_segments.clone());

        // Rebuild the ASG with all accumulated imports.
        self.rebuild_asg().await?;

        // Determine the module ID for the import.
        let import_path = self.find_module_id(&resolved_segments);

        let Some(file_mod) = self.modules.get(&import_path).cloned() else {
            let mut diags = DiagList::new();
            diags.push(invalid_import_diag(
                source_module_id.clone(),
                import_path.clone(),
                import_stmt,
            ));
            return Err(ReplError::Diagnostics(diags));
        };

        let mut diags = DiagList::new();
        let checker = TypeChecker::from_modules(&self.modules, self.package_names.clone());
        let type_env = TypeEnv::new(&self.global_type_env).with_module_id(&import_path);
        let ty = checker
            .check_file_mod(&type_env, &file_mod)?
            .unpack(&mut diags);
        let type_exports = checker
            .type_level_exports(&type_env, &file_mod)
            .unpack(&mut diags);

        if diags.has_errors() {
            return Err(ReplError::Diagnostics(diags));
        }

        let externs = self.collect_externs();
        let eval = Eval::from_externs(
            externs,
            EvalCtx::new(self.effects_tx.clone(), &self.namespace),
        );
        let eval_env = EvalEnv::new(&self.global_eval_env).with_module_id(&import_path);
        let value = Self::eval_file_mod_via_asg(&eval, &eval_env, &file_mod)?;

        self.register_import(alias, ty, value, type_exports);
        Ok(ReplOutcome::Import {
            module_id: import_path,
        })
    }

    /// Rebuild the cached ASG and module map from all accumulated import entries.
    async fn rebuild_asg(&mut self) -> Result<(), ReplError> {
        let mut loader = Loader::new(Arc::clone(&self.finder));

        for entry in &self.import_entries {
            let entry_refs: Vec<&str> = entry.iter().map(String::as_str).collect();
            if let Err(e) = loader.resolve(&entry_refs).await {
                // Non-fatal: log but continue
                eprintln!("repl: failed to resolve import: {e}");
            }
        }

        let diagnosed = loader.finish();
        let mut diags = DiagList::new();
        self.cached_asg = diagnosed.unpack(&mut diags);
        self.modules = self
            .cached_asg
            .modules()
            .map(|mn| (mn.module_id.clone(), mn.file_mod.clone()))
            .collect();
        self.package_names = self.cached_asg.packages().keys().cloned().collect();

        if diags.has_errors() {
            return Err(ReplError::Diagnostics(diags));
        }

        Ok(())
    }

    /// Find the ModuleId that the loader resolved for a set of raw segments.
    fn find_module_id(&self, segments: &[String]) -> ModuleId {
        let raw_id: Vec<String> = segments.to_vec();
        // Search the ASG's modules for a matching raw ID.
        for module_node in self.cached_asg.modules() {
            let node_raw: Vec<String> = module_node
                .module_id
                .package
                .as_slice()
                .iter()
                .cloned()
                .chain(module_node.module_id.path.iter().cloned())
                .collect();
            if node_raw == raw_id {
                return module_node.module_id.clone();
            }
        }
        // Fallback: construct a ModuleId with the package ID prefix.
        let pkg_len = self.package_id.len();
        if segments.len() > pkg_len {
            ModuleId::new(
                PackageId::from(segments[..pkg_len].to_vec()),
                segments[pkg_len..].to_vec(),
            )
        } else {
            ModuleId::new(PackageId::default(), segments.to_vec())
        }
    }

    async fn preload_paths_for_statement(
        &mut self,
        statement: &crate::ModStmt,
        _module_id: &ModuleId,
    ) {
        // In v2, path loading is handled by the Package trait's lookup/load
        // methods. No explicit preloading needed.
        let _ = statement;
    }

    fn process_statement(
        &mut self,
        statement: &crate::ast::ModStmt,
        module_id: &ModuleId,
    ) -> Result<ReplOutcome, ReplError> {
        let type_env = self.type_env(module_id);
        let externs = self.collect_externs();
        let eval = Eval::from_externs(
            externs,
            EvalCtx::new(self.effects_tx.clone(), &self.namespace),
        );
        let eval_env = self.eval_env(module_id);

        match statement {
            crate::ast::ModStmt::Import(_) => {
                panic!("imports must be handled by process_import, not process_statement")
            }
            crate::ast::ModStmt::Let(let_bind) | crate::ast::ModStmt::Export(let_bind) => {
                let checker = TypeChecker::from_modules(&self.modules, self.package_names.clone());
                let diagnosed = checker.check_global_let_bind(&type_env, let_bind)?;
                let ty = check_diagnosed(diagnosed)?;
                let value = eval.eval_expr(&eval_env, &let_bind.expr)?;
                let name = let_bind.var.name.clone();
                self.bindings.insert(name.clone(), (ty.clone(), value));
                Ok(ReplOutcome::Binding { name, ty })
            }
            crate::ast::ModStmt::Expr(expr) => {
                let checker = TypeChecker::from_modules(&self.modules, self.package_names.clone());
                let diagnosed = checker.check_stmt(&type_env, statement)?;
                check_diagnosed(diagnosed)?;
                let value = eval.eval_expr(&eval_env, expr)?;
                Ok(ReplOutcome::Value { value })
            }
            crate::ast::ModStmt::TypeDef(type_def)
            | crate::ast::ModStmt::ExportTypeDef(type_def) => {
                let checker = TypeChecker::from_modules(&self.modules, self.package_names.clone());
                let diagnosed = checker.resolve_type_def(&type_env, type_def);
                let ty = check_diagnosed(diagnosed)?;
                let name = type_def.var.name.clone();
                self.type_defs.insert(name.clone(), ty);
                Ok(ReplOutcome::TypeDef { name })
            }
        }
    }

    /// Collect extern values from all packages in the cached ASG.
    fn collect_externs(&self) -> std::collections::HashMap<String, Value> {
        let mut externs = std::collections::HashMap::new();
        for pkg in self.cached_asg.packages().values() {
            pkg.register_externs(&mut externs);
        }
        externs
    }

    /// Evaluate a file module by running each statement, collecting exports.
    ///
    /// This replaces the removed `Eval::eval_file_mod` for the REPL's import
    /// evaluation path. It processes statements sequentially without import
    /// resolution (imports are handled by the ASG-level evaluator in production;
    /// the REPL evaluates one module at a time).
    fn eval_file_mod_via_asg(
        eval: &Eval<'_>,
        env: &EvalEnv<'_>,
        file_mod: &crate::ast::FileMod,
    ) -> Result<TrackedValue, EvalError> {
        use std::collections::BTreeSet;
        let globals = file_mod.find_globals();
        let mut env = env.with_globals(&globals);

        // Build intra-module dep graph and compute SCCs for eval ordering.
        let dep_graph = crate::dep_graph::build_intra_module_value_dep_graph(&globals);
        let sccs = dep_graph.compute_sccs();

        // Pre-evaluate mutually recursive function SCCs.
        for scc in &sccs {
            if scc.len() > 1 {
                let all_fns = scc.iter().all(|bid| {
                    let (_, expr, _) = globals[bid.name.as_str()];
                    matches!(expr.as_ref(), crate::ast::Expr::Fn(_))
                });
                if all_fns {
                    let group = Self::build_recursive_fn_group(eval, &env, scc, &globals)?;
                    for (name, fn_val) in group {
                        env = env.with_precomputed(name, TrackedValue::new(Value::Fn(fn_val)));
                    }
                }
            }
        }

        let mut exports = crate::Record::default();
        let mut dependencies = BTreeSet::new();
        for statement in &file_mod.statements {
            if let Some((name, value)) = eval.eval_stmt(&env, statement)? {
                dependencies.extend(value.dependencies.clone());
                exports.insert(name, value.value);
            }
        }
        Ok(crate::eval::with_dependencies(
            Value::Record(exports),
            dependencies,
        ))
    }

    /// Build FnValues for a mutually recursive group of function globals.
    fn build_recursive_fn_group(
        eval: &Eval<'_>,
        env: &EvalEnv<'_>,
        scc: &[crate::dep_graph::BindingId],
        globals: &std::collections::HashMap<
            &str,
            (crate::Span, &crate::Loc<crate::ast::Expr>, Option<&str>),
        >,
    ) -> Result<Vec<(String, crate::FnValue)>, EvalError> {
        use std::collections::HashSet;
        let fn_module_id = env.module_id()?;
        let scc_names: HashSet<&str> = scc.iter().map(|bid| bid.name.as_str()).collect();

        let mut preliminary: Vec<(String, crate::FnValue)> = Vec::new();
        for bid in scc {
            let (_, global_expr, _) = globals[bid.name.as_str()];
            let crate::ast::Expr::Fn(fn_expr) = global_expr.as_ref() else {
                unreachable!("all SCC members validated as functions");
            };
            let free_vars = global_expr.as_ref().free_vars();
            let parameters: Vec<String> =
                fn_expr.params.iter().map(|p| p.var.name.clone()).collect();
            let body = fn_expr
                .body
                .as_ref()
                .map(|b| *b.clone())
                .unwrap_or_else(|| crate::Loc::new(crate::ast::Expr::Nil, crate::Span::default()));

            let mut captures = std::collections::HashMap::new();
            for fv in &free_vars {
                if !scc_names.contains(fv) {
                    captures.insert(fv.to_string(), eval.eval_var_name(env, fv)?);
                }
            }

            preliminary.push((
                bid.name.clone(),
                crate::FnValue {
                    env: crate::FnEnv {
                        module_id: fn_module_id.clone(),
                        captures,
                        parameters,
                        self_name: None,
                        recursive_group: None,
                    },
                    body,
                },
            ));
        }

        let shared_group = std::sync::Arc::new(preliminary.clone());
        for (_, fn_val) in &mut preliminary {
            fn_val.env.recursive_group = Some(shared_group.clone());
        }
        Ok(preliminary)
    }

    fn register_import(
        &mut self,
        alias: String,
        ty: Type,
        value: TrackedValue,
        type_exports: RecordType,
    ) {
        self.bindings.insert(alias.clone(), (ty, value));
        if type_exports.iter().next().is_some() {
            self.type_defs.insert(alias, Type::Record(type_exports));
        }
    }
}

fn invalid_import_diag(
    source_module_id: ModuleId,
    import_path: ModuleId,
    import_stmt: &crate::Loc<crate::ImportStmt>,
) -> crate::InvalidImport {
    let vars = &import_stmt.as_ref().vars;
    let path_span = crate::Span::new(
        vars.first()
            .expect("import has at least one segment")
            .span()
            .start(),
        vars.last()
            .expect("import has at least one segment")
            .span()
            .end(),
    );

    crate::InvalidImport {
        source_module_id,
        import_path,
        path_span,
    }
}

fn check_diagnosed<T>(diagnosed: Diagnosed<T>) -> Result<T, ReplError> {
    if diagnosed.diags().has_errors() {
        let mut diags = DiagList::new();
        diagnosed.unpack(&mut diags);
        Err(ReplError::Diagnostics(diags))
    } else {
        Ok(diagnosed.into_inner())
    }
}

fn resolve_self_import_segments(segments: Vec<String>, current_package: &PackageId) -> Vec<String> {
    if segments.first().map(String::as_str) == Some("Self") {
        let mut result: Vec<String> = current_package.as_slice().to_vec();
        result.extend(segments[1..].iter().cloned());
        return result;
    }
    segments
}
