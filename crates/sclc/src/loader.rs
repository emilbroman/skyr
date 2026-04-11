use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::ast::{self, Expr, ListItem, ModStmt, TypeExpr};
use crate::{DiagList, Diagnosed, Loc, ModuleId, PackageId, Span};

use super::asg::{Asg, Edge, GlobalNode, ModuleNode, NodeId, RawModuleId, TypeDeclNode};
use super::{LoadError, PackageFinder};

/// The Loader builds an [`Asg`] by spidering the import graph starting from
/// one or more entry points.
pub struct Loader {
    finder: Arc<dyn PackageFinder>,
    asg: Asg,
    diags: DiagList,
}

impl Loader {
    pub fn new(finder: Arc<dyn PackageFinder>) -> Self {
        Self {
            finder,
            asg: Asg::new(),
            diags: DiagList::new(),
        }
    }

    /// Resolve all transitive dependencies starting from the given raw module ID.
    /// Can be called multiple times to accumulate more of the graph.
    pub async fn resolve(&mut self, raw_id: &[&str]) -> Result<(), LoadError> {
        // Queue entries: (raw module ID, optional source info for diagnostics).
        let mut queue: VecDeque<(RawModuleId, Option<ImportSource>)> =
            VecDeque::from([(raw_id.iter().map(|s| s.to_string()).collect(), None)]);

        while let Some((raw_module_id, import_source)) = queue.pop_front() {
            if self.asg.has_module(&raw_module_id) {
                continue;
            }

            // Find the package for this raw module ID.
            let raw_strs: Vec<&str> = raw_module_id.iter().map(String::as_str).collect();
            let package = match self.finder.find(&raw_strs).await? {
                Some(pkg) => pkg,
                None => {
                    self.diags.push(LoaderDiag::PackageNotFound {
                        raw_id: raw_module_id.clone(),
                        import_source: import_source.clone(),
                    });
                    continue;
                }
            };

            let pkg_id = package.id();
            let pkg_len = pkg_id.len();

            // Construct the proper ModuleId.
            let module_path: Vec<String> = raw_module_id[pkg_len..].to_vec();
            let module_id = ModuleId::new(pkg_id.clone(), module_path);

            // Load and parse the module source.
            let scl_path = module_id.to_path_buf_with_extension("scl");
            let source_bytes = match package.load(&scl_path).await {
                Ok(data) => data.into_owned(),
                Err(LoadError::NotFound(_)) => {
                    self.diags.push(LoaderDiag::ModuleNotFound {
                        raw_id: raw_module_id.clone(),
                        module_id: module_id.clone(),
                        import_source,
                    });
                    continue;
                }
                Err(e) => return Err(e),
            };

            let source = String::from_utf8(source_bytes).map_err(LoadError::Encoding)?;

            let file_mod = {
                let mut parse_diags = DiagList::new();
                let parsed = crate::parse_file_mod(&source, &module_id);
                let file_mod = parsed.unpack(&mut parse_diags);
                self.diags.extend(parse_diags);
                file_mod
            };

            // Validate path expressions.
            self.validate_paths(&module_id, &file_mod, &*package).await;

            // Register the package.
            self.asg
                .register_package(pkg_id.clone(), Arc::clone(&package));

            // Analyze the module: collect globals, type decls, imports, and build edges.
            let analysis = analyze_module(&raw_module_id, &pkg_id, &module_id, &file_mod);

            // Add module node.
            self.asg.add_module(ModuleNode {
                raw_id: raw_module_id.clone(),
                module_id,
                file_mod,
                package_id: pkg_id,
            });

            // Add global nodes.
            for g in analysis.globals {
                self.asg.add_global(g);
            }

            // Add type declaration nodes.
            for td in analysis.type_decls {
                self.asg.add_type_decl(td);
            }

            // Add global expression statements.
            for stmt in analysis.global_exprs {
                self.asg.add_global_expr(raw_module_id.clone(), stmt);
            }

            // Add edges.
            for edge in analysis.edges {
                self.asg.add_edge(edge);
            }

            // Enqueue discovered imports.
            for (import_raw_id, import_src) in analysis.discovered_imports {
                if !self.asg.has_module(&import_raw_id) {
                    queue.push_back((import_raw_id, Some(import_src)));
                }
            }
        }

        Ok(())
    }

