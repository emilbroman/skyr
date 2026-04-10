use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::eval::{FnEnv, tracked, with_dependencies};
use crate::{
    Eval, EvalCtx, EvalEnv, EvalError, FnValue, Loc, ModuleId, PackageId, Record, Span,
    TrackedValue, Value, ast,
};

use super::{Asg, NodeId, RawModuleId};

/// Results from the ASG-driven evaluator.
pub struct EvalResults {
    /// Evaluated module export records, keyed by `ModuleId`.
    pub modules: HashMap<ModuleId, TrackedValue>,
}

/// ASG-driven evaluator that walks the ASG's global SCC ordering.
///
/// Processes individual Global nodes in topological SCC order rather than
/// delegating to the per-module `eval_file_mod`. Import references resolve
/// to pre-assembled module values, eliminating recursive `eval_file_mod`
/// calls and the associated stack overflow on circular module imports.
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
        // The Eval struct is needed for expression-level evaluation (eval_expr,
        // eval_call, extern resolution, resource effects, etc.). We create a
        // CompilationUnit for it, but bypass its module-level orchestration.
        let unit = super::compile::asg_to_compilation_unit(self.asg);
        let evaluator = Eval::from_ctx(&unit, self.ctx);

        let mut state = EvalState {
            import_maps: build_import_maps(self.asg),
            global_values: HashMap::new(),
            module_values: HashMap::new(),
        };

        let sccs = self.asg.compute_sccs();
        for scc in &sccs {
            process_scc(self.asg, &evaluator, scc, &mut state)?;
        }

        // Evaluate bare expression statements (resource calls, etc.).
        for (raw_id, stmt) in self.asg.global_exprs() {
            if let ast::ModStmt::Expr(expr) = stmt
                && let Some(module_node) = self.asg.module(raw_id)
            {
                let globals = module_node.file_mod.find_globals();
                let mut env = EvalEnv::new()
                    .with_module_id(&module_node.module_id)
                    .with_globals(&globals);
                env = add_resolved_imports(&env, raw_id, &state.import_maps, &state.module_values);
                env = add_evaluated_globals(&env, raw_id, &state.global_values);
                evaluator.eval_expr(&env, expr)?;
            }
        }

        // Convert to ModuleId-keyed results.
        let mut modules = HashMap::new();
        for (raw_id, value) in state.module_values {
            if let Some(module_node) = self.asg.module(&raw_id) {
                modules.insert(module_node.module_id.clone(), value);
            }
        }

        Ok(EvalResults { modules })
    }
}

/// Accumulated evaluation state passed through SCC processing.
struct EvalState {
    import_maps: HashMap<RawModuleId, HashMap<String, RawModuleId>>,
    global_values: HashMap<(RawModuleId, String), TrackedValue>,
    module_values: HashMap<RawModuleId, TrackedValue>,
}

// ─── SCC processing ──────────────────────────────────────────────────────────

fn process_scc(
    asg: &Asg,
    evaluator: &Eval<'_>,
    scc: &[NodeId],
    state: &mut EvalState,
) -> Result<(), EvalError> {
    // Separate node types.
    let global_nodes: Vec<&NodeId> = scc
        .iter()
        .filter(|n| matches!(n, NodeId::Global(..)))
        .collect();
    let module_nodes: Vec<&NodeId> = scc
        .iter()
        .filter(|n| matches!(n, NodeId::Module(..)))
        .collect();
    // TypeDecl nodes have no runtime value — skip.

    if global_nodes.len() == 1 && module_nodes.is_empty() {
        // Singleton global — most common case.
        let NodeId::Global(raw_id, name) = global_nodes[0] else {
            unreachable!()
        };
        let has_self_edge = asg.has_self_edge(global_nodes[0]);
        eval_singleton_global(asg, evaluator, raw_id, name, has_self_edge, state)?;
    } else if global_nodes.len() > 1 && module_nodes.is_empty() {
        // Multi-node global SCC (mutually recursive functions).
        eval_recursive_group(asg, evaluator, &global_nodes, state)?;
    } else if !global_nodes.is_empty() && !module_nodes.is_empty() {
        // Mixed SCC — globals and modules in a cycle (cross-module refs).
        eval_mixed_scc(asg, evaluator, &global_nodes, &module_nodes, state)?;
    }

    // Assemble module export records for pure-module SCCs and any module
    // nodes not yet assembled (from singleton or multi-global SCCs that
    // didn't include module nodes).
    for node in &module_nodes {
        let NodeId::Module(raw_id) = node else {
            continue;
        };
        if !state.module_values.contains_key(raw_id) {
            assemble_module(asg, raw_id, &state.global_values, &mut state.module_values);
        }
    }

    Ok(())
}

