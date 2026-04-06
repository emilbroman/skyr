use std::collections::HashSet;

use super::Expr;
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfExpr {
    pub condition: Box<Loc<Expr>>,
    pub then_expr: Box<Loc<Expr>>,
    pub else_expr: Option<Box<Loc<Expr>>>,
}

use crate::eval::{Eval, EvalEnv, EvalError, EvalErrorKind};
use crate::{DiagList, Diagnosed, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv, Value};

impl IfExpr {
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        _expr: &crate::Loc<super::Expr>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        checker
            .check_expr(env, self.condition.as_ref(), Some(&Type::Bool))?
            .unpack(&mut diags);

        let then_ty = checker
            .synth_expr(env, self.then_expr.as_ref())?
            .unpack(&mut diags)
            .unfold();

        if let Some(else_expr) = self.else_expr.as_ref() {
            checker
                .check_expr(env, else_expr.as_ref(), Some(&then_ty))?
                .unpack(&mut diags);
            return Ok(Diagnosed::new(then_ty, diags));
        }

        Ok(Diagnosed::new(Type::Optional(Box::new(then_ty)), diags))
    }

    pub(crate) fn type_check(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<super::Expr>,
        expected: &Type,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        checker
            .check_expr(env, self.condition.as_ref(), Some(&Type::Bool))?
            .unpack(&mut diags);

        let then_ty = checker
            .synth_expr(env, self.then_expr.as_ref())?
            .unpack(&mut diags)
            .unfold();

        if let Some(else_expr) = self.else_expr.as_ref() {
            checker
                .check_expr(env, else_expr.as_ref(), Some(&then_ty))?
                .unpack(&mut diags);
            checker.subsumption_check(env, expr.span(), then_ty.clone(), expected, &mut diags)?;
            return Ok(Diagnosed::new(then_ty, diags));
        }

        let result_ty = Type::Optional(Box::new(then_ty));
        checker.subsumption_check(env, expr.span(), result_ty.clone(), expected, &mut diags)?;
        Ok(Diagnosed::new(result_ty, diags))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<super::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let condition = evaluator.eval_expr(env, self.condition.as_ref())?;
        if matches!(&condition.value, Value::Pending(_)) {
            return Ok(condition.map(|_| Value::Pending(crate::PendingValue)));
        }

        match condition.value {
            Value::Bool(true) => evaluator.eval_expr(env, self.then_expr.as_ref()),
            Value::Bool(false) => {
                if let Some(else_expr) = &self.else_expr {
                    evaluator.eval_expr(env, else_expr.as_ref())
                } else {
                    Ok(crate::eval::tracked(Value::Nil))
                }
            }
            other => Err(env.throw(
                EvalErrorKind::UnexpectedValue(other),
                Some((
                    env.module_id.cloned().unwrap_or_default(),
                    expr.span(),
                    "if".to_string(),
                )),
            )),
        }
    }

    pub(crate) fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.condition.as_ref().free_vars();
        vars.extend(self.then_expr.as_ref().free_vars());
        if let Some(else_expr) = &self.else_expr {
            vars.extend(else_expr.as_ref().free_vars());
        }
        vars
    }
}
