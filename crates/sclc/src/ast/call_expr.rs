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
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let raw_callee_ty = checker
            .synth_expr(env, self.callee.as_ref())?
            .unpack(&mut diags)
            .unfold();
        let callee_ty = env.resolve_var_bound(&raw_callee_ty).unfold();
        if matches!(callee_ty.kind, TypeKind::Never) {
            return Ok(crate::TypeSynth::new(Diagnosed::new(Type::Never(), diags)));
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
            return Ok(crate::TypeSynth::new(Diagnosed::new(Type::Never(), diags)));
        };

        let mut fn_ty = checker.instantiate_call_type_args(env, expr, self, fn_ty, &mut diags)?;

        // If type params remain, infer type arguments from argument types.
        if !fn_ty.type_params.is_empty() {
            // Phase 1: Synth all args to get initial type information.
            let mut synth_arg_types = Vec::new();
            for arg in &self.args {
                let arg_ty = checker.synth_expr(env, arg)?.unpack(&mut DiagList::new());
                synth_arg_types.push(arg_ty);
            }

            let partial =
                crate::infer_type_args(&fn_ty, &synth_arg_types, &env.maps.type_var_bounds);

            let partial = match partial {
                Ok(p) => p,
                Err(_) => {
                    // Inference completely failed — fall back to permissive
                    // types so we still get useful diagnostics.
                    let fallback = self.fallback_fn_ty(&fn_ty);
                    return self.check_args_and_return(checker, env, &fallback, diags);
                }
            };

            let has_unsolved = partial
                .iter()
                .any(|(_, ty)| matches!(ty.kind, TypeKind::Never));

            if !has_unsolved {
                // All type vars solved — substitute and fall through to
                // normal argument checking.
                fn_ty = FnType {
                    type_params: vec![],
                    params: fn_ty
                        .params
                        .iter()
                        .map(|p| p.substitute(&partial))
                        .collect(),
                    ret: Box::new(fn_ty.ret.substitute(&partial)),
                };
            } else {
                // Phase 2: Some vars unsolved. Substitute solved vars and use
                // upper bounds for unsolved, then CHECK args with bidirectional
                // typing to get better actual types (especially for lambdas
                // with untyped parameters).
                let check_replacements: Vec<(usize, Type)> = fn_ty
                    .type_params
                    .iter()
                    .map(|(id, bound)| {
                        let solution = &partial.iter().find(|(pid, _)| pid == id).unwrap().1;
                        if matches!(solution.kind, TypeKind::Never) {
                            (*id, bound.clone())
                        } else {
                            (*id, solution.clone())
                        }
                    })
                    .collect();

                let check_params: Vec<Type> = fn_ty
                    .params
                    .iter()
                    .map(|p| p.substitute(&check_replacements))
                    .collect();

                if self.args.len() < fn_ty.params.len() {
                    diags.push(MissingArguments {
                        module_id: env.module_id()?,
                        expected: fn_ty.params.len(),
                        got: self.args.len(),
                        span: self.callee.span(),
                    });
                }

                let mut actual_arg_types = Vec::new();
                for (index, arg) in self.args.iter().enumerate() {
                    let Some(param_ty) = check_params.get(index) else {
                        diags.push(ExtraneousArgument {
                            module_id: env.module_id()?,
                            index,
                            span: arg.span(),
                        });
                        continue;
                    };

                    let actual = checker
                        .check_expr(env, arg, Some(param_ty))?
                        .unpack(&mut diags);
                    actual_arg_types.push(actual);
                }

                // Phase 3: Re-infer using actual types from checking.
                let final_replacements =
                    crate::infer_type_args(&fn_ty, &actual_arg_types, &env.maps.type_var_bounds)
                        .unwrap_or(partial);

                let ret = fn_ty.ret.substitute(&final_replacements);
                return Ok(crate::TypeSynth::new(Diagnosed::new(ret, diags)));
            }
        }

        self.check_args_and_return(checker, env, &fn_ty, diags)
    }

    /// Check arguments against a fully-instantiated (no type params) fn type
    /// and return the return type.
    fn check_args_and_return(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        fn_ty: &FnType,
        mut diags: DiagList,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
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

        Ok(crate::TypeSynth::new(Diagnosed::new(
            *fn_ty.ret.clone(),
            diags,
        )))
    }

    /// Produce a permissive fallback FnType when inference fails entirely:
    /// params become Any (accept anything), return becomes Never.
    fn fallback_fn_ty(&self, fn_ty: &FnType) -> FnType {
        let param_replacements: Vec<(usize, Type)> = fn_ty
            .type_params
            .iter()
            .map(|(id, _)| (*id, Type::Any()))
            .collect();
        let ret_replacements: Vec<(usize, Type)> = fn_ty
            .type_params
            .iter()
            .map(|(id, _)| (*id, Type::Never()))
            .collect();
        FnType {
            type_params: vec![],
            params: fn_ty
                .params
                .iter()
                .map(|p| p.substitute(&param_replacements))
                .collect(),
            ret: Box::new(fn_ty.ret.substitute(&ret_replacements)),
        }
    }

    #[inline(never)]
    fn synth_call_free_var(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        callee_var_id: usize,
        diags: &mut DiagList,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
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
        Ok(crate::TypeSynth::new(Diagnosed::new(
            ret_var,
            DiagList::new(),
        )))
    }

    #[inline(never)]
    pub(crate) fn type_check(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
        expected: &Type,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let actual_ty = self.type_synth(checker, env, expr)?.unpack(&mut diags);
        checker.subsumption_check(env, expr.span(), actual_ty.clone(), expected, &mut diags)?;
        Ok(crate::TypeSynth::new(Diagnosed::new(actual_ty, diags)))
    }
}

// ─── Evaluation ──────────────────────────────────────────────────────────────

use crate::eval::{Eval, EvalEnv, EvalError, StackFrame};
use crate::{TrackedValue, Value};

impl CallExpr {
    #[inline(never)]
    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<TrackedValue, EvalError> {
        // Spec §8 rule E-Call: the callee is evaluated first, then the
        // arguments in left-to-right order. Evaluating the callee up front
        // means any exception or side effect attached to `f` in `f(x)` is
        // observed before `x` is evaluated.
        let callee = evaluator.eval_expr(env, self.callee.as_ref())?;
        let args = self
            .args
            .iter()
            .map(|arg| evaluator.eval_expr(env, arg))
            .collect::<Result<Vec<_>, _>>()?;
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
                let call_env =
                    function
                        .env
                        .as_eval_env(function, &args, Some(&frame), env.global_env);
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
            other => Err(env.throw(
                crate::eval::EvalErrorKind::UnexpectedValue(other),
                Some((
                    env.module_id.cloned().unwrap_or_default(),
                    self.callee.span(),
                    frame_name,
                )),
            )),
        }
    }
}
