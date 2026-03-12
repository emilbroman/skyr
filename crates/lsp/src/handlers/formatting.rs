use lsp_types::{DocumentFormattingParams, TextEdit};
use sclc::SourceRepo;

use crate::convert::uri_to_path;
use crate::helpers::find_module_by_path;
use crate::{LanguageServer, OutgoingMessage, RequestId};

pub async fn handle_formatting<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: DocumentFormattingParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document.uri;

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(id, Option::<Vec<TextEdit>>::None)];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(id, Option::<Vec<TextEdit>>::None)];
    };

    let Some((_module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path)
    else {
        return vec![OutgoingMessage::response(id, Option::<Vec<TextEdit>>::None)];
    };

    let formatted = format_file_mod(file_mod);

    // Get the document content to compute the full range.
    let docs = server.documents.lock().await;
    let Some(current_text) = docs.get(&path) else {
        return vec![OutgoingMessage::response(id, Option::<Vec<TextEdit>>::None)];
    };

    let line_count = current_text.lines().count() as u32;
    let last_line_len = current_text.lines().last().map_or(0, |l| l.len()) as u32;

    let full_range = lsp_types::Range {
        start: lsp_types::Position {
            line: 0,
            character: 0,
        },
        end: lsp_types::Position {
            line: line_count,
            character: last_line_len,
        },
    };

    vec![OutgoingMessage::response(
        id,
        Some(vec![TextEdit {
            range: full_range,
            new_text: formatted,
        }]),
    )]
}

/// Format a file module back to source text.
fn format_file_mod(file_mod: &sclc::FileMod) -> String {
    let mut out = String::new();
    let mut first = true;

    for stmt in &file_mod.statements {
        if !first {
            out.push('\n');
        }
        first = false;
        format_stmt(&mut out, stmt, 0);
        out.push('\n');
    }

    out
}

fn format_stmt(out: &mut String, stmt: &sclc::ModStmt, indent: usize) {
    let prefix = "  ".repeat(indent);
    match stmt {
        sclc::ModStmt::Import(import) => {
            out.push_str(&prefix);
            out.push_str("import ");
            let path: Vec<&str> = import.vars.iter().map(|v| v.name.as_str()).collect();
            out.push_str(&path.join("/"));
        }
        sclc::ModStmt::Let(bind) => {
            out.push_str(&prefix);
            out.push_str("let ");
            out.push_str(&bind.var.name);
            out.push_str(" = ");
            format_expr(out, &bind.expr, indent);
        }
        sclc::ModStmt::Export(bind) => {
            out.push_str(&prefix);
            out.push_str("export ");
            out.push_str(&bind.var.name);
            out.push_str(" = ");
            format_expr(out, &bind.expr, indent);
        }
        sclc::ModStmt::Expr(expr) => {
            out.push_str(&prefix);
            format_expr(out, expr, indent);
        }
    }
}

