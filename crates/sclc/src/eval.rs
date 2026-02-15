use std::collections::HashMap;

use thiserror::Error;
use tokio::sync::mpsc;

use crate::{Record, Value, ast};

pub struct EvalEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    globals: Option<&'a HashMap<&'a str, &'a ast::Expr>>,
    locals: HashMap<&'a str, Value>,
}

impl<'a> EvalEnv<'a> {
    pub fn new() -> Self {
        Self {
            module_id: None,
            globals: None,
            locals: HashMap::new(),
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            locals: self.locals.clone(),
        }
    }

    pub fn with_globals(&self, globals: &'a HashMap<&'a str, &'a ast::Expr>) -> Self {
        Self {
            module_id: self.module_id,
            globals: Some(globals),
            locals: HashMap::new(),
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            globals: self.globals,
            locals: self.locals.clone(),
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
            locals: HashMap::new(),
        }
    }

    pub fn lookup_local(&self, name: &str) -> Option<&Value> {
        self.locals.get(name)
    }

    pub fn lookup_global(&self, name: &str) -> Option<&ast::Expr> {
        self.globals.and_then(|globals| globals.get(name).copied())
    }
}

pub struct Eval {
    _effects: mpsc::UnboundedSender<Effect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Print(Value),
}

#[derive(Error, Debug)]
pub enum EvalError {
    #[error("evaluation is not implemented yet for statement: {0:?}")]
    UnimplementedStmt(ast::ModStmt),

    #[error("failed to emit effect: {0:?}")]
    EmitEffect(Effect),
}

impl Eval {
    pub fn new(effects: mpsc::UnboundedSender<Effect>) -> Self {
        Self { _effects: effects }
    }

    pub fn eval_expr(&mut self, env: &EvalEnv<'_>, expr: &ast::Expr) -> Result<Value, EvalError> {
        match expr {
            ast::Expr::Int(int) => Ok(Value::Int(int.value)),
            ast::Expr::Let(let_expr) => {
                let bind_value = self.eval_expr(env, &let_expr.bind.expr)?;
                let inner_env = env.with_local(let_expr.bind.var.name.as_str(), bind_value);
                self.eval_expr(&inner_env, &let_expr.expr)
            }
            ast::Expr::Var(var) => {
                if let Some(local_value) = env.lookup_local(var.name.as_str()) {
                    return Ok(local_value.clone());
                }
                if let Some(global_expr) = env.lookup_global(var.name.as_str()) {
                    let global_env = env.without_locals();
                    return self.eval_expr(&global_env, global_expr);
                }
                Ok(Value::Nil)
            }
            ast::Expr::Record(record_expr) => {
                let mut record = Record::default();
                for field in &record_expr.fields {
                    let value = self.eval_expr(env, &field.expr)?;
                    record.insert(field.var.name.clone(), value);
                }
                Ok(Value::Record(record))
            }
            ast::Expr::PropertyAccess(property_access) => {
                let value = self.eval_expr(env, &property_access.expr)?;
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

    pub fn eval_stmt(&mut self, env: &EvalEnv<'_>, stmt: &ast::ModStmt) -> Result<(), EvalError> {
        match stmt {
            ast::ModStmt::Print(print_stmt) => {
                let value = self.eval_expr(env, &print_stmt.expr)?;
                self._effects
                    .send(Effect::Print(value))
                    .map_err(|send_error| EvalError::EmitEffect(send_error.0))?;
                Ok(())
            }
            ast::ModStmt::Let(_) => Ok(()),
            ast::ModStmt::Expr(expr) => {
                let _ = self.eval_expr(env, expr)?;
                Ok(())
            }
            s => Err(EvalError::UnimplementedStmt(s.clone())),
        }
    }

    pub fn eval_file_mod(
        &mut self,
        env: &EvalEnv<'_>,
        file_mod: &ast::FileMod,
    ) -> Result<Value, EvalError> {
        let globals = file_mod.find_globals();
        let env = env.with_globals(&globals);

        for statement in &file_mod.statements {
            self.eval_stmt(&env, statement)?;
        }

        Ok(Value::Record(Record::default()))
    }
}