    /// Validate path expressions in a parsed module, emitting `InvalidPath`
    /// diagnostics for paths that don't exist in the package.
    async fn validate_paths(
        &mut self,
        module_id: &crate::ModuleId,
        file_mod: &ast::FileMod,
        package: &dyn super::Package,
    ) {
        let mut collector = crate::CollectPaths::default();
        crate::visit_file_mod(&mut collector, file_mod);

        for (path_expr, span) in collector.paths {
            let resolved = path_expr.resolve_with_context(module_id);
            let rel = resolved.strip_prefix('/').unwrap_or(&resolved);
            if rel.is_empty() {
                continue;
            }

            let components: Vec<&str> = rel.split('/').collect();
            let mut valid = true;

            for i in 1..=components.len() {
                let prefix = components[..i].join("/");
                let prefix_path = std::path::Path::new(&prefix);
                match package.lookup(prefix_path).await {
                    Ok(None) => {
                        valid = false;
                        break;
                    }
                    Ok(Some(entity)) => {
                        if i < components.len() {
                            // Intermediate component must be a directory.
                            if matches!(entity.as_ref(), super::PackageEntity::File { .. }) {
                                valid = false;
                                break;
                            }
                        }
                    }
                    Err(_) => break, // I/O error — don't emit a false positive.
                }
            }

            if !valid {
                self.diags.push(crate::InvalidPath {
                    module_id: module_id.clone(),
                    resolved_path: resolved,
                    span,
                });
            }
        }
    }

    /// Finalize the Loader, returning the ASG with accumulated diagnostics.
    pub fn finish(self) -> Diagnosed<Asg> {
        Diagnosed::new(self.asg, self.diags)
    }

    /// Finalize with SCC laziness validation: all intra-SCC edges between
    /// globals must be lazy (cross a function boundary).
    ///
    /// This is separate from `finish()` because when bridging to the existing
    /// checker (which has its own cycle detection), the validation would produce
    /// duplicate diagnostics.
    pub fn finish_with_validation(mut self) -> Diagnosed<Asg> {
        self.validate_scc_laziness();
        Diagnosed::new(self.asg, self.diags)
    }

