use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionParams, CompletionResponse,
    CompletionTriggerKind,
};
use sclc::{ModuleId, Program, SourceRepo, TypeChecker};

use crate::convert::{lsp_to_position, uri_to_path};
use crate::helpers::{
    find_file_mod_in_program, find_module_by_path, get_var_type, package_module_id,
};
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
    for (pkg_id, package) in program.packages() {
        for (module_path, _) in package.modules() {
            let mid = package_module_id(pkg_id, module_path);
            let import_path = mid
                .as_slice()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("/");
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
    // The cursor is right after the `.`. Position one character before the dot
    // to find the expression.
    let pos = lsp_to_position(lsp_types::Position {
        line: lsp_pos.line,
        character: lsp_pos.character.saturating_sub(1),
    });

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

#[derive(serde::Serialize, serde::Deserialize)]
struct ResolveData {
    module_segments: Vec<String>,
    name: String,
}

const SCL_KEYWORDS: &[&str] = &[
    "let", "export", "import", "if", "then", "else", "fn", "true", "false", "nil", "try", "catch",
    "raise", "extern",
];
