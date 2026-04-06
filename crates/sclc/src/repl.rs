use std::collections::HashMap;

use tokio::sync::mpsc;

use crate::{
    DiagList, Diagnosed, Effect, Eval, EvalEnv, EvalError, ModuleId, Program, RecordType,
    TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv,
};

#[derive(Clone)]
pub struct ReplState {
    line_number: usize,
    program: Program,
    effects_tx: mpsc::UnboundedSender<Effect>,
    bindings: HashMap<String, (Type, TrackedValue)>,
    type_defs: HashMap<String, Type>,
    namespace: String,
}

pub enum ReplOutcome {
    /// `let x = ...` or `export let x = ...` — binding was stored.
    Binding { name: String, ty: Type },
    /// Bare expression — value was computed.
    Value { value: TrackedValue },
    /// `type Foo = ...` — type def was stored.
    TypeDef { name: String },
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

impl ReplState {
    pub fn new(
        program: Program,
        effects_tx: mpsc::UnboundedSender<Effect>,
        namespace: String,
    ) -> Self {
        Self {
            line_number: 0,
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

    pub fn program_mut(&mut self) -> &mut Program {
        &mut self.program
    }

    pub fn effects_tx(&self) -> &mpsc::UnboundedSender<Effect> {
        &self.effects_tx
    }

    pub fn bindings(&self) -> &HashMap<String, (Type, TrackedValue)> {
        &self.bindings
    }

    pub fn type_defs(&self) -> &HashMap<String, Type> {
        &self.type_defs
    }

    /// Increment the line counter and return a module ID for this REPL line.
    ///
    /// The module ID is scoped under the package ID so that relative path
    /// expressions (e.g. `./file`) resolve correctly against the repo root.
    pub fn next_line_module_id(&mut self) -> ModuleId {
        self.line_number += 1;
        let mut segments = self
            .program
            .self_package_id()
            .map(|id| id.as_slice().to_vec())
            .unwrap_or_default();
        segments.push(format!("Repl{}", self.line_number));
        ModuleId::new(segments)
    }

    /// Build a `TypeEnv` from current bindings and type defs.
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

    /// Build an `EvalEnv` from current bindings.
    pub fn eval_env<'a>(&'a self, module_id: &'a ModuleId) -> EvalEnv<'a> {
        self.bindings.iter().fold(
            EvalEnv::new().with_module_id(module_id),
            |env, (name, (_, value))| env.with_local(name.as_str(), value.clone()),
        )
    }

    /// Process a non-import statement. Returns the outcome on success.
    ///
    /// The statement is type-checked and evaluated, and any resulting
    /// binding or type def is stored in the REPL state.
    pub fn process_statement(
        &mut self,
        statement: &crate::ast::ModStmt,
        module_id: &ModuleId,
    ) -> Result<ReplOutcome, ReplError> {
        let type_env = self.type_env(module_id);
        let eval = Eval::new(
            &self.program,
            self.effects_tx.clone(),
            self.namespace.clone(),
        );
        let eval_env = self.eval_env(module_id);

        match statement {
            crate::ast::ModStmt::Import(_) => {
                panic!("imports must be handled by the caller, not process_statement")
            }
            crate::ast::ModStmt::Let(let_bind) | crate::ast::ModStmt::Export(let_bind) => {
                let checker = TypeChecker::new(&self.program);
                let diagnosed = checker.check_global_let_bind(&type_env, let_bind)?;
                let ty = check_diagnosed(diagnosed)?;
                let value = eval.eval_expr(&eval_env, &let_bind.expr)?;
                let name = let_bind.var.name.clone();
                self.bindings.insert(name.clone(), (ty.clone(), value));
                Ok(ReplOutcome::Binding { name, ty })
            }
            crate::ast::ModStmt::Expr(expr) => {
                let checker = TypeChecker::new(&self.program);
                let diagnosed = checker.check_stmt(&type_env, statement)?;
                check_diagnosed(diagnosed)?;
                let value = eval.eval_expr(&eval_env, expr)?;
                Ok(ReplOutcome::Value { value })
            }
            crate::ast::ModStmt::TypeDef(type_def)
            | crate::ast::ModStmt::ExportTypeDef(type_def) => {
                let checker = TypeChecker::new(&self.program);
                let diagnosed = checker.resolve_type_def(&type_env, type_def);
                let ty = check_diagnosed(diagnosed)?;
                let name = type_def.var.name.clone();
                self.type_defs.insert(name.clone(), ty);
                Ok(ReplOutcome::TypeDef { name })
            }
        }
    }

    /// Register an already-resolved import into the REPL state.
    ///
    /// Call this after loading the import source, resolving imports,
    /// type-checking the module, evaluating it, and extracting type exports.
    pub fn register_import(
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

    /// Create an `Eval` instance for evaluating imports.
    pub fn make_eval(&self) -> Eval<'_> {
        Eval::new(
            &self.program,
            self.effects_tx.clone(),
            self.namespace.clone(),
        )
    }
}

/// Check a `Diagnosed<T>` and return `Err(ReplError::Diagnostics)` if it contains errors.
fn check_diagnosed<T>(diagnosed: Diagnosed<T>) -> Result<T, ReplError> {
    if diagnosed.diags().has_errors() {
        let mut diags = DiagList::new();
        diagnosed.unpack(&mut diags);
        Err(ReplError::Diagnostics(diags))
    } else {
        Ok(diagnosed.into_inner())
    }
}
