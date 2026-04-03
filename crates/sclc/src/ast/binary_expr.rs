use std::collections::HashSet;

use super::Expr;
use crate::Loc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
    NilCoalesce,
}

impl std::fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryOp::Add => write!(f, "+"),
            BinaryOp::Sub => write!(f, "-"),
            BinaryOp::Mul => write!(f, "*"),
            BinaryOp::Div => write!(f, "/"),
            BinaryOp::Eq => write!(f, "=="),
            BinaryOp::Neq => write!(f, "!="),
            BinaryOp::Lt => write!(f, "<"),
            BinaryOp::Lte => write!(f, "<="),
            BinaryOp::Gt => write!(f, ">"),
            BinaryOp::Gte => write!(f, ">="),
            BinaryOp::And => write!(f, "&&"),
            BinaryOp::Or => write!(f, "||"),
            BinaryOp::NilCoalesce => write!(f, "??"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryExpr {
    pub op: BinaryOp,
    pub lhs: Box<Loc<Expr>>,
    pub rhs: Box<Loc<Expr>>,
}

impl BinaryExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.lhs.as_ref().free_vars();
        vars.extend(self.rhs.as_ref().free_vars());
        vars
    }
}

// ─── Type checking ───────────────────────────────────────────────────────────

use crate::checker::{
    DisjointEquality, InvalidBinaryOperands, TypeCheckError, TypeChecker, TypeEnv,
};
use crate::{DiagList, Diagnosed, Type, TypeKind};

impl BinaryExpr {
    #[inline(never)]
    pub(crate) fn type_synth<S: crate::SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let lhs_ty = checker
            .synth_expr(env, self.lhs.as_ref())?
            .unpack(&mut diags)
            .unfold();

        // NilCoalesce is handled separately: check RHS against the inner type.
        if self.op == BinaryOp::NilCoalesce {
            if matches!(lhs_ty.kind, TypeKind::Never) {
                return Ok(Diagnosed::new(Type::Never, diags));
            }
            if let TypeKind::Optional(inner) = &lhs_ty.kind {
                checker
                    .check_expr(env, self.rhs.as_ref(), Some(inner))?
                    .unpack(&mut diags);
                return Ok(Diagnosed::new(inner.as_ref().clone(), diags));
            } else {
                diags.push(crate::checker::NilCoalesceOnNonOptional {
                    module_id: env.module_id()?,
                    ty: lhs_ty.clone(),
                    span: expr.span(),
                });
                return Ok(Diagnosed::new(Type::Never, diags));
            }
        }

        let rhs_ty = checker
            .synth_expr(env, self.rhs.as_ref())?
            .unpack(&mut diags)
            .unfold();

        let result_ty = if matches!(lhs_ty.kind, TypeKind::Never)
            || matches!(rhs_ty.kind, TypeKind::Never)
        {
            Type::Never
        } else {
            match self.op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                    match TypeChecker::<S>::arithmetic_result(self.op, &lhs_ty.kind, &rhs_ty.kind) {
                        Some(ty) => ty,
                        None => {
                            diags.push(InvalidBinaryOperands {
                                module_id: env.module_id()?,
                                op: self.op,
                                lhs: lhs_ty.clone(),
                                rhs: rhs_ty.clone(),
                                span: expr.span(),
                            });
                            Type::Never
                        }
                    }
                }
                BinaryOp::Eq | BinaryOp::Neq => {
                    if lhs_ty.is_disjoint_from(&rhs_ty) {
                        diags.push(DisjointEquality {
                            module_id: env.module_id()?,
                            lhs: lhs_ty.clone(),
                            rhs: rhs_ty.clone(),
                            span: expr.span(),
                        });
                    }
                    Type::Bool
                }
                BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
                    match TypeChecker::<S>::comparison_result(&lhs_ty.kind, &rhs_ty.kind) {
                        Some(ty) => ty,
                        None => {
                            diags.push(InvalidBinaryOperands {
                                module_id: env.module_id()?,
                                op: self.op,
                                lhs: lhs_ty.clone(),
                                rhs: rhs_ty.clone(),
                                span: expr.span(),
                            });
                            Type::Never
                        }
                    }
                }
                BinaryOp::And | BinaryOp::Or => {
                    match TypeChecker::<S>::logical_result(&lhs_ty.kind, &rhs_ty.kind) {
                        Some(ty) => ty,
                        None => {
                            diags.push(InvalidBinaryOperands {
                                module_id: env.module_id()?,
                                op: self.op,
                                lhs: lhs_ty.clone(),
                                rhs: rhs_ty.clone(),
                                span: expr.span(),
                            });
                            Type::Never
                        }
                    }
                }
                BinaryOp::NilCoalesce => unreachable!("handled above"),
            }
        };

        Ok(Diagnosed::new(result_ty, diags))
    }
}

