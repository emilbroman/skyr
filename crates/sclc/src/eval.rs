use thiserror::Error;
use tokio::sync::mpsc;

use crate::{Record, Value, ast};

pub struct Eval {
    _effects: mpsc::UnboundedSender<Effect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Print(Value),
}

#[derive(Error, Debug)]
pub enum EvalError {
    #[error("evaluation is not implemented yet for expression: {0:?}")]
    NotImplemented(ast::Expr),

    #[error("failed to emit effect: {0:?}")]
    EmitEffect(Effect),
}

impl Eval {
    pub fn new(effects: mpsc::UnboundedSender<Effect>) -> Self {
        Self { _effects: effects }
    }

    pub fn eval_expr(&mut self, expr: ast::Expr) -> Result<Value, EvalError> {
        match expr {
            ast::Expr::Int(int) => Ok(Value::Int(int.value)),
            expr => Err(EvalError::NotImplemented(expr)),
        }
    }

    pub fn eval_file_mod(&mut self, file_mod: &ast::FileMod) -> Result<Value, EvalError> {
        for statement in &file_mod.statements {
            match statement {
                ast::ModStmt::Import(_) => continue,
                ast::ModStmt::Print(print_stmt) => {
                    let value = self.eval_expr(print_stmt.expr.clone())?;
                    self._effects
                        .send(Effect::Print(value))
                        .map_err(|send_error| EvalError::EmitEffect(send_error.0))?;
                }
                ast::ModStmt::Expr(expr) => {
                    let _ = self.eval_expr(expr.clone())?;
                }
            }
        }

        Ok(Value::Record(Record::default()))
    }
}
