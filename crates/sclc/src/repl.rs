use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::{
    CompilationUnit, DiagList, Diagnosed, Effect, Eval, EvalEnv, EvalError, FileMod, ModuleId,
    Program, RecordType, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv,
};

#[derive(Clone)]
pub struct Repl {
    line_number: usize,
    unit: CompilationUnit,
    program: Program,
    effects_tx: mpsc::UnboundedSender<Effect>,
    bindings: HashMap<String, (Type, TrackedValue)>,
    type_defs: HashMap<String, Type>,
    namespace: String,
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
    Resolve(crate::ResolveError),
    ResolveImport(crate::ResolveImportError),
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

impl From<crate::ResolveImportError> for ReplError {
    fn from(err: crate::ResolveImportError) -> Self {
        ReplError::ResolveImport(err)
    }
}

impl From<crate::ResolveError> for ReplError {
    fn from(err: crate::ResolveError) -> Self {
        ReplError::Resolve(err)
    }
}

impl Repl {
    pub fn new(
        program: Program,
        effects_tx: mpsc::UnboundedSender<Effect>,
        namespace: String,
    ) -> Self {
        let unit = CompilationUnit::from_program(&program);
        Self::from_parts(unit, program, effects_tx, namespace)
    }

    pub fn from_parts(
        unit: CompilationUnit,
        program: Program,
        effects_tx: mpsc::UnboundedSender<Effect>,
        namespace: String,
    ) -> Self {
        Self {
            line_number: 0,
            unit,
            program,
            effects_tx,
            bindings: HashMap::new(),
            type_defs: HashMap::new(),
            namespace,
        }
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn unit(&self) -> &CompilationUnit {
        &self.unit
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

    pub fn replace_user_source(&mut self, source: impl crate::SourceRepo + 'static) {
        self.program.replace_user_source(source);
        self.sync_unit();
    }

    pub async fn preload_path_dirs(&mut self, dirs: impl IntoIterator<Item = PathBuf>) {
        self.program.preload_path_dirs(dirs).await;
        self.sync_unit();
    }

    pub fn next_line_module_id(&mut self) -> ModuleId {
        self.line_number += 1;
        let package = self.program.self_package_id().cloned().unwrap_or_default();
        ModuleId::new(package, vec![format!("Repl{}", self.line_number)])
    }

    pub fn type_env<'a>(&'a self, module_id: &'a ModuleId) -> TypeEnv<'a> {
        let env = self.bindings.iter().fold(
            TypeEnv::new().with_module_id(module_id),
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
            EvalEnv::new().with_module_id(module_id),
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

    fn sync_unit(&mut self) {
        self.unit = CompilationUnit::from_program(&self.program);
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
        let import_path = self
            .split_import_segments(&raw_segments)
            .unwrap_or_else(|| ModuleId::new(crate::PackageId::default(), raw_segments.clone()));
        let alias = import_stmt
            .as_ref()
            .vars
            .last()
            .expect("import path contains at least one segment")
            .as_ref()
            .name
            .clone();

        let mut diags = DiagList::new();
        self.unit.resolve(&import_path).await?.unpack(&mut diags);
        self.program = self.unit.program().clone();

        let Some(file_mod) = self.unit.module(&import_path).cloned() else {
            diags.push(invalid_import_diag(
                source_module_id.clone(),
                import_path.clone(),
                import_stmt,
            ));
            return Err(ReplError::Diagnostics(diags));
        };

        let checker = TypeChecker::new(&self.unit);
        let type_env = TypeEnv::new().with_module_id(&import_path);
        let ty = checker
            .check_file_mod(&type_env, &file_mod)?
            .unpack(&mut diags);
        let type_exports = checker
            .type_level_exports(&type_env, &file_mod)
            .unpack(&mut diags);

        if diags.has_errors() {
            return Err(ReplError::Diagnostics(diags));
        }

        let eval = Eval::new(&self.unit, self.effects_tx.clone(), self.namespace.clone());
        let eval_env = EvalEnv::new().with_module_id(&import_path);
        let value = eval.eval_file_mod(&eval_env, &file_mod)?;

        self.register_import(alias, ty, value, type_exports);
        Ok(ReplOutcome::Import {
            module_id: import_path,
        })
    }

    async fn preload_paths_for_statement(
        &mut self,
        statement: &crate::ModStmt,
        module_id: &ModuleId,
    ) {
        let file_mod = FileMod {
            statements: vec![statement.clone()],
        };
        let mut collector = crate::CollectPaths::new();
        crate::visit_file_mod(&mut collector, &file_mod);

        let self_package_id = self.program.self_package_id().cloned();
        let dirs: HashSet<PathBuf> = collector
            .paths
            .iter()
            .filter_map(|path_expr| {
                let resolved = path_expr.resolve_with_context(module_id, self_package_id.as_ref());
                let resolved_path = std::path::Path::new(&resolved);
                let parent = resolved_path.parent()?;
                let parent_str = parent.to_string_lossy();
                let parent_rel = parent_str.strip_prefix('/').unwrap_or(&parent_str);
                Some(PathBuf::from(parent_rel))
            })
            .collect();

        if !dirs.is_empty() {
            self.program.preload_path_dirs(dirs).await;
            self.sync_unit();
        }
    }

    fn process_statement(
        &mut self,
        statement: &crate::ast::ModStmt,
        module_id: &ModuleId,
    ) -> Result<ReplOutcome, ReplError> {
        let type_env = self.type_env(module_id);
        let eval = Eval::new(&self.unit, self.effects_tx.clone(), self.namespace.clone());
        let eval_env = self.eval_env(module_id);

        match statement {
            crate::ast::ModStmt::Import(_) => {
                panic!("imports must be handled by process_import, not process_statement")
            }
            crate::ast::ModStmt::Let(let_bind) | crate::ast::ModStmt::Export(let_bind) => {
                let checker = TypeChecker::new(&self.unit);
                let diagnosed = checker.check_global_let_bind(&type_env, let_bind)?;
                let ty = check_diagnosed(diagnosed)?;
                let value = eval.eval_expr(&eval_env, &let_bind.expr)?;
                let name = let_bind.var.name.clone();
                self.bindings.insert(name.clone(), (ty.clone(), value));
                Ok(ReplOutcome::Binding { name, ty })
            }
            crate::ast::ModStmt::Expr(expr) => {
                let checker = TypeChecker::new(&self.unit);
                let diagnosed = checker.check_stmt(&type_env, statement)?;
                check_diagnosed(diagnosed)?;
                let value = eval.eval_expr(&eval_env, expr)?;
                Ok(ReplOutcome::Value { value })
            }
            crate::ast::ModStmt::TypeDef(type_def)
            | crate::ast::ModStmt::ExportTypeDef(type_def) => {
                let checker = TypeChecker::new(&self.unit);
                let diagnosed = checker.resolve_type_def(&type_env, type_def);
                let ty = check_diagnosed(diagnosed)?;
                let name = type_def.var.name.clone();
                self.type_defs.insert(name.clone(), ty);
                Ok(ReplOutcome::TypeDef { name })
            }
        }
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

    fn split_import_segments(&self, raw_segments: &[String]) -> Option<ModuleId> {
        let segments = if raw_segments.first().map(String::as_str) == Some("Self") {
            let mut resolved = self
                .program
                .self_package_id()
                .cloned()
                .unwrap_or_default()
                .as_slice()
                .to_vec();
            resolved.extend(raw_segments[1..].iter().cloned());
            resolved
        } else {
            raw_segments.to_vec()
        };

        let package = self
            .program
            .package_names()
            .filter(|package_name| segments.starts_with(package_name.as_slice()))
            .max_by_key(|package_name| package_name.len())
            .cloned()?;
        let pkg_len = package.len();
        Some(ModuleId::new(package, segments[pkg_len..].to_vec()))
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
