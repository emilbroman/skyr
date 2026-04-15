use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::ast::{self, Expr, ListItem, ModStmt, TypeExpr};
use crate::{
    Cursor, CursorIdentifier, DiagList, Diagnosed, Loc, ModuleId, PackageId, Position, Span,
};

use super::asg::{
    Asg, Edge, GlobalNode, ModuleBody, ModuleNode, NodeId, RawModuleId, TypeDeclNode,
};
use super::{LoadError, PackageFinder};

/// The Loader builds an [`Asg`] by spidering the import graph starting from
/// one or more entry points.
pub struct Loader {
    finder: Arc<dyn PackageFinder>,
    asg: Asg,
    diags: DiagList,
    cursor_data: CursorData,
}

impl Loader {
    pub fn new(finder: Arc<dyn PackageFinder>) -> Self {
        Self {
            finder,
            asg: Asg::new(),
            diags: DiagList::new(),
            cursor_data: CursorData {
                cursor: None,
                pending_declarations: Vec::new(),
                pending_references: Vec::new(),
                resolved_references: Vec::new(),
            },
        }
    }

    /// Resolve all transitive dependencies starting from the given raw module ID.
    /// Can be called multiple times to accumulate more of the graph.
    pub async fn resolve(&mut self, raw_id: &[&str]) -> Result<(), LoadError> {
        // Queue entries: (raw module ID, optional source info for diagnostics).
        let mut queue: VecDeque<(RawModuleId, Option<ImportSource>)> =
            VecDeque::from([(raw_id.iter().map(|s| s.to_string()).collect(), None)]);

        self.drain_queue(&mut queue).await
    }

    async fn drain_queue(
        &mut self,
        queue: &mut VecDeque<(RawModuleId, Option<ImportSource>)>,
    ) -> Result<(), LoadError> {
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

            // Probe for both `.scl` and `.scle` in the package.
            let scl_path = module_id.to_path_buf_with_extension("scl");
            let scle_path = module_id.to_path_buf_with_extension("scle");
            let has_scl = matches!(package.lookup(&scl_path).await, Ok(Some(_)));
            let has_scle = matches!(package.lookup(&scle_path).await, Ok(Some(_)));

            match (has_scl, has_scle) {
                (false, false) => {
                    self.diags.push(LoaderDiag::ModuleNotFound {
                        raw_id: raw_module_id.clone(),
                        module_id: module_id.clone(),
                        import_source,
                    });
                    continue;
                }
                (true, true) => {
                    // Ambiguous: both extensions present. Fatal at entrypoint
                    // (no import_source), otherwise a diagnostic attributed to
                    // the import site.
                    if import_source.is_none() {
                        return Err(LoadError::Other(
                            format!(
                                "ambiguous module {}: both `.scl` and `.scle` exist",
                                raw_module_id.join("/")
                            )
                            .into(),
                        ));
                    }
                    self.diags.push(LoaderDiag::AmbiguousModule {
                        raw_id: raw_module_id.clone(),
                        import_source,
                    });
                    continue;
                }
                (true, false) => {
                    let source_bytes = match package.load(&scl_path).await {
                        Ok(data) => data.into_owned(),
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
                    self.process_module(
                        raw_module_id,
                        module_id,
                        pkg_id,
                        package,
                        ModuleBody::File(file_mod),
                        queue,
                    )
                    .await;
                }
                (false, true) => {
                    let source_bytes = match package.load(&scle_path).await {
                        Ok(data) => data.into_owned(),
                        Err(e) => return Err(e),
                    };
                    let source = String::from_utf8(source_bytes).map_err(LoadError::Encoding)?;
                    let scle_mod = {
                        let mut parse_diags = DiagList::new();
                        let parsed = crate::parse_scle(&source, &module_id);
                        let scle_mod = parsed.unpack(&mut parse_diags);
                        self.diags.extend(parse_diags);
                        match scle_mod {
                            Some(s) => s,
                            None => continue,
                        }
                    };
                    self.process_module(
                        raw_module_id,
                        module_id,
                        pkg_id,
                        package,
                        ModuleBody::Scle(scle_mod),
                        queue,
                    )
                    .await;
                }
            }
        }

        Ok(())
    }

