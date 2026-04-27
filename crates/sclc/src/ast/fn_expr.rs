use std::collections::{HashMap, HashSet};

use super::{Expr, TypeExpr, TypeParam, Var};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnExpr {
    pub type_params: Vec<TypeParam>,
    pub params: Vec<FnParam>,
    pub body: Option<Box<Loc<Expr>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnParam {
    pub var: Loc<Var>,
    pub ty: Option<Loc<TypeExpr>>,
}

use crate::checker::{MissingParameterType, next_type_id};
use crate::eval::{Eval, EvalEnv, EvalError, FnEnv};
use crate::{
    DiagList, Diagnosed, FnType, FnValue, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv,
    Value,
};

impl FnExpr {
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();
        let mut fn_env = env.inner();

        let mut type_param_entries = Vec::with_capacity(self.type_params.len());
        for type_param in &self.type_params {
            let type_id = next_type_id();
            fn_env = fn_env.with_type_var(type_param.var.name.clone(), Type::Var(type_id));
            let upper_bound = if let Some(bound_expr) = &type_param.bound {
                checker
                    .resolve_type_expr(&fn_env, bound_expr)
                    .unpack(&mut diags)
            } else {
                Type::Any()
            };
            fn_env = fn_env.with_type_var_bound(type_id, upper_bound.clone());
            type_param_entries.push((type_id, upper_bound));
        }

        let mut params = Vec::with_capacity(self.params.len());
        for param in &self.params {
            let (body_ty, param_ty) = if let Some(ty_expr) = &param.ty {
                let ty = checker
                    .resolve_type_expr(&fn_env, ty_expr)
                    .unpack(&mut diags);
                (ty.clone(), ty)
            } else {
                diags.push(MissingParameterType {
                    module_id: env.module_id()?,
                    span: param.var.span(),
                });
                // Never in body suppresses cascading errors from operations on
                // the unknown param; Any in the signature avoids false positives
                // at call sites.
                (Type::Never(), Type::Any())
            };
            if let Some((cursor, _)) = &param.var.cursor {
                cursor.set_type(body_ty.clone());
                cursor.set_identifier(crate::CursorIdentifier::Let(param.var.name.clone()));
            }
            fn_env = fn_env.with_local(param.var.name.as_str(), param.var.span(), body_ty);
            params.push(param_ty);
        }

        let ret = if let Some(body) = &self.body {
            checker
                .synth_expr(&fn_env, body.as_ref())?
                .unpack(&mut diags)
        } else {
            Type::Never()
        };
        Ok(crate::TypeSynth::new(Diagnosed::new(
            Type::Fn(FnType {
                type_params: type_param_entries,
                params,
                ret: Box::new(ret),
            }),
            diags,
        )))
    }

    pub(crate) fn type_check(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<super::Expr>,
        expected: &Type,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        // If the expected type isn't a function type, fall back to synth-then-subsume.
        let expected_unfolded = expected.clone().unfold();
        let expected_fn = match &expected_unfolded.kind {
            crate::TypeKind::Fn(fn_ty) => fn_ty.clone(),
            _ => return checker.synth_then_subsume(env, expr, expected),
        };

        let mut diags = DiagList::new();
        let mut fn_env = env.inner();

        let mut type_param_entries = Vec::with_capacity(self.type_params.len());
        for type_param in &self.type_params {
            let type_id = next_type_id();
            fn_env = fn_env.with_type_var(type_param.var.name.clone(), Type::Var(type_id));
            let upper_bound = if let Some(bound_expr) = &type_param.bound {
                checker
                    .resolve_type_expr(&fn_env, bound_expr)
                    .unpack(&mut diags)
            } else {
                Type::Any()
            };
            fn_env = fn_env.with_type_var_bound(type_id, upper_bound.clone());
            type_param_entries.push((type_id, upper_bound));
        }

        let mut params = Vec::with_capacity(self.params.len());
        for (i, param) in self.params.iter().enumerate() {
            let expected_param_ty = expected_fn.params.get(i);
            let param_ty = if let Some(ty_expr) = &param.ty {
                let ty = checker
                    .resolve_type_expr(&fn_env, ty_expr)
                    .unpack(&mut diags);
                // Function parameters are contravariant: the expected param type
                // must be assignable to the provided one.
                if let Some(expected_param_ty) = expected_param_ty {
                    checker.subsumption_check(
                        &fn_env,
                        ty_expr.span(),
                        expected_param_ty.clone(),
                        &ty,
                        &mut diags,
                    )?;
                }
                ty
            } else if let Some(expected_param_ty) = expected_param_ty {
                expected_param_ty.clone()
            } else {
                diags.push(MissingParameterType {
                    module_id: env.module_id()?,
                    span: param.var.span(),
                });
                Type::Any()
            };
            if let Some((cursor, _)) = &param.var.cursor {
                cursor.set_type(param_ty.clone());
                cursor.set_identifier(crate::CursorIdentifier::Let(param.var.name.clone()));
            }
            fn_env = fn_env.with_local(param.var.name.as_str(), param.var.span(), param_ty.clone());
            params.push(param_ty);
        }

        let ret = if let Some(body) = &self.body {
            checker
                .check_expr(&fn_env, body.as_ref(), Some(expected_fn.ret.as_ref()))?
                .unpack(&mut diags)
        } else {
            Type::Never()
        };

        let actual = Type::Fn(FnType {
            type_params: type_param_entries,
            params,
            ret: Box::new(ret),
        });

        let ty = checker.subsumption_check(env, expr.span(), actual, expected, &mut diags)?;
        Ok(crate::TypeSynth::new(Diagnosed::new(ty, diags)))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<super::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let mut captures = HashMap::new();
        for name in expr.as_ref().free_vars() {
            captures.insert(name.to_owned(), evaluator.eval_var_name(env, name)?);
        }
        let body = self
            .body
            .as_ref()
            .map(|b| *b.clone())
            .unwrap_or_else(|| Loc::new(Expr::Nil, crate::Span::default()));
        Ok(crate::eval::tracked(Value::Fn(FnValue {
            env: FnEnv {
                module_id: env.module_id()?,
                raw_module_id: env.raw_module_id().cloned(),
                captures,
                parameters: self
                    .params
                    .iter()
                    .map(|param| param.var.name.clone())
                    .collect(),
                self_name: None,
                recursive_group: None,
            },
            body,
        })))
    }

    pub(crate) fn free_vars(&self) -> HashSet<&str> {
        let mut vars = if let Some(body) = &self.body {
            body.as_ref().free_vars()
        } else {
            HashSet::new()
        };
        for param in &self.params {
            vars.remove(param.var.name.as_str());
        }
        vars
    }
}
