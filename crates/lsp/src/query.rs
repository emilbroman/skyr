use sclc::{Expr, FileMod, LetBind, Loc, ModStmt, Position, Span, Var};

/// Whether a span contains a given position (inclusive on both ends).
fn span_contains(span: Span, pos: Position) -> bool {
    span.start() <= pos && pos <= span.end()
}

/// The result of looking up what's at a cursor position.
pub enum NodeAtPosition<'a> {
    /// A variable reference in an expression.
    Var(&'a Loc<Var>),
    /// A top-level let/export binding's name.
    LetBindVar(&'a LetBind),
    /// A property access (e.g., `foo.bar` — the `bar` part).
    Property {
        expr: &'a Loc<Expr>,
        property: &'a Loc<Var>,
    },
    /// An expression (fallback when no more specific node matches).
    #[allow(dead_code)]
    // inner value reserved for future use (e.g. hover on arbitrary expressions)
    Expr(&'a Loc<Expr>),
}

/// Find the most specific AST node at the given position.
pub fn node_at_position(file_mod: &FileMod, pos: Position) -> Option<NodeAtPosition<'_>> {
    for stmt in &file_mod.statements {
        if let Some(node) = node_in_stmt(stmt, pos) {
            return Some(node);
        }
    }
    None
}

fn node_in_stmt<'a>(stmt: &'a ModStmt, pos: Position) -> Option<NodeAtPosition<'a>> {
    match stmt {
        ModStmt::Import(import) => {
            if span_contains(import.span(), pos) {
                // Return the last var segment (the alias) as the interesting one
                for var in &import.vars {
                    if span_contains(var.span(), pos) {
                        return Some(NodeAtPosition::Var(var));
                    }
                }
            }
            None
        }
        ModStmt::Let(bind) | ModStmt::Export(bind) => {
            if span_contains(bind.var.span(), pos) {
                return Some(NodeAtPosition::LetBindVar(bind));
            }
            node_in_expr(&bind.expr, pos)
        }
        ModStmt::Expr(expr) => node_in_expr(expr, pos),
    }
}

fn node_in_expr<'a>(expr: &'a Loc<Expr>, pos: Position) -> Option<NodeAtPosition<'a>> {
    if !span_contains(expr.span(), pos) {
        return None;
    }

    match expr.as_ref() {
        Expr::Var(var) => Some(NodeAtPosition::Var(var)),

        Expr::PropertyAccess(prop) => {
            if span_contains(prop.property.span(), pos) {
                return Some(NodeAtPosition::Property {
                    expr: &prop.expr,
                    property: &prop.property,
                });
            }
            node_in_expr(&prop.expr, pos)
        }

        Expr::Let(let_expr) => {
            if span_contains(let_expr.bind.var.span(), pos) {
                return Some(NodeAtPosition::LetBindVar(&let_expr.bind));
            }
            node_in_expr(&let_expr.bind.expr, pos).or_else(|| node_in_expr(&let_expr.expr, pos))
        }

        Expr::Fn(fn_expr) => {
            for param in &fn_expr.params {
                if span_contains(param.var.span(), pos) {
                    return Some(NodeAtPosition::Var(&param.var));
                }
            }
            node_in_expr(&fn_expr.body, pos)
        }

        Expr::Call(call_expr) => {
            if let Some(n) = node_in_expr(&call_expr.callee, pos) {
                return Some(n);
            }
            for arg in &call_expr.args {
                if let Some(n) = node_in_expr(arg, pos) {
                    return Some(n);
                }
            }
            Some(NodeAtPosition::Expr(expr))
        }

        Expr::If(if_expr) => node_in_expr(&if_expr.condition, pos)
            .or_else(|| node_in_expr(&if_expr.then_expr, pos))
            .or_else(|| {
                if_expr
                    .else_expr
                    .as_ref()
                    .and_then(|e| node_in_expr(e, pos))
            }),

        Expr::Binary(bin) => node_in_expr(&bin.lhs, pos).or_else(|| node_in_expr(&bin.rhs, pos)),

        Expr::Unary(unary) => node_in_expr(&unary.expr, pos),

        Expr::Record(record) => {
            for field in &record.fields {
                if let Some(n) = node_in_expr(&field.expr, pos) {
                    return Some(n);
                }
            }
            Some(NodeAtPosition::Expr(expr))
        }

        Expr::Dict(dict) => {
            for entry in &dict.entries {
                if let Some(n) = node_in_expr(&entry.key, pos) {
                    return Some(n);
                }
                if let Some(n) = node_in_expr(&entry.value, pos) {
                    return Some(n);
                }
            }
            Some(NodeAtPosition::Expr(expr))
        }

        Expr::List(list) => {
            for item in &list.items {
                if let Some(n) = node_in_list_item(item, pos) {
                    return Some(n);
                }
            }
            Some(NodeAtPosition::Expr(expr))
        }

        Expr::Interp(interp) => {
            for part in &interp.parts {
                if let Some(n) = node_in_expr(part, pos) {
                    return Some(n);
                }
            }
            Some(NodeAtPosition::Expr(expr))
        }

        Expr::Raise(raise) => node_in_expr(&raise.expr, pos),

        Expr::Try(try_expr) => {
            if let Some(n) = node_in_expr(&try_expr.expr, pos) {
                return Some(n);
            }
            for catch in &try_expr.catches {
                if span_contains(catch.exception_var.span(), pos) {
                    return Some(NodeAtPosition::Var(&catch.exception_var));
                }
                if let Some(catch_arg) = &catch.catch_arg
                    && span_contains(catch_arg.span(), pos)
                {
                    return Some(NodeAtPosition::Var(catch_arg));
                }
                if let Some(n) = node_in_expr(&catch.body, pos) {
                    return Some(n);
                }
            }
            Some(NodeAtPosition::Expr(expr))
        }

        // Leaf expressions: Int, Float, Bool, Nil, Str, Extern, Exception
        _ => Some(NodeAtPosition::Expr(expr)),
    }
}

