use std::path::PathBuf;

use lsp_types::{
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, LanguageString, Location,
    MarkedString, ReferenceParams, SymbolKind,
};
use sclc::{ModuleId, Program, SourceRepo, TypeChecker, TypeEnv};

use crate::convert::{lsp_to_position, path_to_uri, span_to_range, uri_to_path};
use crate::overlay::OverlaySource;
use crate::query::{self, NodeAtPosition};
use crate::{LanguageServer, OutgoingMessage, RequestId};

pub async fn handle_document_symbol<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: DocumentSymbolParams,
) -> Vec<OutgoingMessage> {
    let Some(path) = uri_to_path(&params.text_document.uri) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<DocumentSymbolResponse>::None,
        )];
    };

    let program = server.last_program.as_ref();
    let file_mod = program.and_then(|p| find_file_mod_by_path(p, &server.root_path, &path));

    let Some(file_mod) = file_mod else {
        return vec![OutgoingMessage::response(
            id,
            Option::<DocumentSymbolResponse>::None,
        )];
    };

    let mut symbols = Vec::new();

    for stmt in &file_mod.statements {
        match stmt {
            sclc::ModStmt::Import(import) => {
                if let Some(last_var) = import.vars.last() {
                    #[allow(deprecated)]
                    symbols.push(DocumentSymbol {
                        name: last_var.name.clone(),
                        detail: Some("import".to_string()),
                        kind: SymbolKind::MODULE,
                        tags: None,
                        deprecated: None,
                        range: span_to_range(import.span()),
                        selection_range: span_to_range(last_var.span()),
                        children: None,
                    });
                }
            }
            sclc::ModStmt::Let(bind) => {
                let kind = expr_symbol_kind(bind.expr.as_ref().as_ref());
                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: bind.var.name.clone(),
                    detail: None,
                    kind,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(sclc::Span::new(
                        bind.var.span().start(),
                        bind.expr.span().end(),
                    )),
                    selection_range: span_to_range(bind.var.span()),
                    children: None,
                });
            }
            sclc::ModStmt::Export(bind) => {
                let kind = expr_symbol_kind(bind.expr.as_ref().as_ref());
                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: bind.var.name.clone(),
                    detail: Some("export".to_string()),
                    kind,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(sclc::Span::new(
                        bind.var.span().start(),
                        bind.expr.span().end(),
                    )),
                    selection_range: span_to_range(bind.var.span()),
                    children: None,
                });
            }
            sclc::ModStmt::Expr(_) => {}
        }
    }

    vec![OutgoingMessage::response(
        id,
        DocumentSymbolResponse::Nested(symbols),
    )]
}

pub async fn handle_goto_definition<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: GotoDefinitionParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document_position_params.text_document.uri;
    let lsp_pos = params.text_document_position_params.position;
    let pos = lsp_to_position(lsp_pos);

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<GotoDefinitionResponse>::None,
        )];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(
            id,
            Option::<GotoDefinitionResponse>::None,
        )];
    };

    let Some((module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<GotoDefinitionResponse>::None,
        )];
    };

    let Some(node) = query::node_at_position(file_mod, pos) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<GotoDefinitionResponse>::None,
        )];
    };

    let result = match node {
        NodeAtPosition::Var(var) => {
            resolve_var_definition(program, &server.root_path, &module_id, file_mod, &var.name)
        }
        NodeAtPosition::LetBindVar(bind) => {
            // Cursor is on a let binding's name — go to the binding itself (same location)
            let root = server
                .root_path
                .as_deref()
                .unwrap_or(std::path::Path::new("."));
            let def_path = module_id_to_path(root, &module_id);
            path_to_uri(&def_path).map(|uri| Location {
                uri,
                range: span_to_range(bind.var.span()),
            })
        }
        _ => None,
    };

    vec![OutgoingMessage::response(
        id,
        result.map(GotoDefinitionResponse::Scalar),
    )]
}

