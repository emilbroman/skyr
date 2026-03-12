use lsp_types::{
    ParameterInformation, ParameterLabel, SignatureHelp, SignatureHelpParams, SignatureInformation,
};
use sclc::SourceRepo;

use crate::convert::{lsp_to_position, uri_to_path};
use crate::helpers::{find_module_by_path, get_var_type};
use crate::{LanguageServer, OutgoingMessage, RequestId};

pub async fn handle_signature_help<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: SignatureHelpParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document_position_params.text_document.uri;
    let lsp_pos = params.text_document_position_params.position;

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(id, Option::<SignatureHelp>::None)];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(id, Option::<SignatureHelp>::None)];
    };

    let Some((module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path) else {
        return vec![OutgoingMessage::response(id, Option::<SignatureHelp>::None)];
    };

    // Try to find a CallExpr at or near the cursor position.
    let call_info = find_call_at_position(file_mod, lsp_to_position(lsp_pos));

    let Some((callee_name, active_param)) = call_info else {
        return vec![OutgoingMessage::response(id, Option::<SignatureHelp>::None)];
    };

    // Get the type of the callee.
    let Some(ty) = get_var_type(program, &module_id, file_mod, &callee_name) else {
        return vec![OutgoingMessage::response(id, Option::<SignatureHelp>::None)];
    };

    let sclc::Type::Fn(fn_type) = &ty else {
        return vec![OutgoingMessage::response(id, Option::<SignatureHelp>::None)];
    };

    // Try to get parameter names from the AST if the callee is a known function.
    let param_names = get_fn_param_names(file_mod, &callee_name);

    let parameters: Vec<ParameterInformation> = fn_type
        .params
        .iter()
        .enumerate()
        .map(|(i, param_ty)| {
            let label = if let Some(names) = &param_names
                && i < names.len()
            {
                format!("{}: {}", names[i], param_ty)
            } else {
                param_ty.to_string()
            };
            ParameterInformation {
                label: ParameterLabel::Simple(label),
                documentation: None,
            }
        })
        .collect();

    // Build the signature label.
    let params_str = parameters
        .iter()
        .map(|p| match &p.label {
            ParameterLabel::Simple(s) => s.clone(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join(", ");

    let label = format!("{}({}) {}", callee_name, params_str, fn_type.ret);

    let signature = SignatureInformation {
        label,
        documentation: None,
        parameters: Some(parameters),
        active_parameter: Some(active_param),
    };

    vec![OutgoingMessage::response(
        id,
        Some(SignatureHelp {
            signatures: vec![signature],
            active_signature: Some(0),
            active_parameter: Some(active_param),
        }),
    )]
}

/// Try to find a call expression at the given position and return the callee name
/// and the active parameter index.
fn find_call_at_position(file_mod: &sclc::FileMod, pos: sclc::Position) -> Option<(String, u32)> {
    for stmt in &file_mod.statements {
        if let Some(result) = find_call_in_stmt(stmt, pos) {
            return Some(result);
        }
    }
    None
}

fn find_call_in_stmt(stmt: &sclc::ModStmt, pos: sclc::Position) -> Option<(String, u32)> {
    match stmt {
        sclc::ModStmt::Let(bind) | sclc::ModStmt::Export(bind) => {
            find_call_in_expr(&bind.expr, pos)
        }
        sclc::ModStmt::Expr(expr) => find_call_in_expr(expr, pos),
        sclc::ModStmt::Import(_) => None,
    }
}

fn find_call_in_expr(expr: &sclc::Loc<sclc::Expr>, pos: sclc::Position) -> Option<(String, u32)> {
    use sclc::Expr;

    let span = expr.span();
    if !(span.start() <= pos && pos <= span.end()) {
        return None;
    }

    match expr.as_ref() {
        Expr::Call(call) => {
            // Check if cursor is inside any argument first (recurse deeper).
            for arg in &call.args {
                if let Some(result) = find_call_in_expr(arg, pos) {
                    return Some(result);
                }
            }

            // The cursor is inside this call but not in a nested call.
            // Determine callee name.
            let callee_name = match call.callee.as_ref().as_ref() {
                Expr::Var(var) => var.name.clone(),
                _ => return None,
            };

            // Determine active parameter: count how many args come before the cursor.
            let mut active = 0u32;
            for arg in &call.args {
                if arg.span().end() < pos {
                    active += 1;
                }
            }

            Some((callee_name, active))
        }
        Expr::Let(let_expr) => find_call_in_expr(&let_expr.bind.expr, pos)
            .or_else(|| find_call_in_expr(&let_expr.expr, pos)),
        Expr::Fn(fn_expr) => find_call_in_expr(&fn_expr.body, pos),
        Expr::If(if_expr) => find_call_in_expr(&if_expr.condition, pos)
            .or_else(|| find_call_in_expr(&if_expr.then_expr, pos))
            .or_else(|| {
                if_expr
                    .else_expr
                    .as_ref()
                    .and_then(|e| find_call_in_expr(e, pos))
            }),
        Expr::Binary(bin) => {
            find_call_in_expr(&bin.lhs, pos).or_else(|| find_call_in_expr(&bin.rhs, pos))
        }
        Expr::Unary(unary) => find_call_in_expr(&unary.expr, pos),
        Expr::Record(record) => {
            for field in &record.fields {
                if let Some(r) = find_call_in_expr(&field.expr, pos) {
                    return Some(r);
                }
            }
            None
        }
        Expr::List(list) => {
            for item in &list.items {
                if let Some(r) = find_call_in_list_item(item, pos) {
                    return Some(r);
                }
            }
            None
        }
        Expr::Dict(dict) => {
            for entry in &dict.entries {
                if let Some(r) = find_call_in_expr(&entry.key, pos) {
                    return Some(r);
                }
                if let Some(r) = find_call_in_expr(&entry.value, pos) {
                    return Some(r);
                }
            }
            None
        }
        Expr::Interp(interp) => {
            for part in &interp.parts {
                if let Some(r) = find_call_in_expr(part, pos) {
                    return Some(r);
                }
            }
            None
        }
        Expr::PropertyAccess(prop) => find_call_in_expr(&prop.expr, pos),
        Expr::Try(try_expr) => {
            if let Some(r) = find_call_in_expr(&try_expr.expr, pos) {
                return Some(r);
            }
            for catch in &try_expr.catches {
                if let Some(r) = find_call_in_expr(&catch.body, pos) {
                    return Some(r);
                }
            }
            None
        }
        Expr::Raise(raise) => find_call_in_expr(&raise.expr, pos),
        _ => None,
    }
}

fn find_call_in_list_item(item: &sclc::ListItem, pos: sclc::Position) -> Option<(String, u32)> {
    match item {
        sclc::ListItem::Expr(expr) => find_call_in_expr(expr, pos),
        sclc::ListItem::If(if_item) => find_call_in_expr(&if_item.condition, pos)
            .or_else(|| find_call_in_list_item(&if_item.then_item, pos)),
        sclc::ListItem::For(for_item) => find_call_in_expr(&for_item.iterable, pos)
            .or_else(|| find_call_in_list_item(&for_item.emit_item, pos)),
    }
}

/// Get function parameter names from the AST if the callee is a global binding.
fn get_fn_param_names(file_mod: &sclc::FileMod, name: &str) -> Option<Vec<String>> {
    let globals = file_mod.find_globals();
    let expr = globals.get(name)?;
    if let sclc::Expr::Fn(fn_expr) = expr.as_ref() {
        Some(fn_expr.params.iter().map(|p| p.var.name.clone()).collect())
    } else {
        None
    }
}
