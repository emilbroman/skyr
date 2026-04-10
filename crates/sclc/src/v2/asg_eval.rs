use std::collections::HashMap;

use crate::{Eval, EvalCtx, EvalEnv, EvalError, ModuleId, TrackedValue};

use super::{Asg, NodeId};

/// Results from the ASG-driven evaluator.
pub struct EvalResults {
    /// Evaluated module export records, keyed by `ModuleId`.
    pub modules: HashMap<ModuleId, TrackedValue>,
}

/// ASG-driven evaluator that walks the ASG's global SCC ordering.
///
/// Replaces the per-module evaluation in `CompilationUnit::eval()` with a
/// global SCC walk driven by the ASG's dependency graph. Expression-level
/// evaluation (`eval_expr`, `eval_call`, etc.) is delegated to the existing
/// `Eval`.
pub struct AsgEvaluator<'a> {
    asg: &'a Asg,
    ctx: EvalCtx,
}

impl<'a> AsgEvaluator<'a> {
    pub fn new(asg: &'a Asg, ctx: EvalCtx) -> Self {
        Self { asg, ctx }
    }

    /// Evaluate the entire program by walking the ASG's SCC ordering.
    pub fn eval(self) -> Result<EvalResults, EvalError> {
        // Transitional: create CompilationUnit for expression-level evaluation.
        // The Eval struct needs a CompilationUnit for import resolution and
        // extern loading. This will be removed once the AsgEvaluator handles
        // all resolution natively.
        let unit = super::compile::asg_to_compilation_unit(self.asg);
        let evaluator = Eval::from_ctx(&unit, self.ctx);

        let mut modules = HashMap::new();

        // Evaluate modules in SCC topological order. Module nodes depend on
        // their contents via containment edges, so by the time a Module node
        // is reached, all its dependencies (including imported modules) have
        // been visited. We delegate to eval_file_mod for full per-module
        // evaluation.
        let sccs = self.asg.compute_sccs();
        for scc in &sccs {
            for node in scc {
                if let NodeId::Module(raw_id) = node
                    && let Some(module_node) = self.asg.module(raw_id)
                {
                    let env = EvalEnv::new().with_module_id(&module_node.module_id);
                    let value = evaluator.eval_file_mod(&env, &module_node.file_mod)?;
                    modules.insert(module_node.module_id.clone(), value);
                }
            }
        }

        Ok(EvalResults { modules })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn evaluator_on_empty_asg() {
        let asg = super::super::Asg::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = EvalCtx::new(tx, "test");
        let results = AsgEvaluator::new(&asg, ctx).eval().unwrap();
        assert!(results.modules.is_empty());
    }
}
