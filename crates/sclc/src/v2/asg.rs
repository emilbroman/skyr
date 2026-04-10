use std::collections::HashMap;
use std::sync::Arc;

use crate::{ModuleId, PackageId, Span, ast};

use super::Package;

/// A raw (unresolved) module identifier — just the segments as found in source.
pub type RawModuleId = Vec<String>;

/// Identifies a node in the ASG.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeId {
    Module(RawModuleId),
    Global(RawModuleId, String),
    TypeDecl(RawModuleId, String),
}

/// Key for the accumulated global environment (checker and evaluator).
///
/// Unlike `NodeId`, this distinguishes between value-level and type-level
/// results for modules (a module produces both a value-level export record
/// and a type-level export record).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GlobalKey {
    /// A value-level global binding.
    Global(RawModuleId, String),
    /// A type-level declaration.
    TypeDecl(RawModuleId, String),
    /// A module's value-level export record.
    ModuleValue(RawModuleId),
    /// A module's type-level export record (checker only).
    ModuleTypeLevel(RawModuleId),
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeId::Module(id) => write!(f, "module:{}", id.join("/")),
            NodeId::Global(id, name) => write!(f, "global:{}.{}", id.join("/"), name),
            NodeId::TypeDecl(id, name) => write!(f, "type:{}.{}", id.join("/"), name),
        }
    }
}

/// Metadata about an edge in the graph.
#[derive(Clone, Debug)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    /// Whether the reference crosses a function boundary (deferred evaluation).
    pub lazy: bool,
    /// Location of the reference that created this edge.
    pub span: Option<Span>,
}

/// A parsed module with its metadata.
#[derive(Clone)]
pub struct ModuleNode {
    pub raw_id: RawModuleId,
    pub module_id: ModuleId,
    pub file_mod: ast::FileMod,
    pub package_id: PackageId,
}

/// A global value binding in the ASG.
#[derive(Clone)]
pub struct GlobalNode {
    pub raw_module_id: RawModuleId,
    pub name: String,
    pub span: Span,
    pub stmt: ast::LetBind,
    pub is_export: bool,
}

/// A type declaration in the ASG.
#[derive(Clone)]
pub struct TypeDeclNode {
    pub raw_module_id: RawModuleId,
    pub name: String,
    pub type_def: ast::TypeDef,
    pub is_export: bool,
}

/// The Abstract Syntax Graph — the output of the Loader.
#[derive(Clone)]
pub struct Asg {
    modules: HashMap<RawModuleId, ModuleNode>,
    globals: HashMap<(RawModuleId, String), GlobalNode>,
    type_decls: HashMap<(RawModuleId, String), TypeDeclNode>,
    edges: Vec<Edge>,
    /// Global expression statements (bare side-effectful calls).
    /// Not graph nodes — evaluated last.
    global_exprs: Vec<(RawModuleId, ast::ModStmt)>,
    /// Package registry: maps PackageId to the package instance.
    packages: HashMap<PackageId, Arc<dyn Package>>,
}

impl Asg {
    pub(crate) fn new() -> Self {
        Self {
            modules: HashMap::new(),
            globals: HashMap::new(),
            type_decls: HashMap::new(),
            edges: Vec::new(),
            global_exprs: Vec::new(),
            packages: HashMap::new(),
        }
    }

    // ── Mutation (used by the Loader) ──────────────────────────────────

    pub(crate) fn add_module(&mut self, node: ModuleNode) {
        self.modules.insert(node.raw_id.clone(), node);
    }

    pub(crate) fn has_module(&self, raw_id: &[String]) -> bool {
        self.modules.contains_key(raw_id)
    }

    pub(crate) fn add_global(&mut self, node: GlobalNode) {
        self.globals
            .insert((node.raw_module_id.clone(), node.name.clone()), node);
    }

    pub(crate) fn add_type_decl(&mut self, node: TypeDeclNode) {
        self.type_decls
            .insert((node.raw_module_id.clone(), node.name.clone()), node);
    }

