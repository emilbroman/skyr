use std::collections::HashSet;

use super::{Expr, TypeExpr};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeCastExpr {
    pub expr: Box<Loc<Expr>>,
    pub ty: Loc<TypeExpr>,
}

impl TypeCastExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        self.expr.as_ref().free_vars()
    }

    pub fn type_synth(
        &self,
        checker: &crate::checker::TypeChecker<'_>,
        env: &crate::checker::TypeEnv<'_>,
    ) -> Result<crate::Diagnosed<crate::Type>, crate::checker::TypeCheckError> {
        let mut diags = crate::DiagList::new();
        let target_ty = checker.resolve_type_expr(env, &self.ty).unpack(&mut diags);
        checker
            .check_expr(env, &self.expr, Some(&target_ty))?
            .unpack(&mut diags);
        Ok(crate::Diagnosed::new(target_ty, diags))
    }

    pub fn type_check(
        &self,
        checker: &crate::checker::TypeChecker<'_>,
        env: &crate::checker::TypeEnv<'_>,
        expr: &crate::Loc<Expr>,
        expected: &crate::Type,
    ) -> Result<crate::Diagnosed<crate::Type>, crate::checker::TypeCheckError> {
        let mut diags = crate::DiagList::new();
        let target_ty = checker.resolve_type_expr(env, &self.ty).unpack(&mut diags);
        checker
            .check_expr(env, &self.expr, Some(&target_ty))?
            .unpack(&mut diags);
        checker.subsumption_check(env, expr.span(), target_ty.clone(), expected, &mut diags)?;
        Ok(crate::Diagnosed::new(target_ty, diags))
    }

    pub fn eval(
        &self,
        evaluator: &crate::eval::Eval<'_>,
        env: &crate::eval::EvalEnv<'_>,
    ) -> Result<crate::TrackedValue, crate::eval::EvalError> {
        evaluator.eval_expr(env, &self.expr)
    }
}
