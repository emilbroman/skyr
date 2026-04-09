use std::collections::{HashMap, HashSet};

use crate::ast::{Expr, ListItem, ModStmt, TypeDef, TypeExpr};
use crate::{ModuleId, Program};

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
// Property access extraction
// ═══════════════════════════════════════════════════════════════════════════════

/// Collect all `Var(name).property` patterns in an expression tree.
/// Returns a map from base variable name to the set of property names accessed.
/// This is a purely structural walk — scoping is NOT considered. Callers should
/// intersect with `free_vars()` to determine which bases are actually imports.
pub fn property_access_refs(expr: &Expr) -> HashMap<&str, HashSet<&str>> {
    let mut result: HashMap<&str, HashSet<&str>> = HashMap::new();
    collect_property_access_refs(expr, &mut result);
    result
}

fn collect_property_access_refs<'a>(expr: &'a Expr, out: &mut HashMap<&'a str, HashSet<&'a str>>) {
    match expr {
        Expr::PropertyAccess(pa) => {
            if let Expr::Var(var) = pa.expr.as_ref().as_ref() {
                out.entry(var.name.as_str())
                    .or_default()
                    .insert(pa.property.name.as_str());
            }
            // Also recurse into the base expression for chained access
            collect_property_access_refs(pa.expr.as_ref().as_ref(), out);
        }
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Nil
        | Expr::Str(_)
        | Expr::Path(_)
        | Expr::Extern(_)
        | Expr::Exception(_)
        | Expr::Var(_) => {}
        Expr::If(e) => {
            collect_property_access_refs(e.condition.as_ref().as_ref(), out);
            collect_property_access_refs(e.then_expr.as_ref().as_ref(), out);
            if let Some(else_expr) = &e.else_expr {
                collect_property_access_refs(else_expr.as_ref().as_ref(), out);
            }
        }
        Expr::Let(e) => {
            collect_property_access_refs(e.bind.expr.as_ref().as_ref(), out);
            if let Some(body) = &e.expr {
                collect_property_access_refs(body.as_ref().as_ref(), out);
            }
        }
        Expr::Fn(e) => {
            if let Some(body) = &e.body {
                collect_property_access_refs(body.as_ref().as_ref(), out);
            }
        }
        Expr::Call(e) => {
            collect_property_access_refs(e.callee.as_ref().as_ref(), out);
            for arg in &e.args {
                collect_property_access_refs(arg.as_ref(), out);
            }
        }
        Expr::Unary(e) => {
            collect_property_access_refs(e.expr.as_ref().as_ref(), out);
        }
        Expr::Binary(e) => {
            collect_property_access_refs(e.lhs.as_ref().as_ref(), out);
            collect_property_access_refs(e.rhs.as_ref().as_ref(), out);
        }
        Expr::Record(e) => {
            for field in &e.fields {
                collect_property_access_refs(field.expr.as_ref(), out);
            }
        }
        Expr::Dict(e) => {
            for entry in &e.entries {
                collect_property_access_refs(entry.key.as_ref(), out);
                collect_property_access_refs(entry.value.as_ref(), out);
            }
        }
        Expr::List(e) => {
            for item in &e.items {
                collect_list_item_refs(item, out);
            }
        }
        Expr::Interp(e) => {
            for part in &e.parts {
                collect_property_access_refs(part.as_ref(), out);
            }
        }
        Expr::IndexedAccess(e) => {
            collect_property_access_refs(e.expr.as_ref().as_ref(), out);
            collect_property_access_refs(e.index.as_ref().as_ref(), out);
        }
        Expr::TypeCast(e) => {
            collect_property_access_refs(e.expr.as_ref().as_ref(), out);
        }
        Expr::Raise(e) => {
            collect_property_access_refs(e.expr.as_ref().as_ref(), out);
        }
        Expr::Try(e) => {
            collect_property_access_refs(e.expr.as_ref().as_ref(), out);
            for catch in &e.catches {
                collect_property_access_refs(catch.body.as_ref(), out);
            }
        }
    }
}

