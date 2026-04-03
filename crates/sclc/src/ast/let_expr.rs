use std::collections::HashSet;

use super::{Expr, LetBind};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetExpr {
    pub bind: LetBind,
    pub expr: Box<Loc<Expr>>,
}

use crate::eval::{Eval, EvalEnv, EvalError};
use crate::{
    DiagList, Diagnosed, SourceRepo, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv,
};

impl LetExpr {
    pub(crate) fn type_synth<S: SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let annotation_ty = self
            .bind
            .ty
            .as_ref()
            .map(|te| checker.resolve_type_expr(env, te).unpack(&mut diags));
        let bind_ty = checker
            .check_expr(env, self.bind.expr.as_ref(), annotation_ty.as_ref())?
            .unpack(&mut diags);
        let bind_ty = annotation_ty.unwrap_or(bind_ty);
        if let Some((cursor, _)) = &self.bind.var.cursor {
            cursor.set_type(bind_ty.clone());
            cursor.set_identifier(crate::CursorIdentifier::Let(self.bind.var.name.clone()));
        }
        let inner_env = env.with_local(self.bind.var.name.as_str(), self.bind.var.span(), bind_ty);
        let body_ty = checker
            .synth_expr(&inner_env, self.expr.as_ref())?
            .unpack(&mut diags);
        Ok(Diagnosed::new(body_ty, diags))
    }

    pub(crate) fn type_check<S: SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
        expected: &Type,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let annotation_ty = self
            .bind
            .ty
            .as_ref()
            .map(|te| checker.resolve_type_expr(env, te).unpack(&mut diags));
        let bind_ty = checker
            .check_expr(env, self.bind.expr.as_ref(), annotation_ty.as_ref())?
            .unpack(&mut diags);
        let bind_ty = annotation_ty.unwrap_or(bind_ty);
        if let Some((cursor, _)) = &self.bind.var.cursor {
            cursor.set_type(bind_ty.clone());
            cursor.set_identifier(crate::CursorIdentifier::Let(self.bind.var.name.clone()));
        }
        let inner_env = env.with_local(self.bind.var.name.as_str(), self.bind.var.span(), bind_ty);
        let body_ty = checker
            .check_expr(&inner_env, self.expr.as_ref(), Some(expected))?
            .unpack(&mut diags);
        Ok(Diagnosed::new(body_ty, diags))
    }

    pub(crate) fn eval<S: SourceRepo>(
        &self,
        evaluator: &Eval<'_, S>,
        env: &EvalEnv<'_>,
    ) -> Result<TrackedValue, EvalError> {
        let bind_value = evaluator.eval_expr(env, self.bind.expr.as_ref())?;
        let inner_env = env.with_local(self.bind.var.name.as_str(), bind_value);
        evaluator.eval_expr(&inner_env, self.expr.as_ref())
    }

    pub(crate) fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.bind.expr.as_ref().free_vars();
        let mut body_vars = self.expr.as_ref().free_vars();
        body_vars.remove(self.bind.var.name.as_str());
        vars.extend(body_vars);
        vars
    }
}
