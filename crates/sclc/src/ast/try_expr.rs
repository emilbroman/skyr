use std::collections::HashSet;

use super::{Expr, Var};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TryExpr {
    pub expr: Box<Loc<Expr>>,
    pub catches: Vec<CatchClause>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatchClause {
    pub exception_var: Loc<Var>,
    pub catch_arg: Option<Loc<Var>>,
    pub body: Loc<Expr>,
}

use crate::checker::{InvalidCatchTarget, UnexpectedCatchArg};
use crate::eval::{Eval, EvalEnv, EvalError, EvalErrorKind, StackFrame};
use crate::{
    DiagList, Diagnosed, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv, TypeKind, Value,
};

impl TryExpr {
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let try_ty = checker
            .synth_expr(env, self.expr.as_ref())?
            .unpack(&mut diags)
            .unfold();

        self.check_catch_clauses(checker, env, &try_ty, &mut diags)?;

        Ok(crate::TypeSynth::new(Diagnosed::new(try_ty, diags)))
    }

    pub(crate) fn type_check(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<super::Expr>,
        expected: &Type,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let try_ty = checker
            .check_expr(env, self.expr.as_ref(), Some(expected))?
            .unpack(&mut diags)
            .unfold();

        self.check_catch_clauses(checker, env, &try_ty, &mut diags)?;

        checker.subsumption_check(env, expr.span(), try_ty.clone(), expected, &mut diags)?;
        Ok(crate::TypeSynth::new(Diagnosed::new(try_ty, diags)))
    }

    fn check_catch_clauses(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        try_ty: &Type,
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        for catch in &self.catches {
            let catch_var_ty = checker
                .synth_expr(
                    env,
                    &crate::Loc::new(
                        super::Expr::Var(catch.exception_var.clone()),
                        catch.exception_var.span(),
                    ),
                )?
                .unpack(diags)
                .unfold();

            match &catch_var_ty.kind {
                TypeKind::Exception(_) => {
                    if let Some(catch_arg) = &catch.catch_arg {
                        diags.push(UnexpectedCatchArg {
                            module_id: env.module_id()?,
                            span: catch_arg.span(),
                        });
                    }
                    checker
                        .check_expr(env, &catch.body, Some(try_ty))?
                        .unpack(diags);
                }
                TypeKind::Fn(fn_ty) => {
                    let ret_ty = fn_ty.ret.as_ref().clone().unfold();
                    if !matches!(ret_ty.kind, TypeKind::Exception(_)) {
                        diags.push(InvalidCatchTarget {
                            module_id: env.module_id()?,
                            ty: catch_var_ty.clone(),
                            span: catch.exception_var.span(),
                        });
                    }
                    if let Some(catch_arg) = &catch.catch_arg {
                        let param_ty = fn_ty.params.first().cloned().unwrap_or(Type::Never());
                        let inner_env =
                            env.with_local(catch_arg.name.as_str(), catch_arg.span(), param_ty);
                        checker
                            .check_expr(&inner_env, &catch.body, Some(try_ty))?
                            .unpack(diags);
                    } else {
                        checker
                            .check_expr(env, &catch.body, Some(try_ty))?
                            .unpack(diags);
                    }
                }
                TypeKind::Never => {
                    checker
                        .check_expr(env, &catch.body, Some(try_ty))?
                        .unpack(diags);
                }
                _ => {
                    diags.push(InvalidCatchTarget {
                        module_id: env.module_id()?,
                        ty: catch_var_ty,
                        span: catch.exception_var.span(),
                    });
                    checker
                        .check_expr(env, &catch.body, Some(try_ty))?
                        .unpack(diags);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
        _expr: &crate::Loc<super::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let try_module_id = env.module_id.cloned().unwrap_or_default();
        match evaluator.eval_expr(env, self.expr.as_ref()) {
            Ok(value) => Ok(value),
            Err(EvalError {
                kind: EvalErrorKind::Exception(raised),
                stack_trace,
            }) => {
                for catch in &self.catches {
                    let catch_span = catch.exception_var.span();
                    let catch_target = evaluator.eval_expr(
                        env,
                        &crate::Loc::new(super::Expr::Var(catch.exception_var.clone()), catch_span),
                    )?;

                    match catch_target.value {
                        Value::Exception(exc) => {
                            if exc.exception_id == raised.exception_id {
                                return evaluator.eval_expr(env, &catch.body);
                            }
                        }
                        Value::ExternFn(func) => {
                            let arg_value = TrackedValue::new(raised.payload.clone());
                            let call_result = func.call(vec![arg_value], &evaluator.ctx)?;
                            match call_result.value {
                                Value::Exception(exc) => {
                                    if exc.exception_id == raised.exception_id {
                                        if let Some(catch_arg) = &catch.catch_arg {
                                            let inner_env = env.with_local(
                                                catch_arg.name.as_str(),
                                                TrackedValue::new(raised.payload.clone()),
                                            );
                                            return evaluator.eval_expr(&inner_env, &catch.body);
                                        } else {
                                            return evaluator.eval_expr(env, &catch.body);
                                        }
                                    }
                                }
                                _ => {
                                    return Err(env.throw(
                                        EvalErrorKind::UnexpectedValue(call_result.value),
                                        Some((
                                            try_module_id.clone(),
                                            catch_span,
                                            "catch".to_string(),
                                        )),
                                    ));
                                }
                            }
                        }
                        Value::Fn(function) => {
                            let arg_value = TrackedValue::new(raised.payload.clone());
                            let frame = StackFrame {
                                module_id: try_module_id.clone(),
                                span: catch_span,
                                name: "[fn]".to_string(),
                                parent: env.stack,
                            };
                            let call_env = function.env.as_eval_env(
                                &function,
                                &[arg_value],
                                Some(&frame),
                                env.global_env,
                            );
                            let call_result = evaluator.eval_expr(&call_env, &function.body)?;
                            match call_result.value {
                                Value::Exception(exc) => {
                                    if exc.exception_id == raised.exception_id {
                                        if let Some(catch_arg) = &catch.catch_arg {
                                            let inner_env = env.with_local(
                                                catch_arg.name.as_str(),
                                                TrackedValue::new(raised.payload.clone()),
                                            );
                                            return evaluator.eval_expr(&inner_env, &catch.body);
                                        } else {
                                            return evaluator.eval_expr(env, &catch.body);
                                        }
                                    }
                                }
                                _ => {
                                    return Err(env.throw(
                                        EvalErrorKind::UnexpectedValue(call_result.value),
                                        Some((
                                            try_module_id.clone(),
                                            catch_span,
                                            "catch".to_string(),
                                        )),
                                    ));
                                }
                            }
                        }
                        _ => {
                            return Err(env.throw(
                                EvalErrorKind::UnexpectedValue(catch_target.value),
                                Some((try_module_id.clone(), catch_span, "catch".to_string())),
                            ));
                        }
                    }
                }
                // No catch matched, re-raise
                Err(EvalError {
                    kind: EvalErrorKind::Exception(raised),
                    stack_trace,
                })
            }
            Err(other) => Err(other),
        }
    }

    pub(crate) fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.expr.as_ref().free_vars();
        for catch in &self.catches {
            vars.insert(catch.exception_var.name.as_str());
            let mut catch_vars = catch.body.as_ref().free_vars();
            if let Some(catch_arg) = &catch.catch_arg {
                catch_vars.remove(catch_arg.name.as_str());
            }
            vars.extend(catch_vars);
        }
        vars
    }
}