// ─── Evaluation ──────────────────────────────────────────────────────────────

use crate::eval::{Eval, EvalEnv, EvalError, EvalErrorKind};
use crate::{TrackedValue, Value};

impl BinaryExpr {
    #[inline(never)]
    pub(crate) fn eval<S: crate::SourceRepo>(
        &self,
        evaluator: &Eval<'_, S>,
        env: &EvalEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let lhs = evaluator.eval_expr(env, self.lhs.as_ref())?;
        if matches!(lhs.value, Value::Pending(_)) {
            return Ok(crate::eval::pending_with(lhs.dependencies));
        }

        let binary_span = expr.span();
        let binary_module_id = env.module_id.cloned().unwrap_or_default();
        let op_name = format!("{:?}", self.op).to_lowercase();

        match self.op {
            BinaryOp::NilCoalesce => {
                if matches!(lhs.value, Value::Nil) {
                    evaluator.eval_expr(env, self.rhs.as_ref())
                } else {
                    Ok(lhs)
                }
            }
            BinaryOp::And => lhs.try_flat_map(|lhs| match lhs {
                Value::Bool(false) => Ok(TrackedValue::new(Value::Bool(false))),
                Value::Bool(true) => {
                    let rhs = evaluator.eval_expr(env, self.rhs.as_ref())?;
                    if matches!(&rhs.value, Value::Pending(_)) {
                        return Ok(rhs.map(|_| Value::Pending(crate::PendingValue)));
                    }
                    rhs.try_map(|rhs| match rhs {
                        Value::Bool(value) => Ok(Value::Bool(value)),
                        other => Err(env.throw(
                            EvalErrorKind::UnexpectedValue(other),
                            Some((binary_module_id.clone(), binary_span, op_name.clone())),
                        )),
                    })
                }
                other => Err(env.throw(
                    EvalErrorKind::UnexpectedValue(other),
                    Some((binary_module_id.clone(), binary_span, op_name.clone())),
                )),
            }),
            BinaryOp::Or => lhs.try_flat_map(|lhs| match lhs {
                Value::Bool(true) => Ok(TrackedValue::new(Value::Bool(true))),
                Value::Bool(false) => {
                    let rhs = evaluator.eval_expr(env, self.rhs.as_ref())?;
                    if matches!(&rhs.value, Value::Pending(_)) {
                        return Ok(rhs.map(|_| Value::Pending(crate::PendingValue)));
                    }
                    rhs.try_map(|rhs| match rhs {
                        Value::Bool(value) => Ok(Value::Bool(value)),
                        other => Err(env.throw(
                            EvalErrorKind::UnexpectedValue(other),
                            Some((binary_module_id.clone(), binary_span, op_name.clone())),
                        )),
                    })
                }
                other => Err(env.throw(
                    EvalErrorKind::UnexpectedValue(other),
                    Some((binary_module_id.clone(), binary_span, op_name.clone())),
                )),
            }),
            _ => {
                let rhs = evaluator.eval_expr(env, self.rhs.as_ref())?;
                if matches!(rhs.value, Value::Pending(_)) {
                    return Ok(crate::eval::pending_with(rhs.dependencies));
                }

                lhs.try_flat_map(|lhs| {
                    rhs.try_map(|rhs| {
                        evaluator
                            .eval_binary_values(self.op, lhs, rhs)
                            .map_err(|kind| {
                                env.throw(
                                    kind,
                                    Some((binary_module_id.clone(), binary_span, op_name.clone())),
                                )
                            })
                    })
                })
            }
        }
    }
}