pub async fn handle_hover<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: HoverParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document_position_params.text_document.uri;
    let lsp_pos = params.text_document_position_params.position;
    let pos = lsp_to_position(lsp_pos);

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(id, Option::<Hover>::None)];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(id, Option::<Hover>::None)];
    };

    let Some((module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path) else {
        return vec![OutgoingMessage::response(id, Option::<Hover>::None)];
    };

    let Some(node) = query::node_at_position(file_mod, pos) else {
        return vec![OutgoingMessage::response(id, Option::<Hover>::None)];
    };

    let hover = match node {
        NodeAtPosition::Var(var) => {
            get_var_type(program, &module_id, file_mod, &var.name).map(|ty| Hover {
                contents: HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
                    language: "scl".to_string(),
                    value: format!("{}: {}", var.name, ty),
                })),
                range: Some(span_to_range(var.span())),
            })
        }
        NodeAtPosition::LetBindVar(bind) => {
            get_var_type(program, &module_id, file_mod, &bind.var.name).map(|ty| Hover {
                contents: HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
                    language: "scl".to_string(),
                    value: format!("{}: {}", bind.var.name, ty),
                })),
                range: Some(span_to_range(bind.var.span())),
            })
        }
        NodeAtPosition::Property { expr, property } => {
            get_expr_type(program, &module_id, file_mod, expr)
                .and_then(|ty| {
                    if let sclc::Type::Record(record) = ty.unfold() {
                        record.get(&property.name).cloned()
                    } else {
                        None
                    }
                })
                .map(|field_ty| Hover {
                    contents: HoverContents::Scalar(MarkedString::LanguageString(
                        LanguageString {
                            language: "scl".to_string(),
                            value: format!("{}: {}", property.name, field_ty),
                        },
                    )),
                    range: Some(span_to_range(property.span())),
                })
        }
        NodeAtPosition::Expr(_) => None,
    };

    vec![OutgoingMessage::response(id, hover)]
}

pub async fn handle_references<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: ReferenceParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document_position.text_document.uri;
    let lsp_pos = params.text_document_position.position;
    let pos = lsp_to_position(lsp_pos);

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(id, Option::<Vec<Location>>::None)];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(id, Option::<Vec<Location>>::None)];
    };

    let Some((module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path) else {
        return vec![OutgoingMessage::response(id, Option::<Vec<Location>>::None)];
    };

    let Some(node) = query::node_at_position(file_mod, pos) else {
        return vec![OutgoingMessage::response(id, Option::<Vec<Location>>::None)];
    };

    // Determine the variable name to search for.
    let var_name = match node {
        NodeAtPosition::Var(var) => &var.name,
        NodeAtPosition::LetBindVar(bind) => &bind.var.name,
        _ => return vec![OutgoingMessage::response(id, Option::<Vec<Location>>::None)],
    };

    let root = server
        .root_path
        .as_deref()
        .unwrap_or(std::path::Path::new("."));

    // Determine the definition's scope: is this a global/export, import, or something else?
    let globals = file_mod.find_globals();
    let is_global = globals.contains_key(var_name.as_str());

    let checker = TypeChecker::new(program);
    let imports = checker.find_imports(file_mod);
    let is_import = imports.contains_key(var_name.as_str());

    let mut locations = Vec::new();

    if is_global || is_import {
        // Search the current module for all references to this name.
        let spans = query::find_var_references(file_mod, var_name);
        let def_path = module_id_to_path(root, &module_id);
        if let Some(file_uri) = path_to_uri(&def_path) {
            for span in spans {
                locations.push(Location {
                    uri: file_uri.clone(),
                    range: span_to_range(span),
                });
            }
        }
    } else {
        // For locals or unknown symbols, just search the current file by name.
        let spans = query::find_var_references(file_mod, var_name);
        let def_path = module_id_to_path(root, &module_id);
        if let Some(file_uri) = path_to_uri(&def_path) {
            for span in spans {
                locations.push(Location {
                    uri: file_uri.clone(),
                    range: span_to_range(span),
                });
            }
        }
    }

    vec![OutgoingMessage::response(id, Some(locations))]
}

/// Resolve a variable reference to its definition location.
fn resolve_var_definition<S: SourceRepo>(
    program: &Program<OverlaySource<S>>,
    root_path: &Option<PathBuf>,
    module_id: &ModuleId,
    file_mod: &sclc::FileMod,
    var_name: &str,
) -> Option<Location> {
    let root = root_path.as_deref().unwrap_or(std::path::Path::new("."));

    // Check globals (let/export bindings in the same file)
    let globals = file_mod.find_globals();
    if let Some(global_expr) = globals.get(var_name) {
        let def_path = module_id_to_path(root, module_id);
        return path_to_uri(&def_path).map(|uri| Location {
            uri,
            range: span_to_range(global_expr.span()),
        });
    }

    // Check imports
    let checker = TypeChecker::new(program);
    let imports = checker.find_imports(file_mod);
    if let Some((import_module_id, _)) = imports.get(var_name) {
        let def_path = module_id_to_path(root, import_module_id);
        return path_to_uri(&def_path).map(|uri| Location {
            uri,
            range: lsp_types::Range::default(),
        });
    }

    None
}

