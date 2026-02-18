use std::collections::HashMap;

use thiserror::Error;
use tokio::sync::mpsc;

use crate::{ExternFnValue, FnValue, Record, Value, ast};

pub struct EvalEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    globals: Option<&'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>>,
    imports: Option<&'a HashMap<&'a str, (crate::ModuleId, &'a ast::FileMod)>>,
    locals: HashMap<&'a str, Value>,
    stack_depth: u32,
}

impl<'a> EvalEnv<'a> {
    pub fn new() -> Self {
        Self {
            module_id: None,
            globals: None,
            imports: None,
            locals: HashMap::new(),
            stack_depth: 0,
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_globals(&self, globals: &'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>) -> Self {
        Self {
            module_id: self.module_id,
            globals: Some(globals),
            imports: self.imports,
            locals: HashMap::new(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_imports(
        &self,
        imports: &'a HashMap<&'a str, (crate::ModuleId, &'a ast::FileMod)>,
    ) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: Some(imports),
            locals: HashMap::new(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_local(&self, name: &'a str, value: Value) -> Self {
        let mut env = self.inner();
        env.locals.insert(name, value);
        env
    }

    pub fn without_locals(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            locals: HashMap::new(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_stack_frame(&self) -> Result<Self, EvalError> {
        if self.stack_depth >= 50 {
            return Err(EvalError::StackOverflow);
        }

        let mut env = self.inner();
        env.stack_depth += 1;
        Ok(env)
    }

    pub fn lookup_local(&self, name: &str) -> Option<&Value> {
        self.locals.get(name)
    }

    pub fn locals(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.locals.iter().map(|(name, value)| (*name, value))
    }

    pub fn lookup_global(&self, name: &str) -> Option<&crate::Loc<ast::Expr>> {
        self.globals.and_then(|globals| globals.get(name).copied())
    }

    pub fn lookup_import(&self, name: &str) -> Option<(crate::ModuleId, &'a ast::FileMod)> {
        self.imports
            .and_then(|imports| imports.get(name))
            .map(|(module_id, file_mod)| (module_id.clone(), *file_mod))
    }

    pub fn module_id(&self) -> Result<crate::ModuleId, EvalError> {
        self.module_id.cloned().ok_or(EvalError::ModuleIdMissing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnEnv {
    pub module_id: crate::ModuleId,
    pub captures: HashMap<String, Value>,
    pub parameters: Vec<String>,
}

impl FnEnv {
    pub fn as_eval_env<'a>(&'a self, args: &[Value]) -> EvalEnv<'a> {
        let mut env = EvalEnv::new().with_module_id(&self.module_id);

        for (name, value) in &self.captures {
            env = env.with_local(name.as_str(), value.clone());
        }
        for (name, value) in self.parameters.iter().zip(args.iter()) {
            env = env.with_local(name.as_str(), value.clone());
        }

        env
    }
}

pub struct Eval {
    _effects: mpsc::UnboundedSender<Effect>,
    externs: HashMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Print(Value),
}

#[derive(Error, Debug)]
pub enum EvalError {
    #[error("failed to emit effect: {0:?}")]
    EmitEffect(Effect),

    #[error("stack overflow")]
    StackOverflow,

    #[error("module id missing during evaluation")]
    ModuleIdMissing,

    #[error("extern not found: {0}")]
    MissingExtern(String),

    #[error("unexpected value: {0}")]
    UnexpectedValue(Value),
}

pub trait ValueAssertions {
    fn assert_int(self) -> Result<i64, EvalError>;
}

impl ValueAssertions for Value {
    fn assert_int(self) -> Result<i64, EvalError> {
        match self {
            Value::Int(value) => Ok(value),
            other => Err(EvalError::UnexpectedValue(other)),
        }
    }
}

impl ValueAssertions for Option<Value> {
    fn assert_int(self) -> Result<i64, EvalError> {
        self.unwrap_or(Value::Nil).assert_int()
    }
}

impl Eval {
    pub fn new<S: crate::SourceRepo>(effects: mpsc::UnboundedSender<Effect>) -> Self {
        let mut eval = Self {
            _effects: effects,
            externs: HashMap::new(),
        };
        <crate::AnySource<S> as crate::SourceRepo>::register_extern(&mut eval);
        eval
    }

    pub fn add_extern(&mut self, name: impl Into<String>, value: Value) {
        self.externs.insert(name.into(), value);
    }

    pub fn add_extern_fn(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(Vec<Value>) -> Result<Value, EvalError> + Clone + Send + Sync + 'static,
    ) {
        self.add_extern(name, Value::ExternFn(ExternFnValue::new(Box::new(f))));
    }

    pub fn eval_expr(
        &mut self,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
    ) -> Result<Value, EvalError> {
        match expr.as_ref() {
            ast::Expr::Int(int) => Ok(Value::Int(int.value)),
            ast::Expr::Str(str) => Ok(Value::Str(str.value.clone())),
            ast::Expr::Extern(extern_expr) => self
                .externs
                .get(extern_expr.name.as_str())
                .cloned()
                .ok_or_else(|| EvalError::MissingExtern(extern_expr.name.clone())),
            ast::Expr::Let(let_expr) => {
                let bind_value = self.eval_expr(env, let_expr.bind.expr.as_ref())?;
                let inner_env = env.with_local(let_expr.bind.var.name.as_str(), bind_value);
                self.eval_expr(&inner_env, let_expr.expr.as_ref())
            }
            ast::Expr::Fn(fn_expr) => {
                let mut captures = HashMap::new();
                for name in expr.as_ref().free_vars() {
                    captures.insert(name.to_owned(), self.eval_var_name(env, name)?);
                }
                Ok(Value::Fn(FnValue {
                    env: FnEnv {
                        module_id: env.module_id()?,
                        captures,
                        parameters: fn_expr
                            .params
                            .iter()
                            .map(|param| param.var.name.clone())
                            .collect(),
                    },
                    body: *fn_expr.body.clone(),
                }))
            }
            ast::Expr::Call(call_expr) => {
                let args = call_expr
                    .args
                    .iter()
                    .map(|arg| self.eval_expr(env, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                let callee = self.eval_expr(env, call_expr.callee.as_ref())?;

                match callee {
                    Value::Fn(function) => {
                        let call_env = function.env.as_eval_env(&args);
                        self.eval_expr(&call_env, &function.body)
                    }
                    Value::ExternFn(function) => function.call(args),
                    _ => Ok(Value::Nil),
                }
            }
            ast::Expr::Var(var) => self.eval_var_name(env, var.name.as_str()),
            ast::Expr::Record(record_expr) => {
                let mut record = Record::default();
                for field in &record_expr.fields {
                    let value = self.eval_expr(env, &field.expr)?;
                    record.insert(field.var.name.clone(), value);
                }
                Ok(Value::Record(record))
            }
            ast::Expr::Interp(interp_expr) => {
                let mut out = String::new();
                for part in &interp_expr.parts {
                    out.push_str(&self.eval_expr(env, part)?.to_string());
                }
                Ok(Value::Str(out))
            }
            ast::Expr::PropertyAccess(property_access) => {
                let value = self.eval_expr(env, property_access.expr.as_ref())?;
                match value {
                    Value::Record(record) => Ok(record
                        .get(property_access.property.name.as_str())
                        .cloned()
                        .unwrap_or(Value::Nil)),
                    _ => Ok(Value::Nil),
                }
            }
        }
    }

    fn eval_var_name(&mut self, env: &EvalEnv<'_>, name: &str) -> Result<Value, EvalError> {
        if let Some(local_value) = env.lookup_local(name) {
            return Ok(local_value.clone());
        }
        if let Some(global_expr) = env.lookup_global(name) {
            let global_env = env.without_locals().with_stack_frame()?;
            return self.eval_expr(&global_env, global_expr);
        }
        if let Some((target_module_id, import_file_mod)) = env.lookup_import(name) {
            let import_env = EvalEnv::new().with_module_id(&target_module_id);
            return self.eval_file_mod(&import_env, import_file_mod);
        }
        Ok(Value::Nil)
    }

    pub fn eval_stmt(
        &mut self,
        env: &EvalEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Option<(String, Value)>, EvalError> {
        match stmt {
            ast::ModStmt::Print(print_stmt) => {
                let value = self.eval_expr(env, &print_stmt.expr)?;
                self._effects
                    .send(Effect::Print(value))
                    .map_err(|send_error| EvalError::EmitEffect(send_error.0))?;
                Ok(None)
            }
            ast::ModStmt::Let(_) | ast::ModStmt::Import(_) => Ok(None),
            ast::ModStmt::Export(let_bind) => {
                let value = self.eval_expr(env, let_bind.expr.as_ref())?;
                Ok(Some((let_bind.var.name.clone(), value)))
            }
            ast::ModStmt::Expr(expr) => {
                let _ = self.eval_expr(env, expr)?;
                Ok(None)
            }
        }
    }

    pub fn eval_file_mod(
        &mut self,
        env: &EvalEnv<'_>,
        file_mod: &ast::FileMod,
    ) -> Result<Value, EvalError> {
        let globals = file_mod.find_globals();
        let env = env.with_globals(&globals);
        let mut exports = Record::default();

        for statement in &file_mod.statements {
            if let Some((name, value)) = self.eval_stmt(&env, statement)? {
                exports.insert(name, value);
            }
        }

        Ok(Value::Record(exports))
    }
}