// ─── Singleton global evaluation ─────────────────────────────────────────────

fn eval_singleton_global(
    asg: &Asg,
    evaluator: &Eval<'_>,
    raw_id: &RawModuleId,
    name: &str,
    has_self_edge: bool,
    state: &mut EvalState,
) -> Result<(), EvalError> {
    let module_node = asg.module(raw_id).unwrap();

    // Find the LetBind in the module's file_mod (for correct AST pointer
    // identity with the globals map used by eval_var_name).
    let let_bind = find_let_bind(&module_node.file_mod, name).unwrap();

    let globals = module_node.file_mod.find_globals();
    let mut env = EvalEnv::new()
        .with_module_id(&module_node.module_id)
        .with_globals(&globals);
    env = add_resolved_imports(&env, raw_id, &state.import_maps, &state.module_values);
    env = add_evaluated_globals(&env, raw_id, &state.global_values);

    if has_self_edge {
        let value = build_self_recursive_fn(evaluator, &env, name, let_bind)?;
        state
            .global_values
            .insert((raw_id.clone(), name.to_string()), value);
    } else {
        let value = evaluator.eval_expr(&env, let_bind.expr.as_ref())?;
        state
            .global_values
            .insert((raw_id.clone(), name.to_string()), value);
    }

    Ok(())
}

/// Build a self-recursive FnValue, mirroring the logic in `eval_var_name`.
fn build_self_recursive_fn(
    evaluator: &Eval<'_>,
    env: &EvalEnv<'_>,
    name: &str,
    let_bind: &ast::LetBind,
) -> Result<TrackedValue, EvalError> {
    let ast::Expr::Fn(fn_expr) = let_bind.expr.as_ref().as_ref() else {
        // Non-function self-recursive global — shouldn't reach here if the
        // checker diagnosed CyclicDependency, but return Nil gracefully.
        return Ok(tracked(Value::Nil));
    };

    let fn_module_id = env.module_id()?;
    let parameters: Vec<String> = fn_expr.params.iter().map(|p| p.var.name.clone()).collect();
    let body = fn_expr
        .body
        .as_ref()
        .map(|b| *b.clone())
        .unwrap_or_else(|| Loc::new(ast::Expr::Nil, Span::default()));

    let free_vars = let_bind.expr.as_ref().free_vars();
    let global_env = env.without_locals();
    let mut captures = HashMap::new();
    for fv in &free_vars {
        if *fv != name {
            captures.insert(fv.to_string(), evaluator.eval_var_name(&global_env, fv)?);
        }
    }

    let fn_val = FnValue {
        env: FnEnv {
            module_id: fn_module_id,
            captures,
            parameters,
            self_name: Some(name.to_string()),
            recursive_group: None,
        },
        body,
    };
    Ok(tracked(Value::Fn(fn_val)))
}

// ─── Mutually recursive function groups ──────────────────────────────────────