    async fn process_module(
        &mut self,
        raw_module_id: RawModuleId,
        module_id: ModuleId,
        pkg_id: PackageId,
        package: Arc<dyn super::Package>,
        body: ModuleBody,
        queue: &mut VecDeque<(RawModuleId, Option<ImportSource>)>,
    ) {
        // Validate path expressions (only `.scl` modules have statement-level
        // path expressions worth validating).
        if let Some(file_mod) = body.as_file_mod() {
            self.validate_paths(&module_id, file_mod, &*package).await;
        }

        // Register the package.
        self.asg
            .register_package(pkg_id.clone(), Arc::clone(&package));

        // Analyze the module: collect globals, type decls, imports, and build edges.
        let analysis = match &body {
            ModuleBody::File(file_mod) => analyze_module(
                &raw_module_id,
                &pkg_id,
                &module_id,
                file_mod,
                &mut self.cursor_data,
            ),
            ModuleBody::Scle(scle_mod) => analyze_scle_module(
                &raw_module_id,
                &pkg_id,
                &module_id,
                scle_mod,
                &mut self.cursor_data,
            ),
        };

        // Add module node.
        self.asg.add_module(ModuleNode {
            raw_id: raw_module_id.clone(),
            module_id,
            body,
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
    pub fn finish(mut self) -> Diagnosed<Asg> {
        self.asg.rewrite_dangling_global_edges();
        resolve_pending_cursor_data(&self.asg, &self.cursor_data);
        Diagnosed::new(self.asg, self.diags)
    }

    /// Finalize with SCC laziness validation: all intra-SCC edges between
    /// globals must be lazy (cross a function boundary).
    ///
    /// This is separate from `finish()` because when bridging to the existing
    /// checker (which has its own cycle detection), the validation would produce
    /// duplicate diagnostics.
    pub fn finish_with_validation(mut self) -> Diagnosed<Asg> {
        self.asg.rewrite_dangling_global_edges();
        self.validate_scc_laziness();
        resolve_pending_cursor_data(&self.asg, &self.cursor_data);
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

/// Collect import aliases and resolve discovered imports into edges.
///
/// Shared between `analyze_module` and `analyze_scle_module` to avoid
/// duplicating the alias-collection, span-tracking, and edge-building logic.
fn process_imports(
    imports: &[Loc<ast::ImportStmt>],
    raw_id: &RawModuleId,
    pkg_id: &PackageId,
    module_id: &ModuleId,
    analysis: &mut ModuleAnalysis,
) -> HashMap<String, RawModuleId> {
    let mut import_aliases: HashMap<String, RawModuleId> = HashMap::new();
    let mut import_spans: HashMap<RawModuleId, Span> = HashMap::new();

    for import in imports {
        let vars = &import.as_ref().vars;
        if !vars.is_empty() {
            let alias = vars.last().unwrap().name.clone();
            let import_raw_id = resolve_import_path(vars, pkg_id);
            let path_span = Span::new(
                vars.first().unwrap().span().start(),
                vars.last().unwrap().span().end(),
            );
            import_spans
                .entry(import_raw_id.clone())
                .or_insert(path_span);
            import_aliases.insert(alias, import_raw_id);
        }
    }

    let mut seen: HashSet<RawModuleId> = HashSet::new();
    for import_raw_id in import_aliases.values() {
        if seen.insert(import_raw_id.clone()) {
            let span = import_spans.get(import_raw_id).copied().unwrap_or_default();
            analysis.discovered_imports.push((
                import_raw_id.clone(),
                ImportSource {
                    source_module_id: module_id.clone(),
                    path_span: span,
                },
            ));
            analysis.edges.push(Edge {
                from: NodeId::Module(raw_id.clone()),
                to: NodeId::Module(import_raw_id.clone()),
                lazy: false,
                span: None,
            });
        }
    }

    import_aliases
}

/// Analyze a parsed module, extracting nodes, edges, and import references.
fn analyze_module(
    raw_id: &RawModuleId,
    pkg_id: &PackageId,
    module_id: &ModuleId,
    file_mod: &ast::FileMod,
    cd: &mut CursorData,
) -> ModuleAnalysis {
    // Collect sets of names for classification.
    let mut global_names: HashSet<String> = HashSet::new();
    let mut type_names: HashSet<String> = HashSet::new();

    // First pass: collect names.
    for stmt in &file_mod.statements {
        match stmt {
            ModStmt::Let(bind) | ModStmt::Export(bind) => {
                global_names.insert(bind.var.name.clone());
            }
            ModStmt::TypeDef(td) | ModStmt::ExportTypeDef(td) => {
                type_names.insert(td.var.name.clone());
            }
            _ => {}
        }
    }

    let mut analysis = ModuleAnalysis {
        globals: Vec::new(),
        type_decls: Vec::new(),
        global_exprs: Vec::new(),
        edges: Vec::new(),
        discovered_imports: Vec::new(),
    };

    // Collect and resolve imports.
    let imports: Vec<_> = file_mod
        .statements
        .iter()
        .filter_map(|stmt| match stmt {
            ModStmt::Import(import) => Some(import.clone()),
            _ => None,
        })
        .collect();
    let import_aliases = process_imports(&imports, raw_id, pkg_id, module_id, &mut analysis);

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

                // Track cursor on the global binding declaration.
                if let Some((cursor, _)) = &bind.var.cursor {
                    cd.cursor = Some(cursor.clone());
                    cursor.set_declaration(raw_id.clone(), bind.var.span());
                    cursor.set_identifier(CursorIdentifier::Let(bind.var.name.clone()));
                }

                // Traverse the expression body for references.
                let mut refs = Vec::new();
                collect_expr_refs(&ctx, bind.expr.as_ref().as_ref(), false, &mut refs, cd);

                // Also traverse the type annotation, if any.
                if let Some(ty) = &bind.ty {
                    collect_type_refs(&ctx, ty.as_ref(), &mut refs, cd);
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

                // Track cursor on the type declaration.
                if let Some((cursor, _)) = &td.var.cursor {
                    cd.cursor = Some(cursor.clone());
                    cursor.set_declaration(raw_id.clone(), td.var.span());
                    cursor.set_identifier(CursorIdentifier::Type(td.var.name.clone()));
                }

                // Traverse the type body for type references.
                let mut refs = Vec::new();
                collect_type_refs(&ctx, td.ty.as_ref(), &mut refs, cd);

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
                        collect_type_refs(&ctx, bound.as_ref(), &mut bound_refs, cd);
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
            ModStmt::Expr(expr) => {
                collect_expr_refs(&ctx, expr.as_ref(), false, &mut Vec::new(), cd);
                analysis.global_exprs.push(stmt.clone());
            }
            ModStmt::Import(import) => {
                // Edge handling already done above. Track cursor on the
                // import alias (last path segment).
                let vars = &import.as_ref().vars;
                if !vars.is_empty() {
                    let alias_var = vars.last().unwrap();
                    let import_raw_id = resolve_import_path(vars, pkg_id);
                    if let Some((cursor, _)) = &alias_var.cursor {
                        cd.cursor = Some(cursor.clone());
                        cd.pending_declarations.push(PendingCursorDeclaration {
                            cursor: cursor.clone(),
                            target: PendingTarget::ImportAlias {
                                module: import_raw_id,
                            },
                        });
                    }
                }
            }
        }
    }

    analysis
}

/// Analyze a parsed SCLE module, extracting import references and edges. SCLE
/// modules contribute no globals or type declarations — just the `type_expr`
/// and `body` expression, whose references are attached to the module node.
fn analyze_scle_module(
    raw_id: &RawModuleId,
    pkg_id: &PackageId,
    module_id: &ModuleId,
    scle_mod: &ast::ScleMod,
    cd: &mut CursorData,
) -> ModuleAnalysis {
    // SCLE has no same-module globals or type names.
    let global_names: HashSet<String> = HashSet::new();
    let type_names: HashSet<String> = HashSet::new();

    let mut analysis = ModuleAnalysis {
        globals: Vec::new(),
        type_decls: Vec::new(),
        global_exprs: Vec::new(),
        edges: Vec::new(),
        discovered_imports: Vec::new(),
    };

    let import_aliases =
        process_imports(&scle_mod.imports, raw_id, pkg_id, module_id, &mut analysis);

    let ctx = RefContext {
        raw_id,
        global_names: &global_names,
        type_names: &type_names,
        import_aliases: &import_aliases,
    };

    // Collect references from the type expression and the body expression
    // (either may be absent — SCLE parts are optional).
    let mut refs = Vec::new();
    if let Some(type_expr) = &scle_mod.type_expr {
        collect_type_refs(&ctx, type_expr.as_ref(), &mut refs, cd);
    }
    if let Some(body) = &scle_mod.body {
        collect_expr_refs(&ctx, body.as_ref(), false, &mut refs, cd);
    }

    let from = NodeId::Module(raw_id.clone());
    for r in refs {
        analysis.edges.push(Edge {
            from: from.clone(),
            to: r.target,
            lazy: r.lazy,
            span: Some(r.span),
        });
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

// ═══════════════════════════════════════════════════════════════════════════════
// Cursor tracking
// ═══════════════════════════════════════════════════════════════════════════════

/// The target of a pending cursor declaration or reference, resolved at
/// finalization when all modules have been loaded.
#[derive(Clone, Debug)]
enum PendingTarget {
    /// A global value binding — resolves to `GlobalNode.span`.
    Global { module: RawModuleId, name: String },
    /// A type declaration — resolves to `TypeDeclNode.type_def.var.span()`.
    TypeDecl { module: RawModuleId, name: String },
    /// An import alias — resolves to `1:1–1:1` in the target module.
    ImportAlias { module: RawModuleId },
}

/// A cursor found on a Var node whose declaration target must be resolved
/// after all modules are loaded.
struct PendingCursorDeclaration {
    cursor: Cursor,
    target: PendingTarget,
}

/// A reference to a declaration, for "find all references". The target is
/// resolved to a span at finalization.
struct PendingReference {
    target: PendingTarget,
    reference_module: RawModuleId,
    reference_span: Span,
}

/// A local variable reference whose declaration span is already known.
struct ResolvedReference {
    declaration: (RawModuleId, Span),
    reference: (RawModuleId, Span),
}

/// All cursor-related data collected during a module analysis walk.
struct CursorData {
    cursor: Option<Cursor>,
    pending_declarations: Vec<PendingCursorDeclaration>,
    pending_references: Vec<PendingReference>,
    resolved_references: Vec<ResolvedReference>,
}

/// Resolve a [`PendingTarget`] to a `(RawModuleId, Span)` declaration location
/// using the ASG.
fn resolve_target(asg: &Asg, target: &PendingTarget) -> Option<(RawModuleId, Span)> {
    match target {
        PendingTarget::Global { module, name } => {
            let global = asg.global(module, name)?;
            Some((module.clone(), global.span))
        }
        PendingTarget::TypeDecl { module, name } => {
            let td = asg.type_decl(module, name)?;
            Some((module.clone(), td.type_def.var.span()))
        }
        PendingTarget::ImportAlias { module } => {
            let module_top = Span::new(Position::new(1, 1), Position::new(1, 1));
            Some((module.clone(), module_top))
        }
    }
}

/// Resolve all pending cursor declarations and references against a fully
/// loaded ASG.
fn resolve_pending_cursor_data(asg: &Asg, cd: &CursorData) {
    let Some(cursor) = &cd.cursor else {
        return;
    };

    // Resolve deferred declarations (cursor on a reference → set declaration).
    for pending in &cd.pending_declarations {
        if let Some((module, span)) = resolve_target(asg, &pending.target) {
            pending.cursor.set_declaration(module, span);
        }
    }

    // Resolve deferred references (for "find all references").
    for pending in &cd.pending_references {
        if let Some(decl) = resolve_target(asg, &pending.target) {
            cursor.track_reference(
                decl,
                (pending.reference_module.clone(), pending.reference_span),
            );
        }
    }

    // Apply already-resolved local references.
    for resolved in &cd.resolved_references {
        cursor.track_reference(resolved.declaration.clone(), resolved.reference.clone());
    }
}

/// Resolve cursor declarations and references for a module against an
/// already-built ASG. Used by the IDE/LSP path where the ASG is built first
/// and a module is then re-parsed with a cursor.
///
/// This walks the re-parsed module (which contains the cursor) to discover
/// the cursor and its declaration target. It also walks all *other* modules
/// in the ASG to collect references for "find all references" support.
pub fn resolve_cursor_refs(asg: &Asg, raw_id: &RawModuleId, pkg_id: &PackageId, body: &ModuleBody) {
    let mut cd = CursorData {
        cursor: None,
        pending_declarations: Vec::new(),
        pending_references: Vec::new(),
        resolved_references: Vec::new(),
    };

    let module_node = asg.module(raw_id);
    let module_id = match module_node {
        Some(mn) => &mn.module_id,
        None => return,
    };

    // Walk the re-parsed cursor module (this discovers the cursor).
    match body {
        ModuleBody::File(file_mod) => {
            analyze_module(raw_id, pkg_id, module_id, file_mod, &mut cd);
        }
        ModuleBody::Scle(scle_mod) => {
            analyze_scle_module(raw_id, pkg_id, module_id, scle_mod, &mut cd);
        }
    }

    // Walk all other modules in the ASG to collect their references (for
    // "find all references" on a declaration in the cursor's module).
    for mn in asg.modules() {
        if mn.raw_id == *raw_id {
            continue;
        }
        match &mn.body {
            ModuleBody::File(file_mod) => {
                analyze_module(&mn.raw_id, &mn.package_id, &mn.module_id, file_mod, &mut cd);
            }
            ModuleBody::Scle(scle_mod) => {
                analyze_scle_module(&mn.raw_id, &mn.package_id, &mn.module_id, scle_mod, &mut cd);
            }
        }
    }

    resolve_pending_cursor_data(asg, &cd);
}

/// Collect value/module references from an expression.
///
/// `in_fn` tracks whether we're inside a function body (for laziness).
fn collect_expr_refs(
    ctx: &RefContext,
    expr: &Expr,
    in_fn: bool,
    out: &mut Vec<Ref>,
    cd: &mut CursorData,
) {
    collect_expr_refs_with_scope(ctx, expr, in_fn, &mut HashMap::new(), out, cd);
}

fn track_var_cursor(
    ctx: &RefContext,
    var: &Loc<ast::Var>,
    target: PendingTarget,
    cd: &mut CursorData,
) {
    // If this Var carries the cursor, record it for declaration resolution.
    if let Some((cursor, _)) = &var.cursor {
        cd.cursor = Some(cursor.clone());
        cd.pending_declarations.push(PendingCursorDeclaration {
            cursor: cursor.clone(),
            target: target.clone(),
        });
    }
    // Always record as a reference for "find all references".
    cd.pending_references.push(PendingReference {
        target,
        reference_module: ctx.raw_id.clone(),
        reference_span: var.span(),
    });
}

fn track_local_var_cursor(
    ctx: &RefContext,
    var: &Loc<ast::Var>,
    decl_span: Span,
    cd: &mut CursorData,
) {
    let decl = (ctx.raw_id.clone(), decl_span);
    let reference = (ctx.raw_id.clone(), var.span());
    // If this Var carries the cursor, set declaration immediately.
    if let Some((cursor, _)) = &var.cursor {
        cd.cursor = Some(cursor.clone());
        cursor.set_declaration(ctx.raw_id.clone(), decl_span);
        cursor.set_identifier(CursorIdentifier::Let(var.name.clone()));
    }
    // Always record the reference for "find all references".
    cd.resolved_references.push(ResolvedReference {
        declaration: decl,
        reference,
    });
}

fn collect_expr_refs_with_scope(
    ctx: &RefContext,
    expr: &Expr,
    in_fn: bool,
    locals: &mut HashMap<String, Span>,
    out: &mut Vec<Ref>,
    cd: &mut CursorData,
) {
    match expr {
        Expr::Var(var) => {
            let name = &var.name;
            if let Some(&decl_span) = locals.get(name.as_str()) {
                track_local_var_cursor(ctx, var, decl_span, cd);
                return;
            }
            if ctx.global_names.contains(name.as_str()) {
                out.push(Ref {
                    target: NodeId::Global(ctx.raw_id.clone(), name.clone()),
                    lazy: in_fn,
                    span: var.span(),
                });
                track_var_cursor(
                    ctx,
                    var,
                    PendingTarget::Global {
                        module: ctx.raw_id.clone(),
                        name: name.clone(),
                    },
                    cd,
                );
            } else if let Some(import_id) = ctx.import_aliases.get(name.as_str()) {
                out.push(Ref {
                    target: NodeId::Module(import_id.clone()),
                    lazy: in_fn,
                    span: var.span(),
                });
                track_var_cursor(
                    ctx,
                    var,
                    PendingTarget::ImportAlias {
                        module: import_id.clone(),
                    },
                    cd,
                );
            }
        }

        Expr::PropertyAccess(pa) => {
            // Check for qualified reference: Import.member
            if let Expr::Var(var) = pa.expr.as_ref().as_ref() {
                let name = &var.name;
                if !locals.contains_key(name.as_str())
                    && let Some(import_id) = ctx.import_aliases.get(name.as_str())
                {
                    // This is Import.member — add edge to the specific global.
                    out.push(Ref {
                        target: NodeId::Global(import_id.clone(), pa.property.name.clone()),
                        lazy: in_fn,
                        span: pa.property.span(),
                    });
                    // Track cursor on the import alias var.
                    track_var_cursor(
                        ctx,
                        var,
                        PendingTarget::ImportAlias {
                            module: import_id.clone(),
                        },
                        cd,
                    );
                    // Track cursor on the property.
                    track_var_cursor(
                        ctx,
                        &pa.property,
                        PendingTarget::Global {
                            module: import_id.clone(),
                            name: pa.property.name.clone(),
                        },
                        cd,
                    );
                    // Don't recurse into the Var — we've handled it.
                    return;
                }
            }
            // Not a qualified reference — recurse normally.
            collect_expr_refs_with_scope(ctx, pa.expr.as_ref().as_ref(), in_fn, locals, out, cd);
        }

        Expr::Fn(fn_expr) => {
            // Function body references are lazy.
            let mut inner_locals = locals.clone();
            for param in &fn_expr.params {
                inner_locals.insert(param.var.name.clone(), param.var.span());
                // Track cursor on parameter declaration.
                if let Some((cursor, _)) = &param.var.cursor {
                    cd.cursor = Some(cursor.clone());
                    cursor.set_declaration(ctx.raw_id.clone(), param.var.span());
                    cursor.set_identifier(CursorIdentifier::Let(param.var.name.clone()));
                }
            }
            if let Some(body) = &fn_expr.body {
                collect_expr_refs_with_scope(
                    ctx,
                    body.as_ref().as_ref(),
                    true,
                    &mut inner_locals,
                    out,
                    cd,
                );
            }
            // Type annotations on params are NOT lazy.
            for param in &fn_expr.params {
                if let Some(ty) = &param.ty {
                    collect_type_refs(ctx, ty.as_ref(), out, cd);
                }
            }
            // Type param bounds.
            for tp in &fn_expr.type_params {
                if let Some(bound) = &tp.bound {
                    collect_type_refs(ctx, bound.as_ref(), out, cd);
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
                cd,
            );
            // Type annotation on the let bind.
            if let Some(ty) = &let_expr.bind.ty {
                collect_type_refs(ctx, ty.as_ref(), out, cd);
            }
            // Track cursor on the let binding declaration itself.
            if let Some((cursor, _)) = &let_expr.bind.var.cursor {
                cd.cursor = Some(cursor.clone());
                cursor.set_declaration(ctx.raw_id.clone(), let_expr.bind.var.span());
                cursor.set_identifier(CursorIdentifier::Let(let_expr.bind.var.name.clone()));
            }
            // The body gets the new local binding.
            if let Some(body) = &let_expr.expr {
                let mut inner_locals = locals.clone();
                inner_locals.insert(let_expr.bind.var.name.clone(), let_expr.bind.var.span());
                collect_expr_refs_with_scope(
                    ctx,
                    body.as_ref().as_ref(),
                    in_fn,
                    &mut inner_locals,
                    out,
                    cd,
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
                cd,
            );
            collect_expr_refs_with_scope(
                ctx,
                if_expr.then_expr.as_ref().as_ref(),
                in_fn,
                locals,
                out,
                cd,
            );
            if let Some(else_expr) = &if_expr.else_expr {
                collect_expr_refs_with_scope(
                    ctx,
                    else_expr.as_ref().as_ref(),
                    in_fn,
                    locals,
                    out,
                    cd,
                );
            }
        }

        Expr::Call(call) => {
            collect_expr_refs_with_scope(
                ctx,
                call.callee.as_ref().as_ref(),
                in_fn,
                locals,
                out,
                cd,
            );
            for arg in &call.args {
                collect_expr_refs_with_scope(ctx, arg.as_ref(), in_fn, locals, out, cd);
            }
            for ty_arg in &call.type_args {
                collect_type_refs(ctx, ty_arg.as_ref(), out, cd);
            }
        }

        Expr::Binary(bin) => {
            collect_expr_refs_with_scope(ctx, bin.lhs.as_ref().as_ref(), in_fn, locals, out, cd);
            collect_expr_refs_with_scope(ctx, bin.rhs.as_ref().as_ref(), in_fn, locals, out, cd);
        }

        Expr::Unary(un) => {
            collect_expr_refs_with_scope(ctx, un.expr.as_ref().as_ref(), in_fn, locals, out, cd);
        }

        Expr::Record(rec) => {
            for field in &rec.fields {
                collect_expr_refs_with_scope(ctx, &field.expr, in_fn, locals, out, cd);
            }
        }

        Expr::Dict(dict) => {
            for entry in &dict.entries {
                collect_expr_refs_with_scope(ctx, &entry.key, in_fn, locals, out, cd);
                collect_expr_refs_with_scope(ctx, &entry.value, in_fn, locals, out, cd);
            }
        }

        Expr::List(list) => {
            for item in &list.items {
                collect_list_item_refs(ctx, item, in_fn, locals, out, cd);
            }
        }

        Expr::IndexedAccess(ia) => {
            collect_expr_refs_with_scope(ctx, ia.expr.as_ref().as_ref(), in_fn, locals, out, cd);
            collect_expr_refs_with_scope(ctx, ia.index.as_ref().as_ref(), in_fn, locals, out, cd);
        }

        Expr::TypeCast(tc) => {
            collect_expr_refs_with_scope(ctx, tc.expr.as_ref().as_ref(), in_fn, locals, out, cd);
            collect_type_refs(ctx, tc.ty.as_ref(), out, cd);
        }

        Expr::Interp(interp) => {
            for part in &interp.parts {
                collect_expr_refs_with_scope(ctx, part.as_ref(), in_fn, locals, out, cd);
            }
        }

        Expr::Raise(raise) => {
            collect_expr_refs_with_scope(ctx, raise.expr.as_ref().as_ref(), in_fn, locals, out, cd);
        }

        Expr::Try(try_expr) => {
            collect_expr_refs_with_scope(
                ctx,
                try_expr.expr.as_ref().as_ref(),
                in_fn,
                locals,
                out,
                cd,
            );
            for catch in &try_expr.catches {
                // exception_var is a reference to an exception name (global-ish).
                let exc_name = &catch.exception_var.name;
                if !locals.contains_key(exc_name.as_str())
                    && ctx.global_names.contains(exc_name.as_str())
                {
                    out.push(Ref {
                        target: NodeId::Global(ctx.raw_id.clone(), exc_name.clone()),
                        lazy: in_fn,
                        span: catch.exception_var.span(),
                    });
                    track_var_cursor(
                        ctx,
                        &catch.exception_var,
                        PendingTarget::Global {
                            module: ctx.raw_id.clone(),
                            name: exc_name.clone(),
                        },
                        cd,
                    );
                }

                let mut catch_locals = locals.clone();
                if let Some(arg) = &catch.catch_arg {
                    catch_locals.insert(arg.name.clone(), arg.span());
                    // Track cursor on catch arg declaration.
                    if let Some((cursor, _)) = &arg.cursor {
                        cd.cursor = Some(cursor.clone());
                        cursor.set_declaration(ctx.raw_id.clone(), arg.span());
                        cursor.set_identifier(CursorIdentifier::Let(arg.name.clone()));
                    }
                }
                collect_expr_refs_with_scope(ctx, &catch.body, in_fn, &mut catch_locals, out, cd);
            }
        }

        Expr::Extern(ext) => {
            collect_type_refs(ctx, ext.ty.as_ref(), out, cd);
        }

        Expr::Exception(exc) => {
            if let Some(ty) = &exc.ty {
                collect_type_refs(ctx, ty.as_ref(), out, cd);
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
    locals: &mut HashMap<String, Span>,
    out: &mut Vec<Ref>,
    cd: &mut CursorData,
) {
    match item {
        ListItem::Expr(expr) => {
            collect_expr_refs_with_scope(ctx, expr.as_ref(), in_fn, locals, out, cd);
        }
        ListItem::If(if_item) => {
            collect_expr_refs_with_scope(
                ctx,
                if_item.condition.as_ref().as_ref(),
                in_fn,
                locals,
                out,
                cd,
            );
            collect_list_item_refs(ctx, &if_item.then_item, in_fn, locals, out, cd);
        }
        ListItem::For(for_item) => {
            collect_expr_refs_with_scope(
                ctx,
                for_item.iterable.as_ref().as_ref(),
                in_fn,
                locals,
                out,
                cd,
            );
            let mut inner_locals = locals.clone();
            inner_locals.insert(for_item.var.name.clone(), for_item.var.span());
            // Track cursor on for-loop variable declaration.
            if let Some((cursor, _)) = &for_item.var.cursor {
                cd.cursor = Some(cursor.clone());
                cursor.set_declaration(ctx.raw_id.clone(), for_item.var.span());
                cursor.set_identifier(CursorIdentifier::Let(for_item.var.name.clone()));
            }
            collect_list_item_refs(ctx, &for_item.emit_item, in_fn, &mut inner_locals, out, cd);
        }
    }
}

/// Collect type references from a type expression.
fn collect_type_refs(ctx: &RefContext, ty: &TypeExpr, out: &mut Vec<Ref>, cd: &mut CursorData) {
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
                track_var_cursor(
                    ctx,
                    var,
                    PendingTarget::TypeDecl {
                        module: ctx.raw_id.clone(),
                        name: name.clone(),
                    },
                    cd,
                );
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
                // Track cursor on the import alias.
                track_var_cursor(
                    ctx,
                    var,
                    PendingTarget::ImportAlias {
                        module: import_id.clone(),
                    },
                    cd,
                );
                // Track cursor on the type property.
                track_var_cursor(
                    ctx,
                    &pa.property,
                    PendingTarget::TypeDecl {
                        module: import_id.clone(),
                        name: pa.property.name.clone(),
                    },
                    cd,
                );
                return;
            }
            collect_type_refs(ctx, pa.expr.as_ref().as_ref(), out, cd);
        }
        TypeExpr::Optional(inner) => collect_type_refs(ctx, inner.as_ref().as_ref(), out, cd),
        TypeExpr::List(inner) => collect_type_refs(ctx, inner.as_ref().as_ref(), out, cd),
        TypeExpr::Fn(f) => {
            let own_params: HashSet<&str> = f
                .type_params
                .iter()
                .map(|tp| tp.var.name.as_str())
                .collect();
            for param in &f.params {
                collect_type_refs_excluding(ctx, param.as_ref(), &own_params, out, cd);
            }
            collect_type_refs_excluding(ctx, f.ret.as_ref().as_ref(), &own_params, out, cd);
            for tp in &f.type_params {
                if let Some(bound) = &tp.bound {
                    collect_type_refs(ctx, bound.as_ref(), out, cd);
                }
            }
        }
        TypeExpr::Record(rec) => {
            for field in &rec.fields {
                collect_type_refs(ctx, field.ty.as_ref(), out, cd);
            }
        }
        TypeExpr::Dict(dict) => {
            collect_type_refs(ctx, dict.key.as_ref().as_ref(), out, cd);
            collect_type_refs(ctx, dict.value.as_ref().as_ref(), out, cd);
        }
        TypeExpr::Application(app) => {
            collect_type_refs(ctx, app.base.as_ref().as_ref(), out, cd);
            for arg in &app.args {
                collect_type_refs(ctx, arg.as_ref(), out, cd);
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
    cd: &mut CursorData,
) {
    match ty {
        TypeExpr::Var(var) if exclude.contains(var.name.as_str()) => {}
        _ => collect_type_refs(ctx, ty, out, cd),
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
    AmbiguousModule {
        raw_id: RawModuleId,
        import_source: Option<ImportSource>,
    },
}

impl LoaderDiag {
    fn import_source(&self) -> Option<&ImportSource> {
        match self {
            LoaderDiag::PackageNotFound { import_source, .. }
            | LoaderDiag::ModuleNotFound { import_source, .. }
            | LoaderDiag::AmbiguousModule { import_source, .. } => import_source.as_ref(),
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
            LoaderDiag::AmbiguousModule { raw_id, .. } => {
                write!(
                    f,
                    "ambiguous module {}: both `.scl` and `.scle` exist",
                    raw_id.join("/")
                )
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