    pub(crate) fn add_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    pub(crate) fn add_global_expr(&mut self, raw_id: RawModuleId, stmt: ast::ModStmt) {
        self.global_exprs.push((raw_id, stmt));
    }

    pub(crate) fn register_package(&mut self, id: PackageId, pkg: Arc<dyn Package>) {
        self.packages.entry(id).or_insert(pkg);
    }

    // ── Query (used by Checker / Evaluator) ───────────────────────────

    pub fn module(&self, raw_id: &[String]) -> Option<&ModuleNode> {
        self.modules.get(raw_id)
    }

    pub fn modules(&self) -> impl Iterator<Item = &ModuleNode> {
        self.modules.values()
    }

    pub fn global(&self, raw_id: &[String], name: &str) -> Option<&GlobalNode> {
        self.globals.get(&(raw_id.to_vec(), name.to_string()))
    }

    pub fn globals(&self) -> impl Iterator<Item = &GlobalNode> {
        self.globals.values()
    }

    pub fn type_decl(&self, raw_id: &[String], name: &str) -> Option<&TypeDeclNode> {
        self.type_decls.get(&(raw_id.to_vec(), name.to_string()))
    }

    pub fn type_decls(&self) -> impl Iterator<Item = &TypeDeclNode> {
        self.type_decls.values()
    }

    pub fn global_exprs(&self) -> &[(RawModuleId, ast::ModStmt)] {
        &self.global_exprs
    }

    pub fn packages(&self) -> &HashMap<PackageId, Arc<dyn Package>> {
        &self.packages
    }

    pub fn package(&self, id: &PackageId) -> Option<&Arc<dyn Package>> {
        self.packages.get(id)
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    pub fn edges_from(&self, node: &NodeId) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(move |e| &e.from == node)
    }

    /// Returns true if this node has an edge to itself.
    pub fn has_self_edge(&self, node: &NodeId) -> bool {
        self.edges.iter().any(|e| &e.from == node && &e.to == node)
    }

    /// Compute SCCs using Tarjan's algorithm.
    /// Returns SCCs in reverse topological order (dependencies before dependents).
    pub fn compute_sccs(&self) -> Vec<Vec<NodeId>> {
        let all_nodes = self.all_node_ids();
        let n = all_nodes.len();
        if n == 0 {
            return Vec::new();
        }

        let node_index: HashMap<&NodeId, usize> =
            all_nodes.iter().enumerate().map(|(i, n)| (n, i)).collect();

        // Build adjacency list.
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for edge in &self.edges {
            if let (Some(&from), Some(&to)) = (node_index.get(&edge.from), node_index.get(&edge.to))
            {
                adj[from].push(to);
            }
        }

        let mut state = TarjanState {
            index_counter: 0,
            stack: Vec::new(),
            on_stack: vec![false; n],
            index: vec![None; n],
            lowlink: vec![0; n],
            result: Vec::new(),
            adj: &adj,
            nodes: &all_nodes,
        };

        for v in 0..n {
            if state.index[v].is_none() {
                state.strongconnect(v);
            }
        }

        state.result
    }

    /// Collect all node IDs in the ASG.
    fn all_node_ids(&self) -> Vec<NodeId> {
        let mut ids = Vec::new();
        for raw_id in self.modules.keys() {
            ids.push(NodeId::Module(raw_id.clone()));
        }
        for (raw_id, name) in self.globals.keys() {
            ids.push(NodeId::Global(raw_id.clone(), name.clone()));
        }
        for (raw_id, name) in self.type_decls.keys() {
            ids.push(NodeId::TypeDecl(raw_id.clone(), name.clone()));
        }
        ids
    }
}

struct TarjanState<'a> {
    index_counter: usize,
    stack: Vec<usize>,
    on_stack: Vec<bool>,
    index: Vec<Option<usize>>,
    lowlink: Vec<usize>,
    result: Vec<Vec<NodeId>>,
    adj: &'a [Vec<usize>],
    nodes: &'a [NodeId],
}