fn eval_recursive_group(
    asg: &Asg,
    evaluator: &Eval<'_>,
    global_nodes: &[&NodeId],
    state: &mut EvalState,
) -> Result<(), EvalError> {
    // All members must be functions (cyclic non-function dependencies are
    // diagnosed as errors by the checker).
    let scc_names: HashSet<&str> = global_nodes
        .iter()
        .filter_map(|n| {
            if let NodeId::Global(_, name) = n {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect();

    // First pass: build preliminary FnValues.
    let mut preliminary: Vec<(String, FnValue)> = Vec::new();
    for node in global_nodes {
        let NodeId::Global(raw_id, name) = node else {
            continue;
        };
        let module_node = asg.module(raw_id).unwrap();
        let let_bind = find_let_bind(&module_node.file_mod, name).unwrap();
        let ast::Expr::Fn(fn_expr) = let_bind.expr.as_ref().as_ref() else {
            // Non-function in recursive SCC — assign Nil.
            state
                .global_values
                .insert((raw_id.clone(), name.clone()), tracked(Value::Nil));
            continue;
        };

        let globals = module_node.file_mod.find_globals();
        let mut env = EvalEnv::new()
            .with_module_id(&module_node.module_id)
            .with_globals(&globals);
        env = add_resolved_imports(&env, raw_id, &state.import_maps, &state.module_values);
        env = add_evaluated_globals(&env, raw_id, &state.global_values);

        let fn_module_id = env.module_id()?;
        let parameters: Vec<String> = fn_expr.params.iter().map(|p| p.var.name.clone()).collect();
        let body = fn_expr
            .body
            .as_ref()
            .map(|b| *b.clone())
            .unwrap_or_else(|| Loc::new(ast::Expr::Nil, Span::default()));

        let free_vars = let_bind.expr.as_ref().free_vars();
        let global_env = env.without_locals();
        let mut captures = HashMap::new();
        for fv in &free_vars {
            if !scc_names.contains(fv) {
                captures.insert(fv.to_string(), evaluator.eval_var_name(&global_env, fv)?);
            }
        }

        preliminary.push((
            name.clone(),
            FnValue {
                env: FnEnv {
                    module_id: fn_module_id,
                    captures,
                    parameters,
                    self_name: None,
                    recursive_group: None,
                },
                body,
            },
        ));
    }

    // Second pass: wire up shared recursive group.
    let shared_group = Arc::new(preliminary.clone());
    for (_, fn_val) in &mut preliminary {
        fn_val.env.recursive_group = Some(shared_group.clone());
    }

    // Store results.
    for (i, node) in global_nodes.iter().enumerate() {
        let NodeId::Global(raw_id, name) = node else {
            continue;
        };
        if i < preliminary.len() && preliminary[i].0 == *name {
            state.global_values.insert(
                (raw_id.clone(), name.clone()),
                TrackedValue::new(Value::Fn(preliminary[i].1.clone())),
            );
        }
    }

    Ok(())
}

// ─── Mixed SCC (globals + modules in a cycle) ───────────────────────────────

fn eval_mixed_scc(
    asg: &Asg,
    evaluator: &Eval<'_>,
    global_nodes: &[&NodeId],
    module_nodes: &[&NodeId],
    state: &mut EvalState,
) -> Result<(), EvalError> {
    // In a mixed SCC, globals reference modules that haven't been assembled
    // yet. For function globals with lazy cross-module references, we:
    //
    // 1. Evaluate all globals (functions capture what's available; unresolved
    //    imports are not captured — they'll be resolved by eval_var_name's
    //    fallback to the globals map at call time).
    // 2. Assemble all modules from the evaluated globals.
    //
    // This works for the common pattern where circular imports exist but the
    // actual value-level references are deferred through function bodies.

    let all_fns = global_nodes.iter().all(|n| {
        if let NodeId::Global(raw_id, name) = n {
            let module_node = asg.module(raw_id).unwrap();
            find_let_bind(&module_node.file_mod, name)
                .map(|lb| matches!(lb.expr.as_ref().as_ref(), ast::Expr::Fn(_)))
                .unwrap_or(false)
        } else {
            true
        }
    });

    if all_fns && global_nodes.len() > 1 {
        // Build a cross-module recursive group.
        eval_recursive_group(asg, evaluator, global_nodes, state)?;
    } else {
        // Process globals individually.
        for node in global_nodes {
            let NodeId::Global(raw_id, name) = node else {
                continue;
            };
            let has_self_edge = asg.has_self_edge(node);
            eval_singleton_global(asg, evaluator, raw_id, name, has_self_edge, state)?;
        }
    }

    // Assemble all modules in this SCC.
    for node in module_nodes {
        let NodeId::Module(raw_id) = node else {
            continue;
        };
        assemble_module(asg, raw_id, &state.global_values, &mut state.module_values);
    }

    Ok(())
}

// ─── Module assembly ─────────────────────────────────────────────────────────

fn assemble_module(
    asg: &Asg,
    raw_id: &RawModuleId,
    global_values: &HashMap<(RawModuleId, String), TrackedValue>,
    module_values: &mut HashMap<RawModuleId, TrackedValue>,
) {
    let Some(module_node) = asg.module(raw_id) else {
        return;
    };

    let mut exports = Record::default();
    let mut dependencies = BTreeSet::new();

    // Collect exports in statement order (matches eval_file_mod behavior).
    for stmt in &module_node.file_mod.statements {
        if let ast::ModStmt::Export(let_bind) = stmt {
            let key = (raw_id.clone(), let_bind.var.name.clone());
            if let Some(value) = global_values.get(&key) {
                dependencies.extend(value.dependencies.clone());
                exports.insert(let_bind.var.name.clone(), value.value.clone());
            }
        }
    }

    module_values.insert(
        raw_id.clone(),
        with_dependencies(Value::Record(exports), dependencies),
    );
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build per-module import alias → RawModuleId maps from the ASG.
fn build_import_maps(asg: &Asg) -> HashMap<RawModuleId, HashMap<String, RawModuleId>> {
    let mut maps = HashMap::new();
    for module_node in asg.modules() {
        let mut aliases = HashMap::new();
        for stmt in &module_node.file_mod.statements {
            if let ast::ModStmt::Import(import) = stmt {
                let vars = &import.as_ref().vars;
                if !vars.is_empty() {
                    let alias = vars.last().unwrap().name.clone();
                    let import_raw_id = resolve_import_path(vars, &module_node.package_id);
                    aliases.insert(alias, import_raw_id);
                }
            }
        }
        maps.insert(module_node.raw_id.clone(), aliases);
    }
    maps
}

/// Resolve an import path to a raw module ID, replacing `Self` with the
/// current package's segments.
fn resolve_import_path(vars: &[Loc<ast::Var>], pkg_id: &PackageId) -> RawModuleId {
    let segments: Vec<String> = vars.iter().map(|v| v.name.clone()).collect();
    if segments.first().is_some_and(|s| s == "Self") {
        let mut resolved: Vec<String> = pkg_id.as_slice().to_vec();
        resolved.extend(segments[1..].iter().cloned());
        resolved
    } else {
        segments
    }
}

/// Add already-assembled import module values to the env as precomputed
/// entries. This makes import aliases resolve via `eval_var_name`'s
/// precomputed check instead of triggering recursive `eval_file_mod`.
fn add_resolved_imports<'a>(
    env: &EvalEnv<'a>,
    raw_id: &RawModuleId,
    import_maps: &HashMap<RawModuleId, HashMap<String, RawModuleId>>,
    module_values: &HashMap<RawModuleId, TrackedValue>,
) -> EvalEnv<'a> {
    let mut env = env.inner();
    if let Some(imports) = import_maps.get(raw_id) {
        for (alias, import_raw_id) in imports {
            if let Some(module_value) = module_values.get(import_raw_id) {
                env = env.with_precomputed(alias.clone(), module_value.clone());
            }
        }
    }
    env
}

/// Add already-evaluated same-module globals to the env as precomputed
/// entries. These override the globals map for names that have been
/// evaluated, avoiding redundant lazy evaluation via `eval_var_name`.
fn add_evaluated_globals<'a>(
    env: &EvalEnv<'a>,
    raw_id: &RawModuleId,
    global_values: &HashMap<(RawModuleId, String), TrackedValue>,
) -> EvalEnv<'a> {
    let mut env = env.inner();
    for ((rid, name), value) in global_values {
        if rid == raw_id {
            env = env.with_precomputed(name.clone(), value.clone());
        }
    }
    env
}

