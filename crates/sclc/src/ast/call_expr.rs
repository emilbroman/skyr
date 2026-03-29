use std::collections::HashSet;

use super::{Expr, TypeExpr};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallExpr {
    pub callee: Box<Loc<Expr>>,
    pub type_args: Vec<Loc<TypeExpr>>,
    pub args: Vec<Loc<Expr>>,
}

impl CallExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.callee.as_ref().free_vars();
        for arg in &self.args {
            vars.extend(arg.as_ref().free_vars());
        }
        vars
    }
}

// ─── Type checking ───────────────────────────────────────────────────────────

use crate::checker::{
    ExtraneousArgument, MissingArguments, NotAFunction, TypeCheckError, TypeChecker, TypeEnv,
    next_type_id,
};
use crate::{DiagList, Diagnosed, FnType, Type, TypeKind};

impl CallExpr {
    #[inline(never)]
    pub(crate) fn type_synth<S: crate::SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let raw_callee_ty = checker
            .synth_expr(env, self.callee.as_ref())?
            .unpack(&mut diags)
            .unfold();
        let callee_ty = env.resolve_var_bound(&raw_callee_ty).unfold();
        if matches!(callee_ty.kind, TypeKind::Never) {
            return Ok(Diagnosed::new(Type::Never, diags));
        }

        // Free variable callee constraint handling
        if let TypeKind::Var(callee_var_id) = &raw_callee_ty.kind
            && env.is_free_var(*callee_var_id)
        {
            return self.synth_call_free_var(checker, env, *callee_var_id, &mut diags);
        }

        let TypeKind::Fn(fn_ty) = callee_ty.kind else {
            diags.push(NotAFunction {
                module_id: env.module_id()?,
                ty: callee_ty,
                span: self.callee.span(),
            });
            return Ok(Diagnosed::new(Type::Never, diags));
        };

        let fn_ty = checker.instantiate_call_type_args(env, expr, self, fn_ty, &mut diags)?;

        if self.args.len() < fn_ty.params.len() {
            diags.push(MissingArguments {
                module_id: env.module_id()?,
                expected: fn_ty.params.len(),
                got: self.args.len(),
                span: self.callee.span(),
            });
        }

        for (index, arg) in self.args.iter().enumerate() {
            let Some(param_ty) = fn_ty.params.get(index) else {
                diags.push(ExtraneousArgument {
                    module_id: env.module_id()?,
                    index,
                    span: arg.span(),
                });
                continue;
            };

            checker
                .check_expr(env, arg, Some(param_ty))?
                .unpack(&mut diags);
        }

        Ok(Diagnosed::new(*fn_ty.ret, diags))
    }

    #[inline(never)]
    fn synth_call_free_var<S: crate::SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
        callee_var_id: usize,
        diags: &mut DiagList,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut arg_types = Vec::new();
        for arg in &self.args {
            let arg_ty = checker.synth_expr(env, arg)?.unpack(diags);
            arg_types.push(arg_ty);
        }
        let ret_id = next_type_id();
        let ret_var = Type::Var(ret_id);
        if let Some(fv) = &env.free_vars {
            fv.borrow_mut().register(ret_id);
            let fn_constraint = Type::Fn(FnType {
                type_params: vec![],
                params: arg_types,
                ret: Box::new(ret_var.clone()),
            });
            fv.borrow_mut().constrain(callee_var_id, fn_constraint);
        }
        Ok(Diagnosed::new(ret_var, DiagList::new()))
    }

    #[inline(never)]
    pub(crate) fn type_check<S: crate::SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
        expected: &Type,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let actual_ty = self.type_synth(checker, env, expr)?.unpack(&mut diags);
        checker.subsumption_check(env, expr.span(), actual_ty.clone(), expected, &mut diags)?;
        Ok(Diagnosed::new(actual_ty, diags))
    }
}

// ─── Evaluation ──────────────────────────────────────────────────────────────

use crate::eval::{Eval, EvalEnv, EvalError, StackFrame};
use crate::{TrackedValue, Value};

impl CallExpr {
    #[inline(never)]
    pub(crate) fn eval(
        &self,
        evaluator: &Eval,
        env: &EvalEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let args = self
            .args
            .iter()
            .map(|arg| evaluator.eval_expr(env, arg))
            .collect::<Result<Vec<_>, _>>()?;
        let callee = evaluator.eval_expr(env, self.callee.as_ref())?;
        let callee_dependencies = callee.dependencies.clone();
        if matches!(&callee.value, Value::Pending(_)) {
            return Ok(TrackedValue::pending().with_dependencies(callee_dependencies));
        }

        let frame_name = match &**self.callee.as_ref() {
            Expr::Var(var) => var.name.clone(),
            _ => "[fn]".to_string(),
        };

        match callee.value {
            Value::Fn(ref function) => {
                let call_module_id = env.module_id.cloned().unwrap_or_default();
                let frame = StackFrame {
                    module_id: call_module_id,
                    span: expr.span(),
                    name: frame_name,
                    parent: env.stack,
                };
                let call_env = function.env.as_eval_env(function, &args, Some(&frame));
                evaluator
                    .eval_expr(&call_env, &function.body)
                    .map(|value| value.with_dependencies(callee_dependencies))
            }
            Value::ExternFn(function) => {
                // Capture the full source trace for resource tracking.
                let call_module_id = env.module_id.cloned().unwrap_or_default();
                let mut trace = vec![ids::SourceFrame {
                    module_id: call_module_id.to_string(),
                    span: expr.span().to_string(),
                    name: frame_name.clone(),
                }];
                if let Some(stack) = env.stack {
                    for (mid, sp, nm) in stack.collect_trace() {
                        trace.push(ids::SourceFrame {
                            module_id: mid.to_string(),
                            span: sp.to_string(),
                            name: nm,
                        });
                    }
                }
                *evaluator.ctx.source_trace.lock().unwrap() = trace;

                let result = function
                    .call(args, &evaluator.ctx)
                    .map(|value| value.with_dependencies(callee_dependencies));
                evaluator.ctx.source_trace.lock().unwrap().clear();
                result
            }
            _ => Ok(Eval::tracked(Value::Nil).with_dependencies(callee_dependencies)),
        }
    }
}
