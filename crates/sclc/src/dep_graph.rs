use std::collections::{HashMap, HashSet};

use crate::ModuleId;
use crate::ast::{TypeDef, TypeExpr};

/// Identifies a binding (value or type) across modules.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BindingId {
    pub module_id: ModuleId,
    pub name: String,
}

impl std::fmt::Display for BindingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.module_id.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}.{}", self.module_id, self.name)
        }
    }
}

/// Directed dependency graph over bindings.
pub struct DepGraph {
    nodes: Vec<BindingId>,
    node_index: HashMap<BindingId, usize>,
    edges: Vec<HashSet<usize>>,
}

impl DepGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            node_index: HashMap::new(),
            edges: Vec::new(),
        }
    }

    /// Add a node to the graph. Returns its index. Idempotent.
    pub fn add_node(&mut self, id: BindingId) -> usize {
        if let Some(&idx) = self.node_index.get(&id) {
            return idx;
        }
        let idx = self.nodes.len();
        self.node_index.insert(id.clone(), idx);
        self.nodes.push(id);
        self.edges.push(HashSet::new());
        idx
    }

    /// Add a directed edge: `from` depends on `to`.
    pub fn add_edge(&mut self, from: &BindingId, to: &BindingId) {
        let from_idx = self.node_index[from];
        let to_idx = self.node_index[to];
        self.edges[from_idx].insert(to_idx);
    }

    /// Returns true if the node has a self-edge.
    pub fn has_self_edge(&self, id: &BindingId) -> bool {
        let idx = self.node_index[id];
        self.edges[idx].contains(&idx)
    }

    /// Compute strongly connected components using Tarjan's algorithm.
    /// Returns SCCs in topological order (dependencies before dependents).
    pub fn compute_sccs(&self) -> Vec<Vec<BindingId>> {
        let n = self.nodes.len();
        let mut state = TarjanState {
            index_counter: 0,
            stack: Vec::new(),
            on_stack: vec![false; n],
            index: vec![None; n],
            lowlink: vec![0; n],
            result: Vec::new(),
            graph: self,
        };

        for v in 0..n {
            if state.index[v].is_none() {
                state.strongconnect(v);
            }
        }

        state.result
    }
}

struct TarjanState<'a> {
    index_counter: usize,
    stack: Vec<usize>,
    on_stack: Vec<bool>,
    index: Vec<Option<usize>>,
    lowlink: Vec<usize>,
    result: Vec<Vec<BindingId>>,
    graph: &'a DepGraph,
}

impl TarjanState<'_> {
    fn strongconnect(&mut self, v: usize) {
        self.index[v] = Some(self.index_counter);
        self.lowlink[v] = self.index_counter;
        self.index_counter += 1;
        self.stack.push(v);
        self.on_stack[v] = true;

        for &w in &self.graph.edges[v] {
            if self.index[w].is_none() {
                self.strongconnect(w);
                self.lowlink[v] = self.lowlink[v].min(self.lowlink[w]);
            } else if self.on_stack[w] {
                self.lowlink[v] = self.lowlink[v].min(self.index[w].unwrap());
            }
        }

        if self.lowlink[v] == self.index[v].unwrap() {
            let mut component = Vec::new();
            loop {
                let w = self.stack.pop().unwrap();
                self.on_stack[w] = false;
                component.push(self.graph.nodes[w].clone());
                if w == v {
                    break;
                }
            }
            self.result.push(component);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Value binding dependency graph construction
// ═══════════════════════════════════════════════════════════════════════════════

/// Build a dependency graph for value bindings within a single module.
/// Uses `free_vars()` on each binding's expression, filtered to same-module globals.
pub fn build_intra_module_value_dep_graph<'a>(
    globals: &std::collections::HashMap<
        &'a str,
        (
            crate::Span,
            &'a crate::Loc<crate::ast::Expr>,
            Option<&'a str>,
        ),
    >,
) -> DepGraph {
    let module_id = ModuleId::default();
    let mut graph = DepGraph::new();

    // Register all globals as nodes
    for &name in globals.keys() {
        graph.add_node(BindingId {
            module_id: module_id.clone(),
            name: name.to_string(),
        });
    }

    // Add edges based on free_vars filtered to same-module globals
    for (&name, (_, expr, _)) in globals {
        let from = BindingId {
            module_id: module_id.clone(),
            name: name.to_string(),
        };
        let free_vars = expr.as_ref().free_vars();
        for var_name in &free_vars {
            if globals.contains_key(var_name) {
                let to = BindingId {
                    module_id: module_id.clone(),
                    name: var_name.to_string(),
                };
                graph.add_edge(&from, &to);
            }
        }
    }

    graph
}

// ═══════════════════════════════════════════════════════════════════════════════
// Type declaration dependency extraction
// ═══════════════════════════════════════════════════════════════════════════════

const BUILTIN_TYPES: &[&str] = &["Any", "Int", "Float", "Bool", "Str", "Path", "Never"];

/// Collect free type names referenced in a type expression.
/// Returns variable names that are NOT built-in types.
pub fn free_type_names(ty: &TypeExpr) -> HashSet<&str> {
    let mut names = HashSet::new();
    collect_free_type_names(ty, &mut names);
    names
}

fn collect_free_type_names<'a>(ty: &'a TypeExpr, out: &mut HashSet<&'a str>) {
    match ty {
        TypeExpr::Var(var) => {
            if !BUILTIN_TYPES.contains(&var.name.as_str()) {
                out.insert(var.name.as_str());
            }
        }
        TypeExpr::Optional(inner) => collect_free_type_names(inner.as_ref().as_ref(), out),
        TypeExpr::List(inner) => collect_free_type_names(inner.as_ref().as_ref(), out),
        TypeExpr::Fn(f) => {
            // Type params are bound — subtract them after collecting from body
            let mut body_names = HashSet::new();
            for param in &f.params {
                collect_free_type_names(param.as_ref(), &mut body_names);
            }
            collect_free_type_names(f.ret.as_ref().as_ref(), &mut body_names);
            for tp in &f.type_params {
                body_names.remove(tp.var.name.as_str());
                // But bounds themselves are dependencies
                if let Some(bound) = &tp.bound {
                    collect_free_type_names(bound.as_ref(), out);
                }
            }
            out.extend(body_names);
        }
        TypeExpr::Record(r) => {
            for field in &r.fields {
                collect_free_type_names(field.ty.as_ref(), out);
            }
        }
        TypeExpr::Dict(d) => {
            collect_free_type_names(d.key.as_ref().as_ref(), out);
            collect_free_type_names(d.value.as_ref().as_ref(), out);
        }
        TypeExpr::PropertyAccess(pa) => {
            collect_free_type_names(pa.expr.as_ref().as_ref(), out);
        }
        TypeExpr::Application(app) => {
            collect_free_type_names(app.base.as_ref().as_ref(), out);
            for arg in &app.args {
                collect_free_type_names(arg.as_ref(), out);
            }
        }
    }
}

