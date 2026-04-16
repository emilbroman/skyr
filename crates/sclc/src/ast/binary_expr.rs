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

use crate::Prop;
use crate::checker::{
    DisjointEquality, InvalidBinaryOperands, TypeCheckError, TypeChecker, TypeEnv,
};
use crate::{DiagList, Diagnosed, Type, TypeKind};

/// Check if a type represents nil: Optional(Never).
fn is_nil_type(ty: &Type) -> bool {
    matches!(&ty.kind, TypeKind::Optional(inner) if matches!(inner.kind, TypeKind::Never))
}

/// Emit propositions for nil comparison (== nil or != nil).
/// Returns propositions if one side is nil and the other is Optional.
fn nil_comparison_props(op: BinaryOp, result_id: usize, lhs_ty: &Type, rhs_ty: &Type) -> Vec<Prop> {
    // Determine which side is nil and which is the optional value.
    let optional_ty = if is_nil_type(rhs_ty) {
        Some(lhs_ty)
    } else if is_nil_type(lhs_ty) {
        Some(rhs_ty)
    } else {
        None
    };

    let Some(opt_ty) = optional_ty else {
        return Vec::new();
    };
    let TypeKind::Optional(inner) = &opt_ty.kind else {
        return Vec::new();
    };

    // The "non-nil" branch refines the optional to its inner type (A),
    // and the "nil-confirmed" branch refines it to `Never?` — the
    // singleton type of `nil` itself — matching the type that `nil`
    // expressions synthesize.
    let refines_non_nil = Prop::RefinesTo(opt_ty.id(), inner.as_ref().clone());
    let refines_nil = Prop::RefinesTo(opt_ty.id(), Type::Optional(Box::new(Type::Never())));
    let is_true = Prop::IsTrue(result_id);

    match op {
        // x != nil: IsTrue(result) ⇒ x : A;  ¬IsTrue(result) ⇒ x : Never?.
        BinaryOp::Neq => vec![
            is_true.clone().implies(refines_non_nil),
            is_true.negated().implies(refines_nil),
        ],
        // x == nil: ¬IsTrue(result) ⇒ x : A;  IsTrue(result) ⇒ x : Never?.
        BinaryOp::Eq => vec![
            is_true.clone().negated().implies(refines_non_nil),
            is_true.implies(refines_nil),
        ],
        _ => Vec::new(),
    }
}

impl BinaryExpr {
    #[inline(never)]
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let mut props = Vec::new();

        let (lhs_ty, lhs_props) = checker
            .synth_expr(env, self.lhs.as_ref())?
            .unpack_with_props(&mut diags);
        let lhs_ty = lhs_ty.unfold();
        props.extend(lhs_props);

        // NilCoalesce: propagate inner TypeId, no propositions emitted.
        if self.op == BinaryOp::NilCoalesce {
            if matches!(lhs_ty.kind, TypeKind::Never) {
                return Ok(crate::TypeSynth::with_props(
                    Diagnosed::new(Type::Never(), diags),
                    props,
                ));
            }
            if let TypeKind::Optional(inner) = &lhs_ty.kind {
                let (_, rhs_props) = checker
                    .check_expr(env, self.rhs.as_ref(), Some(inner))?
                    .unpack_with_props(&mut diags);
                props.extend(rhs_props);
                return Ok(crate::TypeSynth::with_props(
                    Diagnosed::new(inner.as_ref().clone(), diags),
                    props,
                ));
            } else {
                diags.push(crate::checker::NilCoalesceOnNonOptional {
                    module_id: env.module_id()?,
                    ty: lhs_ty.clone(),
                    span: expr.span(),
                });
                return Ok(crate::TypeSynth::with_props(
                    Diagnosed::new(Type::Never(), diags),
                    props,
                ));
            }
        }

        // && and || with short-circuit scoping.
        if self.op == BinaryOp::And || self.op == BinaryOp::Or {
            return self.synth_logical(checker, env, expr, lhs_ty, props, &mut diags);
        }

        let (rhs_ty, rhs_props) = checker
            .synth_expr(env, self.rhs.as_ref())?
            .unpack_with_props(&mut diags);
        let rhs_ty = rhs_ty.unfold();
        props.extend(rhs_props);

