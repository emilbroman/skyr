use std::collections::HashSet;

use super::{Expr, LetBind};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetExpr {
    pub bind: LetBind,
    pub expr: Option<Box<Loc<Expr>>>,
}

use crate::eval::{Eval, EvalEnv, EvalError};
use crate::{DiagList, Diagnosed, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv};

impl LetExpr {
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let annotation_ty = self
            .bind
            .ty
            .as_ref()
            .map(|te| checker.resolve_type_expr(env, te).unpack(&mut diags));
        let (bind_ty, bind_props) = checker
            .check_expr(env, self.bind.expr.as_ref(), annotation_ty.as_ref())?
            .unpack_with_props(&mut diags);
        let bind_ty = annotation_ty.unwrap_or(bind_ty);
        if let Some((cursor, _)) = &self.bind.var.cursor {
            cursor.set_type(bind_ty.clone());
            cursor.set_identifier(crate::CursorIdentifier::Let(self.bind.var.name.clone()));
        }
        let inner_env = env.with_local(self.bind.var.name.as_str(), self.bind.var.span(), bind_ty);
        let inner_env = inner_env.with_propositions(&bind_props);
        if let Some(body) = &self.expr {
            checker.synth_expr(&inner_env, body.as_ref())
        } else {
            Ok(crate::TypeSynth::new(Diagnosed::new(Type::Never(), diags)))
        }
    }

    pub(crate) fn type_check(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expected: &Type,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let annotation_ty = self
            .bind
            .ty
            .as_ref()
            .map(|te| checker.resolve_type_expr(env, te).unpack(&mut diags));
        let (bind_ty, bind_props) = checker
            .check_expr(env, self.bind.expr.as_ref(), annotation_ty.as_ref())?
            .unpack_with_props(&mut diags);
        let bind_ty = annotation_ty.unwrap_or(bind_ty);
        if let Some((cursor, _)) = &self.bind.var.cursor {
            cursor.set_type(bind_ty.clone());
            cursor.set_identifier(crate::CursorIdentifier::Let(self.bind.var.name.clone()));
        }
        let inner_env = env.with_local(self.bind.var.name.as_str(), self.bind.var.span(), bind_ty);
        let inner_env = inner_env.with_propositions(&bind_props);
        if let Some(body) = &self.expr {
            checker.check_expr(&inner_env, body.as_ref(), Some(expected))
        } else {
            Ok(crate::TypeSynth::new(Diagnosed::new(Type::Never(), diags)))
        }
    }

    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
    ) -> Result<TrackedValue, EvalError> {
        let bind_value = evaluator.eval_expr(env, self.bind.expr.as_ref())?;
        let inner_env = env.with_local(self.bind.var.name.as_str(), bind_value);
        if let Some(body) = &self.expr {
            evaluator.eval_expr(&inner_env, body.as_ref())
        } else {
            Ok(crate::eval::tracked(crate::Value::Nil))
        }
    }

    pub(crate) fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.bind.expr.as_ref().free_vars();
        if let Some(body) = &self.expr {
            let mut body_vars = body.as_ref().free_vars();
            body_vars.remove(self.bind.var.name.as_str());
            vars.extend(body_vars);
        }
        vars
    }
}