/// Get the dependency names for a type definition, excluding its own type parameters.
pub fn type_def_deps(type_def: &TypeDef) -> HashSet<&str> {
    let mut names = free_type_names(type_def.ty.as_ref());
    // Subtract own type parameters — they're bound, not dependencies
    for tp in &type_def.type_params {
        names.remove(tp.var.name.as_str());
        // But bounds on type params ARE dependencies
        if let Some(bound) = &tp.bound {
            names.extend(free_type_names(bound.as_ref()));
        }
    }
    names
}

/// Build a dependency graph over type declarations within a module.
/// Nodes are type definition names; edges represent "references" relationships.
/// Only intra-module type deps are tracked (cross-module types go through imports
/// which are resolved before type checking).
pub fn build_type_dep_graph(type_defs: &[&TypeDef]) -> DepGraph {
    let type_names: HashSet<&str> = type_defs.iter().map(|td| td.var.name.as_str()).collect();
    let module_id = ModuleId::default(); // Type dep graph is intra-module

    let mut graph = DepGraph::new();

    // Register all type def nodes
    for td in type_defs {
        graph.add_node(BindingId {
            module_id: module_id.clone(),
            name: td.var.name.clone(),
        });
    }

    // Add edges
    for td in type_defs {
        let from = BindingId {
            module_id: module_id.clone(),
            name: td.var.name.clone(),
        };
        let deps = type_def_deps(td);
        for dep_name in deps {
            if type_names.contains(dep_name) {
                let to = BindingId {
                    module_id: module_id.clone(),
                    name: dep_name.to_string(),
                };
                graph.add_edge(&from, &to);
            }
        }
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackageId;

    fn bid(name: &str) -> BindingId {
        BindingId {
            module_id: ModuleId::default(),
            name: name.to_string(),
        }
    }

    fn bid_mod(module: &str, name: &str) -> BindingId {
        BindingId {
            module_id: ModuleId::new(PackageId::default(), vec![module.to_string()]),
            name: name.to_string(),
        }
    }

    fn scc_names(sccs: &[Vec<BindingId>]) -> Vec<Vec<String>> {
        sccs.iter()
            .map(|scc| {
                let mut names: Vec<String> = scc.iter().map(|b| b.name.clone()).collect();
                names.sort();
                names
            })
            .collect()
    }

    #[test]
    fn no_nodes() {
        let graph = DepGraph::new();
        let sccs = graph.compute_sccs();
        assert!(sccs.is_empty());
    }

    #[test]
    fn single_node_no_edges() {
        let mut graph = DepGraph::new();
        graph.add_node(bid("a"));
        let sccs = graph.compute_sccs();
        assert_eq!(scc_names(&sccs), vec![vec!["a"]]);
    }

    #[test]
    fn chain() {
        // a -> b -> c (no cycles)
        // Topo order: c, b, a
        let mut graph = DepGraph::new();
        graph.add_node(bid("a"));
        graph.add_node(bid("b"));
        graph.add_node(bid("c"));
        graph.add_edge(&bid("a"), &bid("b"));
        graph.add_edge(&bid("b"), &bid("c"));
        let sccs = graph.compute_sccs();
        let names = scc_names(&sccs);
        assert_eq!(names, vec![vec!["c"], vec!["b"], vec!["a"]]);
    }

    #[test]
    fn simple_cycle() {
        // a -> b -> a
        let mut graph = DepGraph::new();
        graph.add_node(bid("a"));
        graph.add_node(bid("b"));
        graph.add_edge(&bid("a"), &bid("b"));
        graph.add_edge(&bid("b"), &bid("a"));
        let sccs = graph.compute_sccs();
        let names = scc_names(&sccs);
        assert_eq!(names.len(), 1);
        assert_eq!(names[0], vec!["a", "b"]);
    }

    #[test]
    fn self_loop() {
        let mut graph = DepGraph::new();
        graph.add_node(bid("a"));
        graph.add_edge(&bid("a"), &bid("a"));
        let sccs = graph.compute_sccs();
        let names = scc_names(&sccs);
        assert_eq!(names, vec![vec!["a"]]);
        assert!(graph.has_self_edge(&bid("a")));
    }

    #[test]
    fn diamond_no_cycle() {
        // a -> b, a -> c, b -> d, c -> d
        let mut graph = DepGraph::new();
        for name in ["a", "b", "c", "d"] {
            graph.add_node(bid(name));
        }
        graph.add_edge(&bid("a"), &bid("b"));
        graph.add_edge(&bid("a"), &bid("c"));
        graph.add_edge(&bid("b"), &bid("d"));
        graph.add_edge(&bid("c"), &bid("d"));
        let sccs = graph.compute_sccs();
        let names = scc_names(&sccs);
        // All singletons, d before b and c, b and c before a
        assert_eq!(names.len(), 4);
        assert_eq!(names[0], vec!["d"]);
        assert_eq!(names[3], vec!["a"]);
    }

    #[test]
    fn cycle_with_tail() {
        // a -> b -> c -> b (cycle: b, c), a depends on the cycle
        let mut graph = DepGraph::new();
        for name in ["a", "b", "c"] {
            graph.add_node(bid(name));
        }
        graph.add_edge(&bid("a"), &bid("b"));
        graph.add_edge(&bid("b"), &bid("c"));
        graph.add_edge(&bid("c"), &bid("b"));
        let sccs = graph.compute_sccs();
        let names = scc_names(&sccs);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], vec!["b", "c"]); // cycle first
        assert_eq!(names[1], vec!["a"]); // then the dependent
    }

    #[test]
    fn cross_module() {
        // A.x -> B.y -> A.x (cross-module cycle)
        let mut graph = DepGraph::new();
        let ax = bid_mod("A", "x");
        let by = bid_mod("B", "y");
        graph.add_node(ax.clone());
        graph.add_node(by.clone());
        graph.add_edge(&ax, &by);
        graph.add_edge(&by, &ax);
        let sccs = graph.compute_sccs();
        assert_eq!(sccs.len(), 1);
        let mut names: Vec<String> = sccs[0].iter().map(|b| format!("{}", b)).collect();
        names.sort();
        assert_eq!(names, vec!["A.x", "B.y"]);
    }

    #[test]
    fn independent_nodes() {
        let mut graph = DepGraph::new();
        graph.add_node(bid("a"));
        graph.add_node(bid("b"));
        graph.add_node(bid("c"));
        let sccs = graph.compute_sccs();
        assert_eq!(sccs.len(), 3);
        // All singletons, order doesn't matter for independent nodes
        for scc in &sccs {
            assert_eq!(scc.len(), 1);
        }
    }

    #[test]
    fn two_separate_cycles() {
        // a <-> b, c <-> d, a -> c
        let mut graph = DepGraph::new();
        for name in ["a", "b", "c", "d"] {
            graph.add_node(bid(name));
        }
        graph.add_edge(&bid("a"), &bid("b"));
        graph.add_edge(&bid("b"), &bid("a"));
        graph.add_edge(&bid("c"), &bid("d"));
        graph.add_edge(&bid("d"), &bid("c"));
        graph.add_edge(&bid("a"), &bid("c"));
        let sccs = graph.compute_sccs();
        let names = scc_names(&sccs);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], vec!["c", "d"]); // dependency first
        assert_eq!(names[1], vec!["a", "b"]);
    }
}