/// Get the type of an arbitrary expression by running the type checker.
fn get_expr_type<S: SourceRepo>(
    program: &Program<OverlaySource<S>>,
    module_id: &ModuleId,
    file_mod: &sclc::FileMod,
    expr: &sclc::Loc<sclc::Expr>,
) -> Option<sclc::Type> {
    let globals = file_mod.find_globals();
    let checker = TypeChecker::new(program);
    let imports = checker.find_imports(file_mod);
    let env = TypeEnv::new()
        .with_module_id(module_id)
        .with_globals(&globals)
        .with_imports(&imports);

    checker
        .check_expr(&env, expr, None)
        .ok()
        .map(|d| d.into_inner())
}

/// Get the type of a variable by running the type checker.
fn get_var_type<S: SourceRepo>(
    program: &Program<OverlaySource<S>>,
    module_id: &ModuleId,
    file_mod: &sclc::FileMod,
    var_name: &str,
) -> Option<sclc::Type> {
    let globals = file_mod.find_globals();
    let checker = TypeChecker::new(program);
    let imports = checker.find_imports(file_mod);
    let env = TypeEnv::new()
        .with_module_id(module_id)
        .with_globals(&globals)
        .with_imports(&imports);

    // Check locals won't help here (we'd need scope analysis).
    // Check globals: type-check the global's expression.
    if let Some(global_expr) = globals.get(var_name)
        && let Ok(diagnosed) = checker.check_expr(&env, global_expr, None)
    {
        return Some(diagnosed.into_inner());
    }

    // Check imports: type-check the imported module.
    if let Some((_, Some(import_file_mod))) = imports.get(var_name) {
        let import_env = TypeEnv::new().with_module_id(module_id);
        if let Ok(diagnosed) = checker.check_file_mod(&import_env, import_file_mod) {
            return Some(diagnosed.into_inner());
        }
    }

    None
}

/// Find a FileMod by file path in the compiled program.
fn find_file_mod_by_path<'a, S>(
    program: &'a Program<OverlaySource<S>>,
    root_path: &Option<PathBuf>,
    path: &std::path::Path,
) -> Option<&'a sclc::FileMod> {
    find_module_by_path(program, root_path, path).map(|(_, fm)| fm)
}

/// Find a module (module ID + FileMod) by file path.
fn find_module_by_path<'a, S>(
    program: &'a Program<OverlaySource<S>>,
    root_path: &Option<PathBuf>,
    path: &std::path::Path,
) -> Option<(ModuleId, &'a sclc::FileMod)> {
    let root = root_path.as_deref().unwrap_or(std::path::Path::new("."));

    for (package_id, package) in program.packages() {
        for (module_path, file_mod) in package.modules() {
            let full_path = module_path_to_abs(root, package_id, module_path);
            if full_path == path {
                let module_id = package_module_id(package_id, module_path);
                return Some((module_id, file_mod));
            }
        }
    }
    None
}

/// Convert a module's relative path (within a package) to an absolute file path.
fn module_path_to_abs(
    root: &std::path::Path,
    _package_id: &ModuleId,
    module_path: &std::path::Path,
) -> PathBuf {
    root.join(module_path)
}

/// Build a full ModuleId from package ID + module file path.
fn package_module_id(package_id: &ModuleId, module_path: &std::path::Path) -> ModuleId {
    let mut segments: Vec<String> = package_id.as_slice().to_vec();
    if let Some(parent) = module_path.parent() {
        for component in parent.components() {
            if let std::path::Component::Normal(part) = component {
                segments.push(part.to_string_lossy().into_owned());
            }
        }
    }
    if let Some(stem) = module_path.file_stem() {
        segments.push(stem.to_string_lossy().into_owned());
    }
    ModuleId::new(segments)
}

/// Convert a module ID back to a file path.
fn module_id_to_path(root_path: &std::path::Path, module_id: &ModuleId) -> PathBuf {
    let segments = module_id.as_slice();
    if segments.len() < 3 {
        return root_path.to_path_buf();
    }

    let file_segments = &segments[2..];
    let mut path = root_path.to_path_buf();
    for (i, segment) in file_segments.iter().enumerate() {
        if i == file_segments.len() - 1 {
            path.push(format!("{}.scl", segment));
        } else {
            path.push(segment);
        }
    }
    path
}

fn expr_symbol_kind(expr: &sclc::Expr) -> SymbolKind {
    match expr {
        sclc::Expr::Fn(_) => SymbolKind::FUNCTION,
        sclc::Expr::Record(_) => SymbolKind::STRUCT,
        sclc::Expr::Exception(_) => SymbolKind::EVENT,
        _ => SymbolKind::VARIABLE,
    }
}