    /// Check that all edges within an SCC are lazy.
    fn validate_scc_laziness(&mut self) {
        let sccs = self.asg.compute_sccs();
        for scc in &sccs {
            if scc.len() < 2 && !self.asg.has_self_edge(&scc[0]) {
                continue;
            }
            let scc_set: HashSet<&NodeId> = scc.iter().collect();
            for edge in self.asg.edges() {
                if scc_set.contains(&edge.from)
                    && scc_set.contains(&edge.to)
                    && !edge.lazy
                    // Module → Global/TypeDecl edges are structural, not value references.
                    && !matches!(edge.from, NodeId::Module(_))
                {
                    self.diags.push(CyclicEagerDependency {
                        from: edge.from.clone(),
                        to: edge.to.clone(),
                        span: edge.span.unwrap_or_default(),
                    });
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Module analysis
// ═══════════════════════════════════════════════════════════════════════════════

/// Source location of an import statement, for diagnostics.
#[derive(Clone, Debug)]
struct ImportSource {
    source_module_id: ModuleId,
    path_span: Span,
}

struct ModuleAnalysis {
    globals: Vec<GlobalNode>,
    type_decls: Vec<TypeDeclNode>,
    global_exprs: Vec<ModStmt>,
    edges: Vec<Edge>,
    discovered_imports: Vec<(RawModuleId, ImportSource)>,
}

/// Analyze a parsed module, extracting nodes, edges, and import references.
fn analyze_module(
    raw_id: &RawModuleId,
    pkg_id: &PackageId,
    module_id: &ModuleId,
    file_mod: &ast::FileMod,
) -> ModuleAnalysis {
    // Collect sets of names for classification.
    let mut global_names: HashSet<String> = HashSet::new();
    let mut type_names: HashSet<String> = HashSet::new();
    // Maps import alias (last segment) → raw module ID of the import target.
    let mut import_aliases: HashMap<String, RawModuleId> = HashMap::new();

    // First pass: collect names.
    for stmt in &file_mod.statements {
        match stmt {
            ModStmt::Let(bind) | ModStmt::Export(bind) => {
                global_names.insert(bind.var.name.clone());
            }
            ModStmt::TypeDef(td) | ModStmt::ExportTypeDef(td) => {
                type_names.insert(td.var.name.clone());
            }
            ModStmt::Import(import) => {
                let vars = &import.as_ref().vars;
                if !vars.is_empty() {
                    let alias = vars.last().unwrap().name.clone();
                    let import_raw_id = resolve_import_path(vars, pkg_id);
                    import_aliases.insert(alias, import_raw_id);
                }
            }
            ModStmt::Expr(_) => {}
        }
    }

    let mut analysis = ModuleAnalysis {
        globals: Vec::new(),
        type_decls: Vec::new(),
        global_exprs: Vec::new(),
        edges: Vec::new(),
        discovered_imports: Vec::new(),
    };

    // Collect discovered imports (deduplicated).
    let mut seen_imports: HashSet<RawModuleId> = HashSet::new();
    // Also collect import spans for diagnostics.
    let mut import_spans: HashMap<RawModuleId, Span> = HashMap::new();
    for stmt in &file_mod.statements {
        if let ModStmt::Import(import) = stmt {
            let vars = &import.as_ref().vars;
            if !vars.is_empty() {
                let import_raw_id = resolve_import_path(vars, pkg_id);
                let path_span = Span::new(
                    vars.first().unwrap().span().start(),
                    vars.last().unwrap().span().end(),
                );
                import_spans.entry(import_raw_id).or_insert(path_span);
            }
        }
    }

    for import_raw_id in import_aliases.values() {
        if seen_imports.insert(import_raw_id.clone()) {
            let span = import_spans.get(import_raw_id).copied().unwrap_or_default();
            analysis.discovered_imports.push((
                import_raw_id.clone(),
                ImportSource {
                    source_module_id: module_id.clone(),
                    path_span: span,
                },
            ));
            // Module → Import module edge.
            analysis.edges.push(Edge {
                from: NodeId::Module(raw_id.clone()),
                to: NodeId::Module(import_raw_id.clone()),
                lazy: false,
                span: None,
            });
        }
    }

    let ctx = RefContext {
        raw_id,
        global_names: &global_names,
        type_names: &type_names,
        import_aliases: &import_aliases,
    };

    // Second pass: build nodes and edges.
    for stmt in &file_mod.statements {
        match stmt {
            ModStmt::Let(bind) | ModStmt::Export(bind) => {
                let is_export = matches!(stmt, ModStmt::Export(_));
                let name = bind.var.name.clone();
                let from = NodeId::Global(raw_id.clone(), name.clone());

                // Module → Global edge.
                analysis.edges.push(Edge {
                    from: NodeId::Module(raw_id.clone()),
                    to: from.clone(),
                    lazy: false,
                    span: None,
                });

                // Traverse the expression body for references.
                let mut refs = Vec::new();
                collect_expr_refs(&ctx, bind.expr.as_ref().as_ref(), false, &mut refs);

                // Also traverse the type annotation, if any.
                if let Some(ty) = &bind.ty {
                    collect_type_refs(&ctx, ty.as_ref(), &mut refs);
                }

                for r in refs {
                    analysis.edges.push(Edge {
                        from: from.clone(),
                        to: r.target,
                        lazy: r.lazy,
                        span: Some(r.span),
                    });
                }

                analysis.globals.push(GlobalNode {
                    raw_module_id: raw_id.clone(),
                    name,
                    span: bind.var.span(),
                    stmt: bind.clone(),
                    is_export,
                });
            }
            ModStmt::TypeDef(td) | ModStmt::ExportTypeDef(td) => {
                let is_export = matches!(stmt, ModStmt::ExportTypeDef(_));
                let name = td.var.name.clone();
                let from = NodeId::TypeDecl(raw_id.clone(), name.clone());

                // Module → TypeDecl edge.
                analysis.edges.push(Edge {
                    from: NodeId::Module(raw_id.clone()),
                    to: from.clone(),
                    lazy: false,
                    span: None,
                });

                // Traverse the type body for type references.
                let mut refs = Vec::new();
                collect_type_refs(&ctx, td.ty.as_ref(), &mut refs);

                // Subtract own type params.
                let own_params: HashSet<&str> = td
                    .type_params
                    .iter()
                    .map(|tp| tp.var.name.as_str())
                    .collect();

                for r in refs {
                    if let NodeId::TypeDecl(_, ref tname) = r.target
                        && own_params.contains(tname.as_str())
                    {
                        continue;
                    }
                    analysis.edges.push(Edge {
                        from: from.clone(),
                        to: r.target,
                        lazy: false,
                        span: Some(r.span),
                    });
                }

                // Also traverse type param bounds.
                for tp in &td.type_params {
                    if let Some(bound) = &tp.bound {
                        let mut bound_refs = Vec::new();
                        collect_type_refs(&ctx, bound.as_ref(), &mut bound_refs);
                        for r in bound_refs {
                            analysis.edges.push(Edge {
                                from: from.clone(),
                                to: r.target,
                                lazy: false,
                                span: Some(r.span),
                            });
                        }
                    }
                }

                analysis.type_decls.push(TypeDeclNode {
                    raw_module_id: raw_id.clone(),
                    name,
                    type_def: td.clone(),
                    is_export,
                });
            }
            ModStmt::Expr(_) => {
                analysis.global_exprs.push(stmt.clone());
            }
            ModStmt::Import(_) => {
                // Already handled above.
            }
        }
    }

    analysis
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

// ═══════════════════════════════════════════════════════════════════════════════
// Reference collection
// ═══════════════════════════════════════════════════════════════════════════════

/// Context needed for reference resolution.
struct RefContext<'a> {
    raw_id: &'a RawModuleId,
    global_names: &'a HashSet<String>,
    type_names: &'a HashSet<String>,
    import_aliases: &'a HashMap<String, RawModuleId>,
}

/// A discovered reference from an expression or type.
struct Ref {
    target: NodeId,
    lazy: bool,
    span: Span,
}

/// Collect value/module references from an expression.
///
/// `in_fn` tracks whether we're inside a function body (for laziness).
fn collect_expr_refs(ctx: &RefContext, expr: &Expr, in_fn: bool, out: &mut Vec<Ref>) {
    collect_expr_refs_with_scope(ctx, expr, in_fn, &mut HashSet::new(), out);
}

fn collect_expr_refs_with_scope(
    ctx: &RefContext,
    expr: &Expr,
    in_fn: bool,
    locals: &mut HashSet<String>,
    out: &mut Vec<Ref>,
) {
    match expr {
        Expr::Var(var) => {
            let name = &var.name;
            if locals.contains(name.as_str()) {
                return;
            }
            if ctx.global_names.contains(name.as_str()) {
                out.push(Ref {
                    target: NodeId::Global(ctx.raw_id.clone(), name.clone()),
                    lazy: in_fn,
                    span: var.span(),
                });
            } else if ctx.import_aliases.contains_key(name.as_str()) {
                let import_id = &ctx.import_aliases[name.as_str()];
                out.push(Ref {
                    target: NodeId::Module(import_id.clone()),
                    lazy: in_fn,
                    span: var.span(),
                });
            }
        }

        Expr::PropertyAccess(pa) => {
            // Check for qualified reference: Import.member
            if let Expr::Var(var) = pa.expr.as_ref().as_ref() {
                let name = &var.name;
                if !locals.contains(name.as_str())
                    && let Some(import_id) = ctx.import_aliases.get(name.as_str())
                {
                    // This is Import.member — add edge to the specific global.
                    out.push(Ref {
                        target: NodeId::Global(import_id.clone(), pa.property.name.clone()),
                        lazy: in_fn,
                        span: pa.property.span(),
                    });
                    // Don't recurse into the Var — we've handled it.
                    return;
                }
            }
            // Not a qualified reference — recurse normally.
            collect_expr_refs_with_scope(ctx, pa.expr.as_ref().as_ref(), in_fn, locals, out);
        }

        Expr::Fn(fn_expr) => {
            // Function body references are lazy.
            let mut inner_locals = locals.clone();
            for param in &fn_expr.params {
                inner_locals.insert(param.var.name.clone());
            }
            if let Some(body) = &fn_expr.body {
                collect_expr_refs_with_scope(
                    ctx,
                    body.as_ref().as_ref(),
                    true,
                    &mut inner_locals,
                    out,
                );
            }
            // Type annotations on params are NOT lazy.
            for param in &fn_expr.params {
                if let Some(ty) = &param.ty {
                    collect_type_refs(ctx, ty.as_ref(), out);
                }
            }
            // Type param bounds.
            for tp in &fn_expr.type_params {
                if let Some(bound) = &tp.bound {
                    collect_type_refs(ctx, bound.as_ref(), out);
                }
            }
        }

        Expr::Let(let_expr) => {
            // The binding expression is in current scope.
            collect_expr_refs_with_scope(
                ctx,
                let_expr.bind.expr.as_ref().as_ref(),
                in_fn,
                locals,
                out,
            );
            // Type annotation on the let bind.
            if let Some(ty) = &let_expr.bind.ty {
                collect_type_refs(ctx, ty.as_ref(), out);
            }
            // The body gets the new local binding.
            if let Some(body) = &let_expr.expr {
                let mut inner_locals = locals.clone();
                inner_locals.insert(let_expr.bind.var.name.clone());
                collect_expr_refs_with_scope(
                    ctx,
                    body.as_ref().as_ref(),
                    in_fn,
                    &mut inner_locals,
                    out,
                );
            }
        }

        Expr::If(if_expr) => {
            collect_expr_refs_with_scope(
                ctx,
                if_expr.condition.as_ref().as_ref(),
                in_fn,
                locals,
                out,
            );
            collect_expr_refs_with_scope(
                ctx,
                if_expr.then_expr.as_ref().as_ref(),
                in_fn,
                locals,
                out,
            );
            if let Some(else_expr) = &if_expr.else_expr {
                collect_expr_refs_with_scope(ctx, else_expr.as_ref().as_ref(), in_fn, locals, out);
            }
        }

        Expr::Call(call) => {
            collect_expr_refs_with_scope(ctx, call.callee.as_ref().as_ref(), in_fn, locals, out);
            for arg in &call.args {
                collect_expr_refs_with_scope(ctx, arg.as_ref(), in_fn, locals, out);
            }
            for ty_arg in &call.type_args {
                collect_type_refs(ctx, ty_arg.as_ref(), out);
            }
        }

        Expr::Binary(bin) => {
            collect_expr_refs_with_scope(ctx, bin.lhs.as_ref().as_ref(), in_fn, locals, out);
            collect_expr_refs_with_scope(ctx, bin.rhs.as_ref().as_ref(), in_fn, locals, out);
        }

        Expr::Unary(un) => {
            collect_expr_refs_with_scope(ctx, un.expr.as_ref().as_ref(), in_fn, locals, out);
        }

        Expr::Record(rec) => {
            for field in &rec.fields {
                collect_expr_refs_with_scope(ctx, &field.expr, in_fn, locals, out);
            }
        }

        Expr::Dict(dict) => {
            for entry in &dict.entries {
                collect_expr_refs_with_scope(ctx, &entry.key, in_fn, locals, out);
                collect_expr_refs_with_scope(ctx, &entry.value, in_fn, locals, out);
            }
        }

        Expr::List(list) => {
            for item in &list.items {
                collect_list_item_refs(ctx, item, in_fn, locals, out);
            }
        }

        Expr::IndexedAccess(ia) => {
            collect_expr_refs_with_scope(ctx, ia.expr.as_ref().as_ref(), in_fn, locals, out);
            collect_expr_refs_with_scope(ctx, ia.index.as_ref().as_ref(), in_fn, locals, out);
        }

        Expr::TypeCast(tc) => {
            collect_expr_refs_with_scope(ctx, tc.expr.as_ref().as_ref(), in_fn, locals, out);
            collect_type_refs(ctx, tc.ty.as_ref(), out);
        }

        Expr::Interp(interp) => {
            for part in &interp.parts {
                collect_expr_refs_with_scope(ctx, part.as_ref(), in_fn, locals, out);
            }
        }

        Expr::Raise(raise) => {
            collect_expr_refs_with_scope(ctx, raise.expr.as_ref().as_ref(), in_fn, locals, out);
        }

        Expr::Try(try_expr) => {
            collect_expr_refs_with_scope(ctx, try_expr.expr.as_ref().as_ref(), in_fn, locals, out);
            for catch in &try_expr.catches {
                // exception_var is a reference to an exception name (global-ish).
                // But per free_vars() logic, exception_var is _inserted_ into the
                // free vars set (it's the exception being caught, not a local binding).
                // For the dependency graph, we treat it as a reference.
                let exc_name = &catch.exception_var.name;
                if !locals.contains(exc_name.as_str())
                    && ctx.global_names.contains(exc_name.as_str())
                {
                    out.push(Ref {
                        target: NodeId::Global(ctx.raw_id.clone(), exc_name.clone()),
                        lazy: in_fn,
                        span: catch.exception_var.span(),
                    });
                }

                let mut catch_locals = locals.clone();
                if let Some(arg) = &catch.catch_arg {
                    catch_locals.insert(arg.name.clone());
                }
                collect_expr_refs_with_scope(ctx, &catch.body, in_fn, &mut catch_locals, out);
            }
        }

        Expr::Extern(ext) => {
            collect_type_refs(ctx, ext.ty.as_ref(), out);
        }

        Expr::Exception(exc) => {
            if let Some(ty) = &exc.ty {
                collect_type_refs(ctx, ty.as_ref(), out);
            }
        }

        // Literals — no references.
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Nil
        | Expr::Str(_)
        | Expr::Path(_) => {}
    }
}

fn collect_list_item_refs(
    ctx: &RefContext,
    item: &ListItem,
    in_fn: bool,
    locals: &mut HashSet<String>,
    out: &mut Vec<Ref>,
) {
    match item {
        ListItem::Expr(expr) => {
            collect_expr_refs_with_scope(ctx, expr.as_ref(), in_fn, locals, out);
        }
        ListItem::If(if_item) => {
            collect_expr_refs_with_scope(
                ctx,
                if_item.condition.as_ref().as_ref(),
                in_fn,
                locals,
                out,
            );
            collect_list_item_refs(ctx, &if_item.then_item, in_fn, locals, out);
        }
        ListItem::For(for_item) => {
            collect_expr_refs_with_scope(
                ctx,
                for_item.iterable.as_ref().as_ref(),
                in_fn,
                locals,
                out,
            );
            let mut inner_locals = locals.clone();
            inner_locals.insert(for_item.var.name.clone());
            collect_list_item_refs(ctx, &for_item.emit_item, in_fn, &mut inner_locals, out);
        }
    }
}

/// Collect type references from a type expression.
fn collect_type_refs(ctx: &RefContext, ty: &TypeExpr, out: &mut Vec<Ref>) {
    match ty {
        TypeExpr::Var(var) => {
            let name = &var.name;
            // Check if it's a same-module type declaration.
            if ctx.type_names.contains(name.as_str()) {
                out.push(Ref {
                    target: NodeId::TypeDecl(ctx.raw_id.clone(), name.clone()),
                    lazy: false,
                    span: var.span(),
                });
            }
        }
        TypeExpr::PropertyAccess(pa) => {
            // Qualified type reference: Import.Type
            if let TypeExpr::Var(var) = pa.expr.as_ref().as_ref()
                && let Some(import_id) = ctx.import_aliases.get(var.name.as_str())
            {
                out.push(Ref {
                    target: NodeId::TypeDecl(import_id.clone(), pa.property.name.clone()),
                    lazy: false,
                    span: pa.property.span(),
                });
                return;
            }
            collect_type_refs(ctx, pa.expr.as_ref().as_ref(), out);
        }
        TypeExpr::Optional(inner) => collect_type_refs(ctx, inner.as_ref().as_ref(), out),
        TypeExpr::List(inner) => collect_type_refs(ctx, inner.as_ref().as_ref(), out),
        TypeExpr::Fn(f) => {
            let own_params: HashSet<&str> = f
                .type_params
                .iter()
                .map(|tp| tp.var.name.as_str())
                .collect();
            for param in &f.params {
                collect_type_refs_excluding(ctx, param.as_ref(), &own_params, out);
            }
            collect_type_refs_excluding(ctx, f.ret.as_ref().as_ref(), &own_params, out);
            for tp in &f.type_params {
                if let Some(bound) = &tp.bound {
                    collect_type_refs(ctx, bound.as_ref(), out);
                }
            }
        }
        TypeExpr::Record(rec) => {
            for field in &rec.fields {
                collect_type_refs(ctx, field.ty.as_ref(), out);
            }
        }
        TypeExpr::Dict(dict) => {
            collect_type_refs(ctx, dict.key.as_ref().as_ref(), out);
            collect_type_refs(ctx, dict.value.as_ref().as_ref(), out);
        }
        TypeExpr::Application(app) => {
            collect_type_refs(ctx, app.base.as_ref().as_ref(), out);
            for arg in &app.args {
                collect_type_refs(ctx, arg.as_ref(), out);
            }
        }
    }
}

/// Like [`collect_type_refs`] but excludes names in `exclude` (for type params).
fn collect_type_refs_excluding(
    ctx: &RefContext,
    ty: &TypeExpr,
    exclude: &HashSet<&str>,
    out: &mut Vec<Ref>,
) {
    match ty {
        TypeExpr::Var(var) if exclude.contains(var.name.as_str()) => {}
        _ => collect_type_refs(ctx, ty, out),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Diagnostics
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug)]
enum LoaderDiag {
    PackageNotFound {
        raw_id: RawModuleId,
        import_source: Option<ImportSource>,
    },
    ModuleNotFound {
        raw_id: RawModuleId,
        #[allow(dead_code)]
        module_id: ModuleId,
        import_source: Option<ImportSource>,
    },
}

impl LoaderDiag {
    fn import_source(&self) -> Option<&ImportSource> {
        match self {
            LoaderDiag::PackageNotFound { import_source, .. }
            | LoaderDiag::ModuleNotFound { import_source, .. } => import_source.as_ref(),
        }
    }
}

impl std::fmt::Display for LoaderDiag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoaderDiag::PackageNotFound { raw_id, .. } => {
                write!(f, "module not found: {}", raw_id.join("/"))
            }
            LoaderDiag::ModuleNotFound { raw_id, .. } => {
                write!(f, "module not found: {}", raw_id.join("/"))
            }
        }
    }
}

impl std::error::Error for LoaderDiag {}

impl crate::Diag for LoaderDiag {
    fn locate(&self) -> (ModuleId, Span) {
        if let Some(src) = self.import_source() {
            (src.source_module_id.clone(), src.path_span)
        } else {
            (ModuleId::default(), Span::default())
        }
    }
}

/// Diagnostic for an eager (non-lazy) edge within a strongly connected component.
#[derive(Debug)]
struct CyclicEagerDependency {
    from: NodeId,
    to: NodeId,
    span: Span,
}

impl std::fmt::Display for CyclicEagerDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cyclic dependency between `{}` and `{}` requires a function boundary",
            self.from, self.to
        )
    }
}