fn collect_list_item_refs<'a>(item: &'a ListItem, out: &mut HashMap<&'a str, HashSet<&'a str>>) {
    match item {
        ListItem::Expr(expr) => collect_property_access_refs(expr.as_ref(), out),
        ListItem::If(item) => {
            collect_property_access_refs(item.condition.as_ref().as_ref(), out);
            collect_list_item_refs(&item.then_item, out);
        }
        ListItem::For(item) => {
            collect_property_access_refs(item.iterable.as_ref().as_ref(), out);
            collect_list_item_refs(&item.emit_item, out);
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

/// Information about a module's globals and imports needed for dependency analysis.
struct ModuleInfo<'a> {
    module_id: ModuleId,
    /// Global binding names in this module.
    globals: HashSet<&'a str>,
    /// Import alias → (resolved module ID, set of exported binding names).
    imports: HashMap<&'a str, (ModuleId, HashSet<&'a str>)>,
}

/// Build a program-wide dependency graph over all value bindings (let/export).
///
/// Nodes are `BindingId { module_id, name }` for every global in every module.
/// Edges represent "depends on" relationships:
/// - Intra-module: `free_vars()` filtered against same-module globals
/// - Cross-module: `PropertyAccess(Var(import), prop)` patterns resolved to target bindings
/// - Whole-import: if an import alias appears in `free_vars()` without property access,
///   edges are added to ALL exports of that module (conservative)
pub fn build_value_dep_graph(program: &Program) -> DepGraph {
    // First pass: collect module info (globals, imports, exports)
    let mut module_infos: Vec<ModuleInfo> = Vec::new();

    for (package_id, package) in program.packages() {
        for (path, file_mod) in package.modules() {
            let module_segments = path
                .with_extension("")
                .components()
                .map(|c| match c {
                    std::path::Component::Normal(s) => s.to_string_lossy().to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>();
            let mut full_segments = package_id.as_slice().to_vec();
            full_segments.extend(module_segments);
            let module_id = ModuleId::new(full_segments);

            let globals_map = file_mod.find_globals();
            let globals: HashSet<&str> = globals_map.keys().copied().collect();

            // Resolve imports: alias → (target module ID, target exports)
            let mut imports: HashMap<&str, (ModuleId, HashSet<&str>)> = HashMap::new();
            for statement in &file_mod.statements {
                if let ModStmt::Import(import_stmt) = statement {
                    let alias = import_stmt
                        .as_ref()
                        .vars
                        .last()
                        .expect("import has at least one segment");
                    let import_path_segments: Vec<String> = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|v| v.name.clone())
                        .collect();
                    let mut resolved = ModuleId::new(import_path_segments);

                    // Resolve Self/ prefix
                    if resolved.as_slice().first().map(String::as_str) == Some("Self")
                        && let Some(self_id) = program.self_package_id()
                    {
                        let mut segs = self_id.as_slice().to_vec();
                        segs.extend(resolved.as_slice()[1..].iter().cloned());
                        resolved = ModuleId::new(segs);
                    }

                    // Find the target module's globals
                    let target_exports = find_module_exports(program, &resolved);
                    imports.insert(alias.name.as_str(), (resolved, target_exports));
                }
            }

            module_infos.push(ModuleInfo {
                module_id,
                globals,
                imports,
            });
        }
    }

    // Second pass: build graph edges
    let mut graph = DepGraph::new();

    // Register all nodes first
    for info in &module_infos {
        for &name in &info.globals {
            graph.add_node(BindingId {
                module_id: info.module_id.clone(),
                name: name.to_string(),
            });
        }
    }

    // Now add edges by analyzing each binding's body
    for (package_id, package) in program.packages() {
        for (path, file_mod) in package.modules() {
            let module_segments = path
                .with_extension("")
                .components()
                .map(|c| match c {
                    std::path::Component::Normal(s) => s.to_string_lossy().to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>();
            let mut full_segments = package_id.as_slice().to_vec();
            full_segments.extend(module_segments);
            let module_id = ModuleId::new(full_segments);

            let info = module_infos
                .iter()
                .find(|i| i.module_id == module_id)
                .unwrap();

            for statement in &file_mod.statements {
                let let_bind = match statement {
                    ModStmt::Let(lb) | ModStmt::Export(lb) => lb,
                    _ => continue,
                };

                let from = BindingId {
                    module_id: module_id.clone(),
                    name: let_bind.var.name.clone(),
                };

                let free_vars = let_bind.expr.as_ref().free_vars();
                let prop_refs = property_access_refs(let_bind.expr.as_ref().as_ref());

                for var_name in &free_vars {
                    if info.globals.contains(var_name) {
                        // Intra-module dependency
                        let to = BindingId {
                            module_id: module_id.clone(),
                            name: var_name.to_string(),
                        };
                        if graph.node_index.contains_key(&to) {
                            graph.add_edge(&from, &to);
                        }
                    } else if let Some((target_module_id, target_exports)) =
                        info.imports.get(var_name)
                    {
                        // Cross-module dependency
                        if let Some(props) = prop_refs.get(var_name) {
                            // Specific property accesses: add edge for each accessed property
                            for &prop in props {
                                let to = BindingId {
                                    module_id: target_module_id.clone(),
                                    name: prop.to_string(),
                                };
                                if graph.node_index.contains_key(&to) {
                                    graph.add_edge(&from, &to);
                                }
                            }
                        } else {
                            // Import used as a value (not via property access):
                            // conservatively depend on all exports
                            for &export_name in target_exports {
                                let to = BindingId {
                                    module_id: target_module_id.clone(),
                                    name: export_name.to_string(),
                                };
                                if graph.node_index.contains_key(&to) {
                                    graph.add_edge(&from, &to);
                                }
                            }
                        }
                    }
                }
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

/// Find the exported global names of a module identified by its full module ID.
fn find_module_exports<'a>(program: &'a Program, module_id: &ModuleId) -> HashSet<&'a str> {
    for (package_id, package) in program.packages() {
        if !module_id.starts_with_package(package_id) {
            continue;
        }
        let Some(suffix) = module_id.suffix_after_package(package_id) else {
            continue;
        };
        if suffix.is_empty() {
            continue;
        }
        let module_path = suffix
            .iter()
            .cloned()
            .collect::<ModuleId>()
            .to_path_buf_with_extension("scl");
        for (path, file_mod) in package.modules() {
            if path == &module_path {
                return file_mod.find_globals().into_keys().collect();
            }
        }
    }
    HashSet::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bid(name: &str) -> BindingId {
        BindingId {
            module_id: ModuleId::default(),
            name: name.to_string(),
        }
    }

    fn bid_mod(module: &str, name: &str) -> BindingId {
        BindingId {
            module_id: ModuleId::new(vec![module.to_string()]),
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