/// Collect all top-level variable references with the given name in a file module.
///
/// This finds `Expr::Var` nodes whose name matches, as well as let/export binding
/// sites. It does **not** perform scope analysis — it collects all textual matches,
/// which is a reasonable approximation for single-file references of globals/imports.
pub fn find_var_references(file_mod: &FileMod, name: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    for stmt in &file_mod.statements {
        collect_refs_in_stmt(stmt, name, &mut spans);
    }
    spans
}

fn collect_refs_in_stmt(stmt: &ModStmt, name: &str, spans: &mut Vec<Span>) {
    match stmt {
        ModStmt::Import(import) => {
            if let Some(last_var) = import.vars.last()
                && last_var.name == name
            {
                spans.push(last_var.span());
            }
        }
        ModStmt::Let(bind) | ModStmt::Export(bind) => {
            if bind.var.name == name {
                spans.push(bind.var.span());
            }
            collect_refs_in_expr(&bind.expr, name, spans);
        }
        ModStmt::Expr(expr) => {
            collect_refs_in_expr(expr, name, spans);
        }
    }
}

fn collect_refs_in_expr(expr: &Loc<Expr>, name: &str, spans: &mut Vec<Span>) {
    match expr.as_ref() {
        Expr::Var(var) if var.name == name => {
            spans.push(var.span());
        }
        Expr::Var(_) => {}
        Expr::Let(let_expr) => {
            if let_expr.bind.var.name == name {
                spans.push(let_expr.bind.var.span());
            }
            collect_refs_in_expr(&let_expr.bind.expr, name, spans);
            // If the let rebinds the name, references in the body refer to the local,
            // not the outer definition. For simplicity, we still collect them.
            collect_refs_in_expr(&let_expr.expr, name, spans);
        }
        Expr::Fn(fn_expr) => {
            // If a parameter shadows the name, skip the body.
            if fn_expr.params.iter().any(|p| p.var.name == name) {
                return;
            }
            collect_refs_in_expr(&fn_expr.body, name, spans);
        }
        Expr::Call(call) => {
            collect_refs_in_expr(&call.callee, name, spans);
            for arg in &call.args {
                collect_refs_in_expr(arg, name, spans);
            }
        }
        Expr::If(if_expr) => {
            collect_refs_in_expr(&if_expr.condition, name, spans);
            collect_refs_in_expr(&if_expr.then_expr, name, spans);
            if let Some(else_expr) = &if_expr.else_expr {
                collect_refs_in_expr(else_expr, name, spans);
            }
        }
        Expr::Binary(bin) => {
            collect_refs_in_expr(&bin.lhs, name, spans);
            collect_refs_in_expr(&bin.rhs, name, spans);
        }
        Expr::Unary(unary) => {
            collect_refs_in_expr(&unary.expr, name, spans);
        }
        Expr::Record(record) => {
            for field in &record.fields {
                collect_refs_in_expr(&field.expr, name, spans);
            }
        }
        Expr::Dict(dict) => {
            for entry in &dict.entries {
                collect_refs_in_expr(&entry.key, name, spans);
                collect_refs_in_expr(&entry.value, name, spans);
            }
        }
        Expr::List(list) => {
            for item in &list.items {
                collect_refs_in_list_item(item, name, spans);
            }
        }
        Expr::Interp(interp) => {
            for part in &interp.parts {
                collect_refs_in_expr(part, name, spans);
            }
        }
        Expr::PropertyAccess(prop) => {
            collect_refs_in_expr(&prop.expr, name, spans);
        }
        Expr::Raise(raise) => {
            collect_refs_in_expr(&raise.expr, name, spans);
        }
        Expr::Try(try_expr) => {
            collect_refs_in_expr(&try_expr.expr, name, spans);
            for catch in &try_expr.catches {
                collect_refs_in_expr(&catch.body, name, spans);
            }
        }
        // Leaf nodes: Int, Float, Bool, Nil, Str, Extern, Exception
        _ => {}
    }
}

fn collect_refs_in_list_item(item: &sclc::ListItem, name: &str, spans: &mut Vec<Span>) {
    match item {
        sclc::ListItem::Expr(expr) => collect_refs_in_expr(expr, name, spans),
        sclc::ListItem::If(if_item) => {
            collect_refs_in_expr(&if_item.condition, name, spans);
            collect_refs_in_list_item(&if_item.then_item, name, spans);
        }
        sclc::ListItem::For(for_item) => {
            collect_refs_in_expr(&for_item.iterable, name, spans);
            // If the for-variable shadows the name, skip the body.
            if for_item.var.name != name {
                collect_refs_in_list_item(&for_item.emit_item, name, spans);
            }
        }
    }
}

fn node_in_list_item<'a>(item: &'a sclc::ListItem, pos: Position) -> Option<NodeAtPosition<'a>> {
    match item {
        sclc::ListItem::Expr(expr) => node_in_expr(expr, pos),
        sclc::ListItem::If(if_item) => node_in_expr(&if_item.condition, pos)
            .or_else(|| node_in_list_item(&if_item.then_item, pos)),
        sclc::ListItem::For(for_item) => {
            if span_contains(for_item.var.span(), pos) {
                return Some(NodeAtPosition::Var(&for_item.var));
            }
            node_in_expr(&for_item.iterable, pos)
                .or_else(|| node_in_list_item(&for_item.emit_item, pos))
        }
    }
}
