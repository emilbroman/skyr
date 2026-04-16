use std::collections::HashSet;

use super::Expr;
use crate::Loc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
}

impl std::fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnaryOp::Negate => write!(f, "-"),
            UnaryOp::Not => write!(f, "!"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnaryExpr {
    pub op: UnaryOp,
    pub expr: Box<Loc<Expr>>,
}

impl UnaryExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        self.expr.as_ref().free_vars()
    }
}

// ─── Type checking ───────────────────────────────────────────────────────────

use crate::checker::{InvalidUnaryOperand, TypeCheckError, TypeChecker, TypeEnv};
use crate::{DiagList, Diagnosed, Type, TypeKind};

impl UnaryExpr {
    #[inline(never)]
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let operand_ty = checker
            .synth_expr(env, self.expr.as_ref())?
            .unpack(&mut diags)
            .unfold();

        let result_ty = if matches!(operand_ty.kind, TypeKind::Never) {
            Type::Never()
        } else {
            match self.op {
                UnaryOp::Negate => match &operand_ty.kind {
                    TypeKind::Int => Type::Int(),
                    TypeKind::Float => Type::Float(),
                    _ => {
                        diags.push(InvalidUnaryOperand {
                            module_id: env.module_id()?,
                            op: self.op,
                            operand: operand_ty.clone(),
                            span: expr.span(),
                        });
                        Type::Never()
                    }
                },
                UnaryOp::Not => match &operand_ty.kind {
                    TypeKind::Bool => Type::Bool(),
                    _ => {
                        diags.push(InvalidUnaryOperand {
                            module_id: env.module_id()?,
                            op: self.op,
                            operand: operand_ty.clone(),
                            span: expr.span(),
                        });
                        Type::Never()
                    }
                },
            }
        };

        Ok(crate::TypeSynth::new(Diagnosed::new(result_ty, diags)))
    }
}

// ─── Evaluation ──────────────────────────────────────────────────────────────

use crate::eval::{Eval, EvalEnv, EvalError, EvalErrorKind};
use crate::{TrackedValue, Value};

impl UnaryExpr {
    #[inline(never)]
    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let value = evaluator.eval_expr(env, self.expr.as_ref())?;
        if matches!(value.value, Value::Pending(_)) {
            return Ok(crate::eval::pending_with(value.dependencies));
        }
        let unary_span = expr.span();
        let unary_module_id = env.module_id.cloned().unwrap_or_default();
        match self.op {
            UnaryOp::Negate => value.try_map(|value| match value {
                Value::Int(value) => Ok(Value::Int(-value)),
                Value::Float(value_float) => Ok(Value::Float(
                    ordered_float::NotNan::new(-value_float.into_inner()).map_err(|_| {
                        env.throw(
                            EvalErrorKind::InvalidNumericResult("unary - produced NaN".into()),
                            Some((unary_module_id.clone(), unary_span, "-".to_string())),
                        )
                    })?,
                )),
                other => Err(env.throw(
                    EvalErrorKind::UnexpectedValue(other),
                    Some((unary_module_id.clone(), unary_span, "-".to_string())),
                )),
            }),
            UnaryOp::Not => value.try_map(|value| match value {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                other => Err(env.throw(
                    EvalErrorKind::UnexpectedValue(other),
                    Some((unary_module_id.clone(), unary_span, "!".to_string())),
                )),
            }),
        }
    }
}