/// Find a LetBind by name in a module's statements.
fn find_let_bind<'m>(file_mod: &'m ast::FileMod, name: &str) -> Option<&'m ast::LetBind> {
    file_mod.statements.iter().find_map(|stmt| match stmt {
        ast::ModStmt::Let(lb) | ast::ModStmt::Export(lb) if lb.var.name == name => Some(lb),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::v2::{InMemoryPackage, Loader, build_default_finder};

    #[tokio::test]
    async fn evaluator_on_empty_asg() {
        let asg = super::super::Asg::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = EvalCtx::new(tx, "test");
        let results = AsgEvaluator::new(&asg, ctx).eval().unwrap();
        assert!(results.modules.is_empty());
    }

    #[tokio::test]
    async fn eval_simple_export() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"export let x = 42".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let asg = loader.finish().into_inner();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = EvalCtx::new(tx, "test");
        let results = AsgEvaluator::new(&asg, ctx).eval().unwrap();

        let main_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        let main_val = results.modules.get(&main_id).unwrap();
        assert_eq!(main_val.value.to_string(), "{x: 42}");
    }

    #[tokio::test]
    async fn eval_with_import() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nexport let x = Lib.foo".to_vec(),
        );
        files.insert(PathBuf::from("Lib.scl"), b"export let foo = 42".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let asg = loader.finish().into_inner();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = EvalCtx::new(tx, "test");
        let results = AsgEvaluator::new(&asg, ctx).eval().unwrap();

        let main_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        let main_val = results.modules.get(&main_id).unwrap();
        assert_eq!(main_val.value.to_string(), "{x: 42}");
    }
}