impl TarjanState<'_> {
    fn strongconnect(&mut self, v: usize) {
        self.index[v] = Some(self.index_counter);
        self.lowlink[v] = self.index_counter;
        self.index_counter += 1;
        self.stack.push(v);
        self.on_stack[v] = true;

        for &w in &self.adj[v] {
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
                component.push(self.nodes[w].clone());
                if w == v {
                    break;
                }
            }
            self.result.push(component);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_id(s: &str) -> RawModuleId {
        s.split('/').map(String::from).collect()
    }

    #[test]
    fn empty_asg_sccs() {
        let asg = Asg::new();
        assert!(asg.compute_sccs().is_empty());
    }

    #[test]
    fn simple_chain() {
        let mut asg = Asg::new();

        // Module A has global a, Module B has global b. a -> b.
        asg.add_global(GlobalNode {
            raw_module_id: raw_id("A"),
            name: "a".into(),
            span: Span::default(),
            stmt: ast::LetBind {
                doc_comment: None,
                var: crate::Loc::new(
                    ast::Var {
                        name: "a".into(),
                        cursor: None,
                    },
                    Span::default(),
                ),
                ty: None,
                expr: Box::new(crate::Loc::new(ast::Expr::Nil, Span::default())),
            },
            is_export: false,
        });
        asg.add_global(GlobalNode {
            raw_module_id: raw_id("B"),
            name: "b".into(),
            span: Span::default(),
            stmt: ast::LetBind {
                doc_comment: None,
                var: crate::Loc::new(
                    ast::Var {
                        name: "b".into(),
                        cursor: None,
                    },
                    Span::default(),
                ),
                ty: None,
                expr: Box::new(crate::Loc::new(ast::Expr::Nil, Span::default())),
            },
            is_export: false,
        });

        asg.add_edge(Edge {
            from: NodeId::Global(raw_id("A"), "a".into()),
            to: NodeId::Global(raw_id("B"), "b".into()),
            lazy: false,
            span: None,
        });

        let sccs = asg.compute_sccs();
        assert_eq!(sccs.len(), 2);
        // b should come first (dependency)
        assert_eq!(sccs[0], vec![NodeId::Global(raw_id("B"), "b".into())]);
        assert_eq!(sccs[1], vec![NodeId::Global(raw_id("A"), "a".into())]);
    }

    #[test]
    fn cycle_detected() {
        let mut asg = Asg::new();

        asg.add_global(GlobalNode {
            raw_module_id: raw_id("A"),
            name: "a".into(),
            span: Span::default(),
            stmt: ast::LetBind {
                doc_comment: None,
                var: crate::Loc::new(
                    ast::Var {
                        name: "a".into(),
                        cursor: None,
                    },
                    Span::default(),
                ),
                ty: None,
                expr: Box::new(crate::Loc::new(ast::Expr::Nil, Span::default())),
            },
            is_export: false,
        });
        asg.add_global(GlobalNode {
            raw_module_id: raw_id("A"),
            name: "b".into(),
            span: Span::default(),
            stmt: ast::LetBind {
                doc_comment: None,
                var: crate::Loc::new(
                    ast::Var {
                        name: "b".into(),
                        cursor: None,
                    },
                    Span::default(),
                ),
                ty: None,
                expr: Box::new(crate::Loc::new(ast::Expr::Nil, Span::default())),
            },
            is_export: false,
        });

        asg.add_edge(Edge {
            from: NodeId::Global(raw_id("A"), "a".into()),
            to: NodeId::Global(raw_id("A"), "b".into()),
            lazy: true,
            span: None,
        });
        asg.add_edge(Edge {
            from: NodeId::Global(raw_id("A"), "b".into()),
            to: NodeId::Global(raw_id("A"), "a".into()),
            lazy: true,
            span: None,
        });

        let sccs = asg.compute_sccs();
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 2);
    }
}
