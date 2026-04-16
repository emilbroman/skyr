use std::collections::HashSet;

use super::Expr;
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfExpr {
    pub condition: Box<Loc<Expr>>,
    pub then_expr: Box<Loc<Expr>>,
    pub else_expr: Option<Box<Loc<Expr>>>,
}

use crate::eval::{Eval, EvalEnv, EvalError, EvalErrorKind};
use crate::{
    DiagList, Diagnosed, Prop, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv, Value,
};

impl IfExpr {
    /// Wrap branch propositions in implications.
    fn wrap_branch_props(
        cond_id: usize,
        then_props: Vec<Prop>,
        else_props: Vec<Prop>,
        cond_props: Vec<Prop>,
    ) -> Vec<Prop> {
        let mut result_props = cond_props;
        let is_true = Prop::IsTrue(cond_id);
        let not_true = Prop::IsTrue(cond_id).negated();

        for prop in then_props {
            result_props.push(is_true.clone().implies(prop));
        }
        for prop in else_props {
            result_props.push(not_true.clone().implies(prop));
        }
        result_props
    }

    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        _expr: &crate::Loc<super::Expr>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();

        // Synthesize condition to get its TypeId and propositions.
        let (cond_ty, cond_props) = checker
            .synth_expr(env, self.condition.as_ref())?
            .unpack_with_props(&mut diags);
        checker.subsumption_check(
            env,
            self.condition.span(),
            cond_ty.clone(),
            &Type::Bool(),
            &mut diags,
        )?;
        let cond_id = cond_ty.id();

        // Create branch environments with assumptions.
        let mut then_assumptions = cond_props.clone();
        then_assumptions.push(Prop::IsTrue(cond_id));
        let then_env = env.with_propositions(&then_assumptions);

        let mut else_assumptions = cond_props.clone();
        else_assumptions.push(Prop::IsTrue(cond_id).negated());
        let else_env = env.with_propositions(&else_assumptions);

        let (then_ty, then_props) = checker
            .synth_expr(&then_env, self.then_expr.as_ref())?
            .unpack_with_props(&mut diags);
        let then_ty = then_ty.unfold();

        if let Some(else_expr) = self.else_expr.as_ref() {
            let (_, else_props) = checker
                .check_expr(&else_env, else_expr.as_ref(), Some(&then_ty))?
                .unpack_with_props(&mut diags);
            let props = Self::wrap_branch_props(cond_id, then_props, else_props, cond_props);
            return Ok(crate::TypeSynth::with_props(
                Diagnosed::new(then_ty, diags),
                props,
            ));
        }

        let result_ty = if matches!(then_ty.kind, crate::TypeKind::Optional(_)) {
            then_ty
        } else {
            Type::Optional(Box::new(then_ty))
        };
        Ok(crate::TypeSynth::with_props(
            Diagnosed::new(result_ty, diags),
            cond_props,
        ))
    }

    pub(crate) fn type_check(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<super::Expr>,
        expected: &Type,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        let mut diags = DiagList::new();

        // Synthesize condition to get its TypeId and propositions.
        let (cond_ty, cond_props) = checker
            .synth_expr(env, self.condition.as_ref())?
            .unpack_with_props(&mut diags);
        checker.subsumption_check(
            env,
            self.condition.span(),
            cond_ty.clone(),
            &Type::Bool(),
            &mut diags,
        )?;
        let cond_id = cond_ty.id();

        // Create branch environments with assumptions.
        let mut then_assumptions = cond_props.clone();
        then_assumptions.push(Prop::IsTrue(cond_id));
        let then_env = env.with_propositions(&then_assumptions);

        let mut else_assumptions = cond_props.clone();
        else_assumptions.push(Prop::IsTrue(cond_id).negated());
        let else_env = env.with_propositions(&else_assumptions);

        let (then_ty, then_props) = checker
            .synth_expr(&then_env, self.then_expr.as_ref())?
            .unpack_with_props(&mut diags);
        let then_ty = then_ty.unfold();

        if let Some(else_expr) = self.else_expr.as_ref() {
            let (_, else_props) = checker
                .check_expr(&else_env, else_expr.as_ref(), Some(&then_ty))?
                .unpack_with_props(&mut diags);
            checker.subsumption_check(env, expr.span(), then_ty.clone(), expected, &mut diags)?;
            let props = Self::wrap_branch_props(cond_id, then_props, else_props, cond_props);
            return Ok(crate::TypeSynth::with_props(
                Diagnosed::new(then_ty, diags),
                props,
            ));
        }

        let result_ty = if matches!(then_ty.kind, crate::TypeKind::Optional(_)) {
            then_ty
        } else {
            Type::Optional(Box::new(then_ty))
        };
        checker.subsumption_check(env, expr.span(), result_ty.clone(), expected, &mut diags)?;
        Ok(crate::TypeSynth::with_props(
            Diagnosed::new(result_ty, diags),
            cond_props,
        ))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<super::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        let condition = evaluator.eval_expr(env, self.condition.as_ref())?;
        if matches!(&condition.value, Value::Pending(_)) {
            return Ok(condition.map(|_| Value::Pending(crate::PendingValue)));
        }

        match condition.value {
            Value::Bool(true) => evaluator.eval_expr(env, self.then_expr.as_ref()),
            Value::Bool(false) => {
                if let Some(else_expr) = &self.else_expr {
                    evaluator.eval_expr(env, else_expr.as_ref())
                } else {
                    Ok(crate::eval::tracked(Value::Nil))
                }
            }
            other => Err(env.throw(
                EvalErrorKind::UnexpectedValue(other),
                Some((
                    env.module_id.cloned().unwrap_or_default(),
                    expr.span(),
                    "if".to_string(),
                )),
            )),
        }
    }

    pub(crate) fn free_vars(&self) -> HashSet<&str> {
        let mut vars = self.condition.as_ref().free_vars();
        vars.extend(self.then_expr.as_ref().free_vars());
        if let Some(else_expr) = &self.else_expr {
            vars.extend(else_expr.as_ref().free_vars());
        }
        vars
    }
}
