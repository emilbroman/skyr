use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::eval::{FnEnv, tracked, with_dependencies};
use crate::{
    Eval, EvalCtx, EvalEnv, EvalError, FnValue, GlobalEvalEnv, Loc, ModuleId, PackageId, Record,
    Span, TrackedValue, Value, ast, v2::GlobalKey,
};

use super::{Asg, NodeId, RawModuleId};

/// Results from the ASG-driven evaluator.
pub struct EvalResults {
    /// Evaluated module export records, keyed by `ModuleId`.
    pub modules: HashMap<ModuleId, TrackedValue>,
}

/// ASG-driven evaluator that walks the ASG's global SCC ordering.
///
/// Processes individual Global nodes in topological SCC order. Import
/// references resolve to pre-assembled module values in `GlobalEvalEnv`,
/// eliminating recursive `eval_file_mod` calls and the associated stack
/// overflow on circular module imports.
pub struct AsgEvaluator<'a> {
    asg: &'a Asg,
    ctx: EvalCtx,
    /// Optional pre-seeded evaluation environment from prior iterations
    /// (e.g. the REPL). Globals already present are skipped.
    initial_env: Option<GlobalEvalEnv>,
}

impl<'a> AsgEvaluator<'a> {
    pub fn new(asg: &'a Asg, ctx: EvalCtx) -> Self {
        Self {
            asg,
            ctx,
            initial_env: None,
        }
    }

    /// Pre-seed the evaluator with an existing `GlobalEvalEnv`.
    ///
    /// Globals already present in the initial env will be skipped during
    /// evaluation, avoiding duplicate side effects in incremental contexts
    /// like the REPL.
    pub fn with_initial_env(mut self, env: GlobalEvalEnv) -> Self {
        self.initial_env = Some(env);
        self
    }

    /// Evaluate the entire program by walking the ASG's SCC ordering.
    pub fn eval(self) -> Result<(EvalResults, GlobalEvalEnv), EvalError> {
        let externs = collect_externs(self.asg);
        let evaluator = Eval::from_externs(externs, self.ctx);

        let mut global_env = if let Some(mut env) = self.initial_env {
            // Merge import maps from the new ASG into the existing env.
            let new_maps = build_import_maps(self.asg);
            env.merge_import_maps(new_maps);
            env
        } else {
            GlobalEvalEnv::new(build_import_maps(self.asg))
        };

        let sccs = self.asg.compute_sccs();
        for scc in &sccs {
            process_scc(self.asg, &evaluator, scc, &mut global_env)?;
        }

        // Evaluate bare expression statements (resource calls, etc.).
        for (raw_id, stmt) in self.asg.global_exprs() {
            if let ast::ModStmt::Expr(expr) = stmt
                && let Some(mn) = self.asg.module(raw_id)
            {
                let env = EvalEnv::new(&global_env)
                    .with_module_id(&mn.module_id)
                    .with_raw_module_id(&mn.raw_id);
                evaluator.eval_expr(&env, expr)?;
            }
        }

        // Convert to ModuleId-keyed results.
        let mut modules = HashMap::new();
        for (key, value) in global_env.iter() {
            if let GlobalKey::ModuleValue(raw_id) = key
                && let Some(mn) = self.asg.module(raw_id)
            {
                modules.insert(mn.module_id.clone(), value.clone());
            }
        }

        Ok((EvalResults { modules }, global_env))
    }
}

// ─── SCC processing ──────────────────────────────────────────────────────────

