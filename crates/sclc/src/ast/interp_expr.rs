use std::collections::HashSet;

use super::Expr;
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterpExpr {
    pub parts: Vec<Loc<Expr>>,
}

impl InterpExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        let mut vars = HashSet::new();
        for part in &self.parts {
            vars.extend(part.as_ref().free_vars());
        }
        vars
    }

    pub fn type_synth<S: crate::SourceRepo>(
        &self,
        checker: &crate::checker::TypeChecker<'_, S>,
        env: &crate::checker::TypeEnv<'_>,
    ) -> Result<crate::Diagnosed<crate::Type>, crate::checker::TypeCheckError> {
        let mut diags = crate::DiagList::new();
        for part in &self.parts {
            checker.synth_expr(env, part)?.unpack(&mut diags);
        }
        Ok(crate::Diagnosed::new(crate::Type::Str, diags))
    }

    pub fn eval(
        &self,
        evaluator: &crate::eval::Eval,
        env: &crate::eval::EvalEnv<'_>,
    ) -> Result<crate::TrackedValue, crate::eval::EvalError> {
        let mut out = String::new();
        let mut dependencies = std::collections::BTreeSet::new();
        for part in &self.parts {
            let value = evaluator.eval_expr(env, part)?;
            dependencies.extend(value.dependencies.clone());
            if matches!(value.value, crate::Value::Pending(_)) {
                return Ok(crate::eval::Eval::pending_with(dependencies));
            }
            out.push_str(&value.value.to_string());
        }
        Ok(crate::eval::Eval::with_dependencies(
            crate::Value::Str(out),
            dependencies,
        ))
    }
}