impl std::error::Error for CyclicEagerDependency {}

impl crate::Diag for CyclicEagerDependency {
    fn locate(&self) -> (ModuleId, Span) {
        (ModuleId::default(), self.span)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::CompositePackageFinder;
    use crate::{InMemoryPackage, StdPackage};

    use super::*;

    fn make_finder(files: HashMap<PathBuf, Vec<u8>>, pkg_id: PackageId) -> Arc<dyn PackageFinder> {
        let user_pkg = Arc::new(InMemoryPackage::new(pkg_id, files));
        let std_pkg = Arc::new(StdPackage::new());

        // Wrap each in a PackageFinder.
        struct PkgFinder(Arc<dyn super::super::Package>);

        #[async_trait::async_trait]
        impl PackageFinder for PkgFinder {
            async fn find(
                &self,
                raw_id: &[&str],
            ) -> Result<Option<Arc<dyn super::super::Package>>, LoadError> {
                let pkg_id = self.0.id();
                let segments = pkg_id.as_slice();
                if raw_id.len() >= segments.len()
                    && raw_id[..segments.len()]
                        .iter()
                        .zip(segments.iter())
                        .all(|(a, b)| *a == b.as_str())
                {
                    Ok(Some(Arc::clone(&self.0)))
                } else {
                    Ok(None)
                }
            }
        }

        Arc::new(CompositePackageFinder::new(vec![
            Arc::new(PkgFinder(user_pkg)),
            Arc::new(PkgFinder(std_pkg)),
        ]))
    }

    #[tokio::test]
    async fn single_module_no_imports() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"let x = 1\nlet y = x".to_vec());

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish();

