use super::TypeExpr;
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExceptionExpr {
    pub exception_id: u64,
    pub ty: Option<Loc<TypeExpr>>,
}

use crate::eval::{Eval, EvalEnv, EvalError};
use crate::{
    DiagList, Diagnosed, ExceptionValue, ExternFnValue, SourceRepo, TrackedValue, Type,
    TypeCheckError, TypeChecker, TypeEnv, Value,
};

impl ExceptionExpr {
    pub(crate) fn type_synth<S: SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let exception_ty = Type::Exception(self.exception_id);
        if let Some(ty_expr) = &self.ty {
            let param_ty = checker.resolve_type_expr(env, ty_expr).unpack(&mut diags);
            let fn_ty = Type::Fn(crate::FnType {
                type_params: vec![],
                params: vec![param_ty],
                ret: Box::new(exception_ty),
            });
            Ok(Diagnosed::new(fn_ty, diags))
        } else {
            Ok(Diagnosed::new(exception_ty, diags))
        }
    }

    pub(crate) fn eval(
        &self,
        _evaluator: &Eval,
        _env: &EvalEnv<'_>,
        _expr: &crate::Loc<super::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let exception_id = self.exception_id;
        if self.ty.is_some() {
            let exc_fn = Value::ExternFn(ExternFnValue::new(Box::new(
                move |args: Vec<TrackedValue>, _ctx: &crate::EvalCtx| {
                    let payload = args
                        .into_iter()
                        .next()
                        .map(|a| a.value)
                        .unwrap_or(Value::Nil);
                    Ok(TrackedValue::new(Value::Exception(ExceptionValue {
                        exception_id,
                        payload: Box::new(payload),
                    })))
                },
            )));
            Ok(Eval::tracked(exc_fn))
        } else {
            Ok(Eval::tracked(Value::Exception(ExceptionValue {
                exception_id,
                payload: Box::new(Value::Nil),
            })))
        }
    }
}