fn format_expr(out: &mut String, expr: &sclc::Loc<sclc::Expr>, indent: usize) {
    use sclc::Expr;

    match expr.as_ref() {
        Expr::Int(i) => {
            out.push_str(&i.value.to_string());
        }
        Expr::Float(f) => {
            out.push_str(&f.value.to_string());
        }
        Expr::Bool(b) => {
            out.push_str(if b.value { "true" } else { "false" });
        }
        Expr::Nil => {
            out.push_str("nil");
        }
        Expr::Str(s) => {
            out.push('"');
            out.push_str(&s.value.replace('\\', "\\\\").replace('"', "\\\""));
            out.push('"');
        }
        Expr::Var(var) => {
            out.push_str(&var.name);
        }
        Expr::Fn(fn_expr) => {
            out.push_str("fn");
            if !fn_expr.type_params.is_empty() {
                out.push('<');
                for (i, tp) in fn_expr.type_params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&tp.var.name);
                    if let Some(bound) = &tp.bound {
                        out.push_str(" <: ");
                        format_type_expr(out, bound);
                    }
                }
                out.push('>');
            }
            out.push('(');
            for (i, param) in fn_expr.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&param.var.name);
                out.push_str(": ");
                format_type_expr(out, &param.ty);
            }
            out.push_str(") ");
            format_expr(out, &fn_expr.body, indent);
        }
        Expr::Call(call) => {
            format_expr(out, &call.callee, indent);
            if !call.type_args.is_empty() {
                out.push('<');
                for (i, ta) in call.type_args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    format_type_expr(out, ta);
                }
                out.push('>');
            }
            out.push('(');
            for (i, arg) in call.args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(out, arg, indent);
            }
            out.push(')');
        }
        Expr::Let(let_expr) => {
            out.push_str("let ");
            out.push_str(&let_expr.bind.var.name);
            out.push_str(" = ");
            format_expr(out, &let_expr.bind.expr, indent);
            out.push('\n');
            let prefix = "  ".repeat(indent);
            out.push_str(&prefix);
            format_expr(out, &let_expr.expr, indent);
        }
        Expr::If(if_expr) => {
            out.push_str("if ");
            format_expr(out, &if_expr.condition, indent);
            out.push_str(" then ");
            format_expr(out, &if_expr.then_expr, indent);
            if let Some(else_expr) = &if_expr.else_expr {
                out.push_str(" else ");
                format_expr(out, else_expr, indent);
            }
        }
        Expr::Binary(bin) => {
            format_expr(out, &bin.lhs, indent);
            out.push(' ');
            out.push_str(&bin.op.to_string());
            out.push(' ');
            format_expr(out, &bin.rhs, indent);
        }
        Expr::Unary(unary) => {
            out.push_str(&unary.op.to_string());
            format_expr(out, &unary.expr, indent);
        }
        Expr::Record(record) => {
            if record.fields.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{\n");
                let inner_indent = indent + 1;
                let inner_prefix = "  ".repeat(inner_indent);
                for (i, field) in record.fields.iter().enumerate() {
                    out.push_str(&inner_prefix);
                    out.push_str(&field.var.name);
                    out.push_str(": ");
                    format_expr(out, &field.expr, inner_indent);
                    if i < record.fields.len() - 1 {
                        out.push(',');
                    }
                    out.push('\n');
                }
                let prefix = "  ".repeat(indent);
                out.push_str(&prefix);
                out.push('}');
            }
        }
        Expr::Dict(dict) => {
            if dict.entries.is_empty() {
                out.push_str("{:}");
            } else {
                out.push_str("{\n");
                let inner_indent = indent + 1;
                let inner_prefix = "  ".repeat(inner_indent);
                for (i, entry) in dict.entries.iter().enumerate() {
                    out.push_str(&inner_prefix);
                    format_expr(out, &entry.key, inner_indent);
                    out.push_str(": ");
                    format_expr(out, &entry.value, inner_indent);
                    if i < dict.entries.len() - 1 {
                        out.push(',');
                    }
                    out.push('\n');
                }
                let prefix = "  ".repeat(indent);
                out.push_str(&prefix);
                out.push('}');
            }
        }
        Expr::List(list) => {
            if list.items.is_empty() {
                out.push_str("[]");
            } else {
                out.push('[');
                for (i, item) in list.items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    format_list_item(out, item, indent);
                }
                out.push(']');
            }
        }
        Expr::Interp(interp) => {
            out.push('"');
            for part in &interp.parts {
                match part.as_ref() {
                    Expr::Str(s) => {
                        out.push_str(&s.value.replace('\\', "\\\\").replace('"', "\\\""));
                    }
                    _ => {
                        out.push_str("${");
                        format_expr(out, part, indent);
                        out.push('}');
                    }
                }
            }
            out.push('"');
        }
        Expr::PropertyAccess(prop) => {
            format_expr(out, &prop.expr, indent);
            out.push('.');
            out.push_str(&prop.property.name);
        }
        Expr::Extern(ext) => {
            out.push_str("extern \"");
            out.push_str(&ext.name);
            out.push_str("\": ");
            format_type_expr(out, &ext.ty);
        }
        Expr::Exception(exc) => {
            out.push_str(&format!("exception {}", exc.exception_id));
            if let Some(ty) = &exc.ty {
                out.push('(');
                format_type_expr(out, ty);
                out.push(')');
            }
        }
        Expr::Raise(raise) => {
            out.push_str("raise ");
            format_expr(out, &raise.expr, indent);
        }
        Expr::Try(try_expr) => {
            out.push_str("try ");
            format_expr(out, &try_expr.expr, indent);
            for catch in &try_expr.catches {
                out.push_str(" catch ");
                out.push_str(&catch.exception_var.name);
                if let Some(arg) = &catch.catch_arg {
                    out.push('(');
                    out.push_str(&arg.name);
                    out.push(')');
                }
                out.push(' ');
                format_expr(out, &catch.body, indent);
            }
        }
    }
}

fn format_list_item(out: &mut String, item: &sclc::ListItem, indent: usize) {
    match item {
        sclc::ListItem::Expr(expr) => format_expr(out, expr, indent),
        sclc::ListItem::If(if_item) => {
            out.push_str("if ");
            format_expr(out, &if_item.condition, indent);
            out.push(' ');
            format_list_item(out, &if_item.then_item, indent);
        }
        sclc::ListItem::For(for_item) => {
            out.push_str("for ");
            out.push_str(&for_item.var.name);
            out.push_str(" in ");
            format_expr(out, &for_item.iterable, indent);
            out.push(' ');
            format_list_item(out, &for_item.emit_item, indent);
        }
    }
}

fn format_type_expr(out: &mut String, type_expr: &sclc::Loc<sclc::TypeExpr>) {
    use sclc::TypeExpr;

    match type_expr.as_ref() {
        TypeExpr::Var(var) => {
            out.push_str(&var.name);
        }
        TypeExpr::Optional(inner) => {
            format_type_expr(out, inner);
            out.push('?');
        }
        TypeExpr::List(inner) => {
            out.push('[');
            format_type_expr(out, inner);
            out.push(']');
        }
        TypeExpr::Fn(fn_ty) => {
            out.push_str("fn");
            if !fn_ty.type_params.is_empty() {
                out.push('<');
                for (i, tp) in fn_ty.type_params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&tp.var.name);
                    if let Some(bound) = &tp.bound {
                        out.push_str(" <: ");
                        format_type_expr(out, bound);
                    }
                }
                out.push('>');
            }
            out.push('(');
            for (i, param) in fn_ty.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_type_expr(out, param);
            }
            out.push_str(") ");
            format_type_expr(out, &fn_ty.ret);
        }
        TypeExpr::Record(record) => {
            out.push_str("{ ");
            for (i, field) in record.fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&field.var.name);
                out.push_str(": ");
                format_type_expr(out, &field.ty);
            }
            out.push_str(" }");
        }
        TypeExpr::Dict(dict) => {
            out.push('{');
            format_type_expr(out, &dict.key);
            out.push_str(": ");
            format_type_expr(out, &dict.value);
            out.push('}');
        }
    }
}
