use std::path::PathBuf;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionParams, CompletionResponse,
    CompletionTriggerKind,
};
use sclc::{ModuleId, Program, SourceRepo, TypeChecker, TypeEnv};

use crate::convert::{lsp_to_position, uri_to_path};
use crate::overlay::OverlaySource;
use crate::{LanguageServer, OutgoingMessage, RequestId};

pub async fn handle_completion<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: CompletionParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document_position.text_document.uri;
    let lsp_pos = params.text_document_position.position;

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<CompletionResponse>::None,
        )];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(
            id,
            Option::<CompletionResponse>::None,
        )];
    };

    let Some((module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<CompletionResponse>::None,
        )];
    };

    // Check if this is a dot-triggered completion.
    let is_dot_trigger = params.context.as_ref().is_some_and(|ctx| {
        ctx.trigger_kind == CompletionTriggerKind::TRIGGER_CHARACTER
            && ctx.trigger_character.as_deref() == Some(".")
    });

    let items = if is_dot_trigger {
        dot_completions(program, &module_id, file_mod, lsp_pos)
    } else {
        scope_completions(program, &module_id, file_mod)
    };

    vec![OutgoingMessage::response(
        id,
        Some(CompletionResponse::List(CompletionList {
            is_incomplete: false,
            items,
        })),
    )]
}

pub async fn handle_completion_resolve<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    mut item: CompletionItem,
) -> Vec<OutgoingMessage> {
    // If the item has `data` with module_id and var_name, resolve its type.
    if let Some(data) = &item.data
        && let Ok(resolve_data) = serde_json::from_value::<ResolveData>(data.clone())
        && let Some(program) = server.last_program.as_ref()
    {
        let module_id = ModuleId::new(resolve_data.module_segments);
        if let Some(file_mod) = find_file_mod_in_program(program, &module_id)
            && let Some(ty) = get_var_type(program, &module_id, file_mod, &resolve_data.name)
        {
            item.detail = Some(ty.to_string());
        }
    }
    vec![OutgoingMessage::response(id, item)]
}

/// Completions for names in scope (globals, imports, keywords).
fn scope_completions<S: SourceRepo>(
    program: &Program<OverlaySource<S>>,
    module_id: &ModuleId,
    file_mod: &sclc::FileMod,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Globals (let/export bindings)
    let globals = file_mod.find_globals();
    for (name, expr) in &globals {
        let kind = match expr.as_ref() {
            sclc::Expr::Fn(_) => CompletionItemKind::FUNCTION,
            sclc::Expr::Record(_) => CompletionItemKind::STRUCT,
            sclc::Expr::Exception(_) => CompletionItemKind::EVENT,
            _ => CompletionItemKind::VARIABLE,
        };
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(kind),
            data: Some(
                serde_json::to_value(ResolveData {
                    module_segments: module_id.as_slice().to_vec(),
                    name: name.to_string(),
                })
                .unwrap(),
            ),
            ..Default::default()
        });
    }

    // Imports
    let checker = TypeChecker::new(program);
    let imports = checker.find_imports(file_mod);
    for name in imports.keys() {
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::MODULE),
            data: Some(
                serde_json::to_value(ResolveData {
                    module_segments: module_id.as_slice().to_vec(),
                    name: name.to_string(),
                })
                .unwrap(),
            ),
            ..Default::default()
        });
    }

    // Import path suggestions (known modules from the compiled program)
    for (package_id, package) in program.packages() {
        for (module_path, _) in package.modules() {
            let mid = package_module_id(package_id, module_path);
            let import_path = mid
                .as_slice()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(".");
            let label = mid.as_slice().last().cloned().unwrap_or_default();
            items.push(CompletionItem {
                label,
                kind: Some(CompletionItemKind::MODULE),
                detail: Some(import_path.clone()),
                filter_text: Some(import_path.clone()),
                insert_text: Some(import_path),
                ..Default::default()
            });
        }
    }

    // SCL keywords
    for kw in SCL_KEYWORDS {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        });
    }

    items
}

