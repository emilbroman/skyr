use thiserror::Error;

use crate::{Record, Value, ast};

pub struct Eval;

#[derive(Error, Debug)]
pub enum EvalError {
    #[error("evaluation is not implemented yet for expression: {0:?}")]
    NotImplemented(ast::Expr),
}

impl Eval {
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
                ast::ModStmt::Expr(expr) => {
                    let _ = self.eval_expr(expr.clone())?;
                }
            }
        }

        Ok(Value::Record(Record::default()))
    }
}