fn process_scc(
    asg: &Asg,
    evaluator: &Eval<'_>,
    scc: &[NodeId],
    global_env: &mut GlobalEvalEnv,
) -> Result<(), EvalError> {
    let global_nodes: Vec<&NodeId> = scc
        .iter()
        .filter(|n| matches!(n, NodeId::Global(..)))
        .collect();
    let module_nodes: Vec<&NodeId> = scc
        .iter()
        .filter(|n| matches!(n, NodeId::Module(..)))
        .collect();

    if global_nodes.len() == 1 && module_nodes.is_empty() {
        let NodeId::Global(raw_id, name) = global_nodes[0] else {
            unreachable!()
        };
        let has_self_edge = asg.has_self_edge(global_nodes[0]);
        eval_singleton_global(asg, evaluator, raw_id, name, has_self_edge, global_env)?;
    } else if global_nodes.len() > 1 && module_nodes.is_empty() {
        eval_recursive_group(asg, evaluator, &global_nodes, global_env)?;
    } else if !global_nodes.is_empty() && !module_nodes.is_empty() {
        eval_mixed_scc(asg, evaluator, &global_nodes, &module_nodes, global_env)?;
    }

    // Assemble module export records.
    for node in &module_nodes {
        let NodeId::Module(raw_id) = node else {
            continue;
        };
        if global_env
            .get(&GlobalKey::ModuleValue(raw_id.clone()))
            .is_none()
        {
            assemble_module(asg, raw_id, global_env);
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
    global_env: &mut GlobalEvalEnv,
) -> Result<(), EvalError> {
    // Skip if already evaluated (e.g. from a previous REPL iteration).
    let key = GlobalKey::Global(raw_id.clone(), name.to_string());
    if global_env.get(&key).is_some() {
        return Ok(());
    }

    let mn = asg.module(raw_id).unwrap();
    let lb = find_let_bind(&mn.file_mod, name).unwrap();

    let env = EvalEnv::new(global_env)
        .with_module_id(&mn.module_id)
        .with_raw_module_id(&mn.raw_id);

    let value = if has_self_edge {
        build_self_recursive_fn(evaluator, &env, name, lb)?
    } else {
        evaluator.eval_expr(&env, lb.expr.as_ref())?
    };

    global_env.insert(GlobalKey::Global(raw_id.clone(), name.to_string()), value);
    Ok(())
}

/// Build a self-recursive FnValue.
fn build_self_recursive_fn(
    evaluator: &Eval<'_>,
    env: &EvalEnv<'_>,
    name: &str,
    let_bind: &ast::LetBind,
) -> Result<TrackedValue, EvalError> {
    let ast::Expr::Fn(fn_expr) = let_bind.expr.as_ref().as_ref() else {
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
    global_env: &mut GlobalEvalEnv,
) -> Result<(), EvalError> {
    // Skip if all members are already evaluated.
    let all_present = global_nodes.iter().all(|n| {
        if let NodeId::Global(raw_id, name) = n {
            global_env
                .get(&GlobalKey::Global(raw_id.clone(), name.clone()))
                .is_some()
        } else {
            true
        }
    });
    if all_present {
        return Ok(());
    }

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
        let mn = asg.module(raw_id).unwrap();
        let lb = find_let_bind(&mn.file_mod, name).unwrap();
        let ast::Expr::Fn(fn_expr) = lb.expr.as_ref().as_ref() else {
            global_env.insert(
                GlobalKey::Global(raw_id.clone(), name.clone()),
                tracked(Value::Nil),
            );
            continue;
        };

        let env = EvalEnv::new(global_env)
            .with_module_id(&mn.module_id)
            .with_raw_module_id(&mn.raw_id);

        let fn_module_id = env.module_id()?;
        let parameters: Vec<String> = fn_expr.params.iter().map(|p| p.var.name.clone()).collect();
        let body = fn_expr
            .body
            .as_ref()
            .map(|b| *b.clone())
            .unwrap_or_else(|| Loc::new(ast::Expr::Nil, Span::default()));

        let free_vars = lb.expr.as_ref().free_vars();
        let global_env_ref = env.without_locals();
        let mut captures = HashMap::new();
        for fv in &free_vars {
            if !scc_names.contains(fv) {
                captures.insert(
                    fv.to_string(),
                    evaluator.eval_var_name(&global_env_ref, fv)?,
                );
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
            global_env.insert(
                GlobalKey::Global(raw_id.clone(), name.clone()),
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
    global_env: &mut GlobalEvalEnv,
) -> Result<(), EvalError> {
    let all_fns = global_nodes.iter().all(|n| {
        if let NodeId::Global(raw_id, name) = n {
            let mn = asg.module(raw_id).unwrap();
            find_let_bind(&mn.file_mod, name)
                .map(|lb| matches!(lb.expr.as_ref().as_ref(), ast::Expr::Fn(_)))
                .unwrap_or(false)
        } else {
            true
        }
    });

    if all_fns && global_nodes.len() > 1 {
        eval_recursive_group(asg, evaluator, global_nodes, global_env)?;
    } else {
        for node in global_nodes {
            let NodeId::Global(raw_id, name) = node else {
                continue;
            };
            let has_self_edge = asg.has_self_edge(node);
            eval_singleton_global(asg, evaluator, raw_id, name, has_self_edge, global_env)?;
        }
    }

    for node in module_nodes {
        let NodeId::Module(raw_id) = node else {
            continue;
        };
        assemble_module(asg, raw_id, global_env);
    }

    Ok(())
}

// ─── Module assembly ─────────────────────────────────────────────────────────

fn assemble_module(asg: &Asg, raw_id: &RawModuleId, global_env: &mut GlobalEvalEnv) {
    let Some(mn) = asg.module(raw_id) else {
        return;
    };

    let mut exports = Record::default();
    let mut dependencies = BTreeSet::new();

    for stmt in &mn.file_mod.statements {
        if let ast::ModStmt::Export(lb) = stmt {
            let key = GlobalKey::Global(raw_id.clone(), lb.var.name.clone());
            if let Some(value) = global_env.get(&key) {
                dependencies.extend(value.dependencies.clone());
                exports.insert(lb.var.name.clone(), value.value.clone());
            }
        }
    }

    global_env.insert(
        GlobalKey::ModuleValue(raw_id.clone()),
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

fn find_let_bind<'m>(file_mod: &'m ast::FileMod, name: &str) -> Option<&'m ast::LetBind> {
    file_mod.statements.iter().find_map(|stmt| match stmt {
        ast::ModStmt::Let(lb) | ast::ModStmt::Export(lb) if lb.var.name == name => Some(lb),
        _ => None,
    })
}

fn collect_externs(asg: &Asg) -> HashMap<String, Value> {
    let mut externs = HashMap::new();
    for pkg in asg.packages().values() {
        pkg.register_externs(&mut externs);
    }
    externs
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
        let (results, _env) = AsgEvaluator::new(&asg, ctx).eval().unwrap();
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
        let (results, _env) = AsgEvaluator::new(&asg, ctx).eval().unwrap();

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
        let (results, _env) = AsgEvaluator::new(&asg, ctx).eval().unwrap();

        let main_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        let main_val = results.modules.get(&main_id).unwrap();
        assert_eq!(main_val.value.to_string(), "{x: 42}");
    }
}