/// Completions after a `.` — suggest record fields / module exports.
fn dot_completions<S: SourceRepo>(
    program: &Program<OverlaySource<S>>,
    module_id: &ModuleId,
    file_mod: &sclc::FileMod,
    lsp_pos: lsp_types::Position,
) -> Vec<CompletionItem> {
    // The cursor is right after the `.`. We need to find the expression before the dot.
    // Position the cursor one character before the dot to find the expression.
    let before_dot = sclc::Position::new(lsp_pos.line + 1, lsp_pos.character); // character is 0-based in LSP; sclc is 1-based, and we want one before the trigger
    let pos = lsp_to_position(lsp_types::Position {
        line: lsp_pos.line,
        character: lsp_pos.character.saturating_sub(1),
    });

    let _ = before_dot; // use `pos` which already accounts for the offset

    // Find the node at the position before the dot.
    let Some(node) = crate::query::node_at_position(file_mod, pos) else {
        return vec![];
    };

    // We need the type of the expression before the dot.
    let var_name = match node {
        crate::query::NodeAtPosition::Var(var) => Some(var.name.as_str()),
        crate::query::NodeAtPosition::LetBindVar(bind) => Some(bind.var.name.as_str()),
        _ => None,
    };

    let Some(var_name) = var_name else {
        return vec![];
    };

    let Some(ty) = get_var_type(program, module_id, file_mod, var_name) else {
        return vec![];
    };

    type_member_completions(&ty)
}

/// Generate completion items for the members of a type.
fn type_member_completions(ty: &sclc::Type) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    if let sclc::Type::Record(record) = ty.unfold() {
        for (name, field_ty) in record.iter() {
            let kind = match field_ty {
                sclc::Type::Fn(_) => CompletionItemKind::METHOD,
                _ => CompletionItemKind::FIELD,
            };
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(kind),
                detail: Some(field_ty.to_string()),
                ..Default::default()
            });
        }
    }

    items
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

    if let Some(global_expr) = globals.get(var_name)
        && let Ok(diagnosed) = checker.check_expr(&env, global_expr, None)
    {
        return Some(diagnosed.into_inner());
    }

    if let Some((_, Some(import_file_mod))) = imports.get(var_name) {
        let import_env = TypeEnv::new().with_module_id(module_id);
        if let Ok(diagnosed) = checker.check_file_mod(&import_env, import_file_mod) {
            return Some(diagnosed.into_inner());
        }
    }

    None
}

fn find_module_by_path<'a, S>(
    program: &'a Program<OverlaySource<S>>,
    root_path: &Option<PathBuf>,
    path: &std::path::Path,
) -> Option<(ModuleId, &'a sclc::FileMod)> {
    let root = root_path.as_deref().unwrap_or(std::path::Path::new("."));
    for (package_id, package) in program.packages() {
        for (module_path, file_mod) in package.modules() {
            if root.join(module_path) == path {
                let module_id = package_module_id(package_id, module_path);
                return Some((module_id, file_mod));
            }
        }
    }
    None
}

fn find_file_mod_in_program<'a, S>(
    program: &'a Program<OverlaySource<S>>,
    target_module_id: &ModuleId,
) -> Option<&'a sclc::FileMod> {
    for (package_id, package) in program.packages() {
        for (module_path, file_mod) in package.modules() {
            let mid = package_module_id(package_id, module_path);
            if mid == *target_module_id {
                return Some(file_mod);
            }
        }
    }
    None
}

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

#[derive(serde::Serialize, serde::Deserialize)]
struct ResolveData {
    module_segments: Vec<String>,
    name: String,
}

const SCL_KEYWORDS: &[&str] = &[
    "let", "export", "import", "if", "then", "else", "fn", "true", "false", "nil", "try", "catch",
    "raise", "extern",
];
