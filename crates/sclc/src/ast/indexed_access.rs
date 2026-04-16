use std::collections::HashSet;

use super::Expr;
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedAccessExpr {
    pub expr: Box<Loc<Expr>>,
    pub index: Box<Loc<Expr>>,
}

impl IndexedAccessExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.expr.as_ref().free_vars();
        vars.extend(self.index.as_ref().free_vars());
        vars
    }

    pub fn type_synth(
        &self,
        checker: &crate::checker::TypeChecker<'_>,
        env: &crate::checker::TypeEnv<'_>,
        expr: &crate::Loc<Expr>,
    ) -> Result<crate::TypeSynth, crate::checker::TypeCheckError> {
        use crate::{DiagList, Diagnosed, Type, TypeKind};

        let mut diags = DiagList::new();
        let container_ty = checker
            .synth_expr(env, self.expr.as_ref())?
            .unpack(&mut diags)
            .unfold();
        let container_ty = env.resolve_var_bound(&container_ty).unfold();
        if matches!(container_ty.kind, TypeKind::Never) {
            return Ok(crate::TypeSynth::new(Diagnosed::new(Type::Never(), diags)));
        }
        let result_ty = match &container_ty.kind {
            TypeKind::Dict(dict_ty) => {
                checker
                    .check_expr(env, self.index.as_ref(), Some(dict_ty.key.as_ref()))?
                    .unpack(&mut diags);
                Type::Optional(dict_ty.value.clone())
            }
            TypeKind::List(inner_ty) => {
                checker
                    .check_expr(env, self.index.as_ref(), Some(&Type::Int()))?
                    .unpack(&mut diags);
                Type::Optional(inner_ty.clone())
            }
            _ => {
                diags.push(crate::checker::InvalidIndexTarget {
                    module_id: env.module_id()?,
                    ty: container_ty,
                    span: expr.span(),
                });
                Type::Never()
            }
        };
        Ok(crate::TypeSynth::new(Diagnosed::new(result_ty, diags)))
    }

    pub fn eval(
        &self,
        evaluator: &crate::eval::Eval<'_>,
        env: &crate::eval::EvalEnv<'_>,
        expr: &crate::Loc<Expr>,
    ) -> Result<crate::TrackedValue, crate::eval::EvalError> {
        use crate::Value;
        use crate::eval::EvalErrorKind;

        let container = evaluator.eval_expr(env, self.expr.as_ref())?;
        match container.value {
            Value::Pending(_) => Ok(crate::eval::pending_with(container.dependencies)),
            Value::Dict(dict) => {
                let index = evaluator.eval_expr(env, self.index.as_ref())?;
                let mut deps = container.dependencies;
                deps.extend(index.dependencies);
                match index.value {
                    Value::Pending(_) => Ok(crate::eval::pending_with(deps)),
                    _ => {
                        let result = dict.get(&index.value).cloned().unwrap_or(Value::Nil);
                        Ok(crate::eval::with_dependencies(result, deps))
                    }
                }
            }
            Value::List(list) => {
                let index = evaluator.eval_expr(env, self.index.as_ref())?;
                let mut deps = container.dependencies;
                deps.extend(index.dependencies);
                match index.value {
                    Value::Pending(_) => Ok(crate::eval::pending_with(deps)),
                    Value::Int(i) => {
                        let result = if i >= 0 {
                            list.get(i as usize).cloned().unwrap_or(Value::Nil)
                        } else {
                            Value::Nil
                        };
                        Ok(crate::eval::with_dependencies(result, deps))
                    }
                    other => Err(env.throw(
                        EvalErrorKind::UnexpectedValue(other),
                        Some((
                            env.module_id.cloned().unwrap_or_default(),
                            self.index.span(),
                            "index".to_string(),
                        )),
                    )),
                }
            }
            other => Err(env.throw(
                EvalErrorKind::UnexpectedValue(other),
                Some((
                    env.module_id.cloned().unwrap_or_default(),
                    expr.span(),
                    "indexed access".to_string(),
                )),
            )),
        }
    }
}