        assert!(!result.diags().has_errors());
        let asg = result.into_inner();
        assert!(
            asg.module(&["Test".to_string(), "Main".to_string()])
                .is_some()
        );
        assert!(
            asg.global(&["Test".to_string(), "Main".to_string()], "x")
                .is_some()
        );
        assert!(
            asg.global(&["Test".to_string(), "Main".to_string()], "y")
                .is_some()
        );

        // y depends on x.
        let edges: Vec<_> = asg
            .edges_from(&NodeId::Global(
                vec!["Test".into(), "Main".into()],
                "y".into(),
            ))
            .collect();
        assert!(
            edges
                .iter()
                .any(|e| e.to == NodeId::Global(vec!["Test".into(), "Main".into()], "x".into()))
        );
    }

    #[tokio::test]
    async fn module_with_import() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nlet x = Lib.foo".to_vec(),
        );
        files.insert(PathBuf::from("Lib.scl"), b"export let foo = 42".to_vec());

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish();

        assert!(!result.diags().has_errors());
        let asg = result.into_inner();
        assert!(
            asg.module(&["Test".to_string(), "Main".to_string()])
                .is_some()
        );
        assert!(
            asg.module(&["Test".to_string(), "Lib".to_string()])
                .is_some()
        );
    }

    #[tokio::test]
    async fn self_import() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Self/Lib\nlet x = Lib.foo".to_vec(),
        );
        files.insert(PathBuf::from("Lib.scl"), b"export let foo = 42".to_vec());

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish();

        assert!(!result.diags().has_errors());
        let asg = result.into_inner();
        // Self/Lib should resolve to Test/Lib.
        assert!(
            asg.module(&["Test".to_string(), "Lib".to_string()])
                .is_some()
        );
    }

    #[tokio::test]
    async fn cyclic_imports_handled() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("A.scl"),
            b"import Test/B\nlet x = B.y".to_vec(),
        );
        files.insert(
            PathBuf::from("B.scl"),
            b"import Test/A\nlet y = A.x".to_vec(),
        );

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "A"]).await.unwrap();
        let result = loader.finish();

        let asg = result.into_inner();
        assert!(asg.module(&["Test".to_string(), "A".to_string()]).is_some());
        assert!(asg.module(&["Test".to_string(), "B".to_string()]).is_some());
    }

    #[tokio::test]
    async fn missing_import_produces_diagnostic() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"import NonExistent/Pkg".to_vec(),
        );

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish();

        assert!(result.diags().has_errors());
    }

    #[tokio::test]
    async fn lazy_edge_inside_fn_body() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"let a = () => b\nlet b = () => a".to_vec(),
        );

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish();
        let asg = result.into_inner();

        // Both edges should be lazy (inside fn bodies).
        let a_edges: Vec<_> = asg
            .edges_from(&NodeId::Global(
                vec!["Test".into(), "Main".into()],
                "a".into(),
            ))
            .collect();
        assert!(a_edges.iter().all(|e| e.lazy));

        let b_edges: Vec<_> = asg
            .edges_from(&NodeId::Global(
                vec!["Test".into(), "Main".into()],
                "b".into(),
            ))
            .collect();
        assert!(b_edges.iter().all(|e| e.lazy));
    }

    #[tokio::test]
    async fn eager_edge_outside_fn_body() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"let a = b\nlet b = 1".to_vec());

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish();
        let asg = result.into_inner();

        // a → b edge should be eager (not in a fn body).
        let a_edges: Vec<_> = asg
            .edges_from(&NodeId::Global(
                vec!["Test".into(), "Main".into()],
                "a".into(),
            ))
            .collect();
        assert!(
            a_edges.iter().any(|e| !e.lazy
                && e.to == NodeId::Global(vec!["Test".into(), "Main".into()], "b".into()))
        );
    }

    // ── SCC laziness validation tests ─────────────────────────────────

    #[tokio::test]
    async fn eager_cycle_produces_diagnostic() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"let a = b\nlet b = a".to_vec());

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish_with_validation();

        // Eager mutual reference should produce a diagnostic.
        assert!(result.diags().has_errors());
        let msgs: Vec<String> = result.diags().iter().map(|d| d.to_string()).collect();
        assert!(msgs.iter().any(|m| m.contains("cyclic dependency")));
    }

    #[tokio::test]
    async fn lazy_cycle_no_diagnostic() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"let a = () => b()\nlet b = () => a()".to_vec(),
        );

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish_with_validation();

        // All-lazy cycle should NOT produce a cyclic dependency diagnostic.
        let msgs: Vec<String> = result.diags().iter().map(|d| d.to_string()).collect();
        assert!(
            !msgs.iter().any(|m| m.contains("cyclic dependency")),
            "unexpected diagnostic: {msgs:?}"
        );
    }

    #[tokio::test]
    async fn self_referencing_eager_global_diagnostic() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"let a = a".to_vec());

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish_with_validation();

        assert!(result.diags().has_errors());
        let msgs: Vec<String> = result.diags().iter().map(|d| d.to_string()).collect();
        assert!(msgs.iter().any(|m| m.contains("cyclic dependency")));
    }

    #[tokio::test]
    async fn self_referencing_lazy_global_ok() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"let a = () => a()".to_vec());

        let finder = make_finder(files, PackageId::from(["Test"]));
        let mut loader = Loader::new(finder);
        loader.resolve(&["Test", "Main"]).await.unwrap();
        let result = loader.finish_with_validation();

        let msgs: Vec<String> = result.diags().iter().map(|d| d.to_string()).collect();
        assert!(
            !msgs.iter().any(|m| m.contains("cyclic dependency")),
            "unexpected diagnostic: {msgs:?}"
        );
    }
}
