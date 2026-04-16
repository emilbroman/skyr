use std::collections::{BTreeSet, HashSet};

use super::Expr;
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictExpr {
    pub entries: Vec<DictEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictEntry {
    pub key: Loc<Expr>,
    pub value: Loc<Expr>,
}

impl DictExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        let mut vars = HashSet::new();
        for entry in &self.entries {
            vars.extend(entry.key.as_ref().free_vars());
            vars.extend(entry.value.as_ref().free_vars());
        }
        vars
    }

    #[inline(never)]
    pub(crate) fn type_synth(
        &self,
        checker: &crate::checker::TypeChecker<'_>,
        env: &crate::checker::TypeEnv<'_>,
    ) -> Result<crate::TypeSynth, crate::checker::TypeCheckError> {
        let mut diags = crate::DiagList::new();
        let mut props = Vec::new();
        let dict_ty = if let Some((first, rest)) = self.entries.split_first() {
            let (key_ty, key_props) = checker
                .synth_expr(env, &first.key)?
                .unpack_with_props(&mut diags);
            let key_ty = key_ty.unfold();
            props.extend(key_props);
            let (value_ty, value_props) = checker
                .synth_expr(env, &first.value)?
                .unpack_with_props(&mut diags);
            let value_ty = value_ty.unfold();
            props.extend(value_props);
            for entry in rest {
                let (_, entry_props) = checker
                    .check_expr(env, &entry.key, Some(&key_ty))?
                    .unpack_with_props(&mut diags);
                props.extend(entry_props);
                let (_, entry_props) = checker
                    .check_expr(env, &entry.value, Some(&value_ty))?
                    .unpack_with_props(&mut diags);
                props.extend(entry_props);
            }
            crate::Type::Dict(crate::DictType {
                key: Box::new(key_ty),
                value: Box::new(value_ty),
            })
        } else {
            crate::Type::Dict(crate::DictType {
                key: Box::new(crate::Type::Never()),
                value: Box::new(crate::Type::Never()),
            })
        };
        Ok(crate::TypeSynth::with_props(
            crate::Diagnosed::new(dict_ty, diags),
            props,
        ))
    }

    #[inline(never)]
    pub(crate) fn type_check(
        &self,
        checker: &crate::checker::TypeChecker<'_>,
        env: &crate::checker::TypeEnv<'_>,
        expr: &Loc<Expr>,
        expected: &crate::Type,
    ) -> Result<crate::TypeSynth, crate::checker::TypeCheckError> {
        if let crate::TypeKind::Dict(expected_dict) = &expected.kind {
            return self.check_dict_against(checker, env, expected_dict);
        }
        checker.synth_then_subsume(env, expr, expected)
    }

    #[inline(never)]
    fn check_dict_against(
        &self,
        checker: &crate::checker::TypeChecker<'_>,
        env: &crate::checker::TypeEnv<'_>,
        expected_dict: &crate::DictType,
    ) -> Result<crate::TypeSynth, crate::checker::TypeCheckError> {
        let mut diags = crate::DiagList::new();
        let mut props = Vec::new();
        let expected_key = expected_dict.key.as_ref().clone().unfold();
        let expected_value = expected_dict.value.as_ref().clone().unfold();
        for entry in &self.entries {
            let (_, entry_props) = checker
                .check_expr(env, &entry.key, Some(&expected_key))?
                .unpack_with_props(&mut diags);
            props.extend(entry_props);
            let (_, entry_props) = checker
                .check_expr(env, &entry.value, Some(&expected_value))?
                .unpack_with_props(&mut diags);
            props.extend(entry_props);
        }
        Ok(crate::TypeSynth::with_props(
            crate::Diagnosed::new(
                crate::Type::Dict(crate::DictType {
                    key: Box::new(expected_key),
                    value: Box::new(expected_value),
                }),
                diags,
            ),
            props,
        ))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &crate::eval::Eval<'_>,
        env: &crate::eval::EvalEnv<'_>,
    ) -> Result<crate::TrackedValue, crate::eval::EvalError> {
        let mut dict = crate::Dict::default();
        let mut dependencies = BTreeSet::new();
        for entry in &self.entries {
            let key = evaluator.eval_expr(env, &entry.key)?;
            let value = evaluator.eval_expr(env, &entry.value)?;
            dependencies.extend(key.dependencies.clone());
            dependencies.extend(value.dependencies.clone());
            if matches!(key.value, crate::Value::Pending(_))
                || matches!(value.value, crate::Value::Pending(_))
            {
                return Ok(crate::eval::pending_with(dependencies));
            }
            dict.insert(key.value, value.value);
        }
        Ok(crate::eval::with_dependencies(
            crate::Value::Dict(dict),
            dependencies,
        ))
    }
}