        let result_ty =
            if matches!(lhs_ty.kind, TypeKind::Never) || matches!(rhs_ty.kind, TypeKind::Never) {
                Type::Never()
            } else {
                match self.op {
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                        match TypeChecker::arithmetic_result(self.op, &lhs_ty.kind, &rhs_ty.kind) {
                            Some(ty) => ty,
                            None => {
                                diags.push(InvalidBinaryOperands {
                                    module_id: env.module_id()?,
                                    op: self.op,
                                    lhs: lhs_ty.clone(),
                                    rhs: rhs_ty.clone(),
                                    span: expr.span(),
                                });
                                Type::Never()
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
                        let result = Type::Bool();
                        // Emit nil comparison propositions.
                        props.extend(nil_comparison_props(self.op, result.id(), &lhs_ty, &rhs_ty));
                        result
                    }
                    BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
                        match TypeChecker::comparison_result(&lhs_ty.kind, &rhs_ty.kind) {
                            Some(ty) => ty,
                            None => {
                                diags.push(InvalidBinaryOperands {
                                    module_id: env.module_id()?,
                                    op: self.op,
                                    lhs: lhs_ty.clone(),
                                    rhs: rhs_ty.clone(),
                                    span: expr.span(),
                                });
                                Type::Never()
                            }
                        }
                    }
                    BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
                    BinaryOp::NilCoalesce => unreachable!("handled above"),
                }
            };

        Ok(crate::TypeSynth::with_props(
            Diagnosed::new(result_ty, diags),
            props,
        ))
    }

    /// Handle && and || with short-circuit proposition scoping.
    fn synth_logical(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
        lhs_ty: Type,
        mut props: Vec<Prop>,
        diags: &mut DiagList,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let lhs_id = lhs_ty.id();

        // Short-circuit scoping: RHS is checked in env where LHS truthiness is assumed.
        let rhs_assumption = match self.op {
            BinaryOp::And => Prop::IsTrue(lhs_id),
            BinaryOp::Or => Prop::IsTrue(lhs_id).negated(),
            _ => unreachable!(),
        };
        let rhs_env = env.with_propositions(std::slice::from_ref(&rhs_assumption));

        let (rhs_ty, rhs_props) = checker
            .synth_expr(&rhs_env, self.rhs.as_ref())?
            .unpack_with_props(diags);
        let rhs_ty = rhs_ty.unfold();

        // Wrap RHS props in implication (they were derived under the assumption).
        for prop in rhs_props {
            props.push(rhs_assumption.clone().implies(prop));
        }

        let result_ty = match TypeChecker::logical_result(&lhs_ty.kind, &rhs_ty.kind) {
            Some(ty) => ty,
            None => {
                diags.push(InvalidBinaryOperands {
                    module_id: env.module_id()?,
                    op: self.op,
                    lhs: lhs_ty.clone(),
                    rhs: rhs_ty.clone(),
                    span: expr.span(),
                });
                Type::Never()
            }
        };

        let result_id = result_ty.id();

        // Emit conjunction/disjunction implications.
        match self.op {
            BinaryOp::And => {
                props.push(Prop::IsTrue(result_id).implies(Prop::IsTrue(lhs_id)));
                props.push(Prop::IsTrue(result_id).implies(Prop::IsTrue(rhs_ty.id())));
            }
            BinaryOp::Or => {
                props.push(
                    Prop::IsTrue(result_id)
                        .negated()
                        .implies(Prop::IsTrue(lhs_id).negated()),
                );
                props.push(
                    Prop::IsTrue(result_id)
                        .negated()
                        .implies(Prop::IsTrue(rhs_ty.id()).negated()),
                );
            }
            _ => unreachable!(),
        }

        Ok(crate::TypeSynth::with_props(
            Diagnosed::new(result_ty, std::mem::take(diags)),
            props,
        ))
    }
}

// ─── Evaluation ──────────────────────────────────────────────────────────────

use crate::eval::{Eval, EvalEnv, EvalError, EvalErrorKind};
use crate::{TrackedValue, Value};

impl BinaryExpr {
    #[inline(never)]
    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
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
