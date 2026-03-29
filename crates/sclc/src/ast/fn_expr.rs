use std::collections::{HashMap, HashSet};

use super::{Expr, TypeExpr, TypeParam, Var};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnExpr {
    pub type_params: Vec<TypeParam>,
    pub params: Vec<FnParam>,
    pub body: Box<Loc<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnParam {
    pub var: Loc<Var>,
    pub ty: Loc<TypeExpr>,
}

use crate::checker::next_type_id;
use crate::eval::{Eval, EvalEnv, EvalError, FnEnv};
use crate::{
    DiagList, Diagnosed, FnType, FnValue, SourceRepo, TrackedValue, Type, TypeCheckError,
    TypeChecker, TypeEnv, Value,
};

impl FnExpr {
    pub(crate) fn type_synth<S: SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
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
                Type::Any
            };
            fn_env = fn_env.with_type_var_bound(type_id, upper_bound.clone());
            type_param_entries.push((type_id, upper_bound));
        }

        let mut params = Vec::with_capacity(self.params.len());
        for param in &self.params {
            let param_ty = checker
                .resolve_type_expr(&fn_env, &param.ty)
                .unpack(&mut diags);
            fn_env = fn_env.with_local(param.var.name.as_str(), param.var.span(), param_ty.clone());
            params.push(param_ty);
        }

        let ret = checker
            .synth_expr(&fn_env, self.body.as_ref())?
            .unpack(&mut diags);
        Ok(Diagnosed::new(
            Type::Fn(FnType {
                type_params: type_param_entries,
                params,
                ret: Box::new(ret),
            }),
            diags,
        ))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &Eval,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<super::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let mut captures = HashMap::new();
        for name in expr.as_ref().free_vars() {
            captures.insert(name.to_owned(), evaluator.eval_var_name(env, name)?);
        }
        Ok(Eval::tracked(Value::Fn(FnValue {
            env: FnEnv {
                module_id: env.module_id()?,
                captures,
                parameters: self
                    .params
                    .iter()
                    .map(|param| param.var.name.clone())
                    .collect(),
                self_name: None,
            },
            body: *self.body.clone(),
        })))
    }

    pub(crate) fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.body.as_ref().free_vars();
        for param in &self.params {
            vars.remove(param.var.name.as_str());
        }
        vars
    }
}
