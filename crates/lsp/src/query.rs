use sclc::{Expr, FileMod, LetBind, Loc, ModStmt, Position, Span, Var};

/// Whether a span contains a given position (inclusive on both ends).
fn span_contains(span: Span, pos: Position) -> bool {
    span.start() <= pos && pos <= span.end()
}

/// The result of looking up what's at a cursor position.
#[allow(dead_code)]
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
                if let Some(n) = node_in_expr_ref(&field.expr, pos) {
                    return Some(n);
                }
            }
            Some(NodeAtPosition::Expr(expr))
        }

        Expr::Dict(dict) => {
            for entry in &dict.entries {
                if let Some(n) = node_in_expr_ref(&entry.key, pos) {
                    return Some(n);
                }
                if let Some(n) = node_in_expr_ref(&entry.value, pos) {
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
                if let Some(n) = node_in_expr_ref(&catch.body, pos) {
                    return Some(n);
                }
            }
            Some(NodeAtPosition::Expr(expr))
        }

        // Leaf expressions: Int, Float, Bool, Nil, Str, Extern, Exception
        _ => Some(NodeAtPosition::Expr(expr)),
    }
}

/// Like `node_in_expr` but for a `Loc<Expr>` by reference (not behind Box).
fn node_in_expr_ref(expr: &Loc<Expr>, pos: Position) -> Option<NodeAtPosition<'_>> {
    if !span_contains(expr.span(), pos) {
        return None;
    }
    // Re-use the same logic. We need a small trick since Loc<Expr> and &Loc<Expr> are the same.
    node_in_expr(expr, pos)
}

fn node_in_list_item<'a>(item: &'a sclc::ListItem, pos: Position) -> Option<NodeAtPosition<'a>> {
    match item {
        sclc::ListItem::Expr(expr) => node_in_expr_ref(expr, pos),
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
