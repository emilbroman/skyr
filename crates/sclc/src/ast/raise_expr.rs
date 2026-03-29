use std::collections::HashSet;

use super::Expr;
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaiseExpr {
    pub expr: Box<Loc<Expr>>,
}

use crate::checker::NotAnException;
use crate::eval::{Eval, EvalEnv, EvalError, EvalErrorKind, RaisedException};
use crate::{
    DiagList, Diagnosed, SourceRepo, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv,
    TypeKind, Value,
};

impl RaiseExpr {
    pub(crate) fn type_synth<S: SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let inner_ty = checker
            .synth_expr(env, self.expr.as_ref())?
            .unpack(&mut diags)
            .unfold();
        if !matches!(inner_ty.kind, TypeKind::Exception(_) | TypeKind::Never) {
            diags.push(NotAnException {
                module_id: env.module_id()?,
                ty: inner_ty,
                span: self.expr.span(),
            });
        }
        Ok(Diagnosed::new(Type::Never, diags))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &Eval,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<super::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let value = evaluator.eval_expr(env, self.expr.as_ref())?;
        if matches!(value.value, Value::Pending(_)) {
            return Ok(Eval::pending_with(value.dependencies));
        }
        let raise_frame = Some((
            env.module_id.cloned().unwrap_or_default(),
            expr.span(),
            "raise".to_string(),
        ));
        let exception_name = match self.expr.as_ref().as_ref() {
            super::Expr::Var(var) => var.name.clone(),
            super::Expr::Call(call) => match call.callee.as_ref().as_ref() {
                super::Expr::Var(var) => var.name.clone(),
                _ => "exception".to_string(),
            },
            _ => "exception".to_string(),
        };
        match value.value {
            Value::Exception(exc) => Err(env.throw(
                EvalErrorKind::Exception(RaisedException {
                    exception_id: exc.exception_id,
                    payload: *exc.payload,
                    name: exception_name,
                }),
                raise_frame,
            )),
            other => Err(env.throw(EvalErrorKind::UnexpectedValue(other), raise_frame)),
        }
    }

    pub(crate) fn free_vars(&self) -> HashSet<&str> {
        self.expr.as_ref().free_vars()
    }
}
