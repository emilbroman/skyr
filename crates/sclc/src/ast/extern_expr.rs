use super::TypeExpr;
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternExpr {
    pub name: String,
    pub ty: Loc<TypeExpr>,
}

use crate::eval::{Eval, EvalEnv, EvalError, EvalErrorKind};
use crate::{DiagList, Diagnosed, TrackedValue, TypeCheckError, TypeChecker, TypeEnv};

impl ExternExpr {
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let resolved_ty = checker.resolve_type_expr(env, &self.ty).unpack(&mut diags);
        Ok(crate::TypeSynth::new(Diagnosed::new(resolved_ty, diags)))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<super::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        evaluator
            .externs
            .get(self.name.as_str())
            .cloned()
            .map(crate::eval::tracked)
            .ok_or_else(|| {
                env.throw(
                    EvalErrorKind::MissingExtern(self.name.clone()),
                    Some((
                        env.module_id.cloned().unwrap_or_default(),
                        expr.span(),
                        "extern".to_string(),
                    )),
                )
            })
    }
}
