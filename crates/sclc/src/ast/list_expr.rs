use std::collections::{BTreeSet, HashSet};

use super::{Expr, Var};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListExpr {
    pub items: Vec<ListItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListItem {
    Expr(Loc<Expr>),
    If(ListIfItem),
    For(ListForItem),
}

impl ListItem {
    pub fn free_vars(&self) -> HashSet<&str> {
        match self {
            ListItem::Expr(expr) => expr.as_ref().free_vars(),
            ListItem::If(item) => {
                let mut vars = item.condition.as_ref().free_vars();
                vars.extend(item.then_item.as_ref().free_vars());
                vars
            }
            ListItem::For(item) => {
                let mut vars = item.iterable.as_ref().free_vars();
                let mut body_vars = item.emit_item.as_ref().free_vars();
                body_vars.remove(item.var.name.as_str());
                vars.extend(body_vars);
                vars
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListIfItem {
    pub condition: Box<Loc<Expr>>,
    pub then_item: Box<ListItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListForItem {
    pub var: Loc<Var>,
    pub iterable: Box<Loc<Expr>>,
    pub emit_item: Box<ListItem>,
}

impl ListExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        let mut vars = HashSet::new();
        for item in &self.items {
            vars.extend(item.free_vars());
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
        let list_ty = if let Some((first, rest)) = self.items.split_first() {
            let (first_ty, first_props) = checker
                .check_list_item(env, first, None)?
                .unpack_with_props(&mut diags);
            let first_ty = first_ty.unfold();
            props.extend(first_props);
            for item in rest {
                let (_, item_props) = checker
                    .check_list_item(env, item, Some(&first_ty))?
                    .unpack_with_props(&mut diags);
                props.extend(item_props);
            }
            crate::Type::List(Box::new(first_ty))
        } else {
            crate::Type::List(Box::new(crate::Type::Never()))
        };
        Ok(crate::TypeSynth::with_props(
            crate::Diagnosed::new(list_ty, diags),
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
        if let crate::TypeKind::List(expected_item_ty) = &expected.kind {
            return self.check_list_against(checker, env, expected_item_ty);
        }
        checker.synth_then_subsume(env, expr, expected)
    }

    #[inline(never)]
    fn check_list_against(
        &self,
        checker: &crate::checker::TypeChecker<'_>,
        env: &crate::checker::TypeEnv<'_>,
        expected_item_ty: &crate::Type,
    ) -> Result<crate::TypeSynth, crate::checker::TypeCheckError> {
        let mut diags = crate::DiagList::new();
        let mut props = Vec::new();
        let expected_item_ty = expected_item_ty.clone().unfold();
        for item in &self.items {
            let (_, item_props) = checker
                .check_list_item(env, item, Some(&expected_item_ty))?
                .unpack_with_props(&mut diags);
            props.extend(item_props);
        }
        Ok(crate::TypeSynth::with_props(
            crate::Diagnosed::new(crate::Type::List(Box::new(expected_item_ty)), diags),
            props,
        ))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &crate::eval::Eval<'_>,
        env: &crate::eval::EvalEnv<'_>,
    ) -> Result<crate::TrackedValue, crate::eval::EvalError> {
        let mut values = Vec::new();
        let mut dependencies = BTreeSet::new();
        for item in &self.items {
            match evaluator.eval_list_item(env, item, &mut values)? {
                crate::eval::ListItemOutcome::Complete => {}
                crate::eval::ListItemOutcome::Pending(pending_dependencies) => {
                    dependencies.extend(pending_dependencies);
                    return Ok(crate::eval::pending_with(dependencies));
                }
            }
        }

        for value in &values {
            dependencies.extend(value.dependencies.clone());
        }

        Ok(crate::eval::with_dependencies(
            crate::Value::List(values.into_iter().map(|value| value.value).collect()),
            dependencies,
        ))
    }
}
