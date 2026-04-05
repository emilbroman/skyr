use crate::{BinaryOp, Lexer, Loc, Position, Span, Token, TypeExpr, ast::*};

/// A comment extracted from the source, with its location.
#[derive(Debug, Clone)]
struct Comment {
    span: Span,
    text: String,
}

/// Collect all comments from source by lexing it.
fn collect_comments(source: &str) -> Vec<Comment> {
    Lexer::new(source)
        .filter_map(|tok| match *tok.as_ref() {
            Token::Comment(text) => Some(Comment {
                span: tok.span(),
                text: text.to_owned(),
            }),
            _ => None,
        })
        .collect()
}

fn encode_string(s: &str) -> String {
    crate::string_escape::encode_string(s)
}

pub struct Formatter {
    comments: Vec<Comment>,
    /// Index of the next comment to potentially emit.
    comment_cursor: usize,
    output: String,
    indent: usize,
    /// Whether we're at the beginning of a line (for indent purposes).
    at_line_start: bool,
}

impl Formatter {
    pub fn format(source: &str, file_mod: &FileMod) -> String {
        let comments = collect_comments(source);
        let mut f = Formatter {
            comments,
            comment_cursor: 0,
            output: String::new(),
            indent: 0,
            at_line_start: true,
        };
        f.emit_file_mod(file_mod);
        f.emit_remaining_comments();
        // Ensure file ends with a single newline
        let trimmed = f.output.trim_end().to_owned();
        if trimmed.is_empty() {
            trimmed
        } else {
            trimmed + "\n"
        }
    }

    // ── helpers ──────────────────────────────────────────────────────

    fn write(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.at_line_start {
            for _ in 0..self.indent {
                self.output.push('\t');
            }
            self.at_line_start = false;
        }
        self.output.push_str(s);
    }

    fn newline(&mut self) {
        self.output.push('\n');
        self.at_line_start = true;
    }

    fn space(&mut self) {
        self.write(" ");
    }

    fn indent(&mut self) {
        self.indent += 1;
    }

    fn dedent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    /// Emit any comments whose start position is before `pos`.
    fn emit_comments_before(&mut self, pos: Position) {
        while self.comment_cursor < self.comments.len() {
            let comment = &self.comments[self.comment_cursor];
            if comment.span.start() < pos {
                let text = comment.text.clone();
                self.write(&text);
                self.newline();
                self.comment_cursor += 1;
            } else {
                break;
            }
        }
    }

    /// Emit any comments that appear between two positions (exclusive on both ends
    /// is fine — we use "after prev, before next").
    fn emit_comments_between(&mut self, after: Position, before: Position) {
        while self.comment_cursor < self.comments.len() {
            let comment = &self.comments[self.comment_cursor];
            if comment.span.start() >= after && comment.span.start() < before {
                let text = comment.text.clone();
                self.write(&text);
                self.newline();
                self.comment_cursor += 1;
            } else {
                break;
            }
        }
    }

    /// Emit inline comment on the same line if one exists between `after` and `before`.
    fn emit_trailing_comment(&mut self, after: Position, before: Position) {
        if self.comment_cursor < self.comments.len() {
            let comment = &self.comments[self.comment_cursor];
            // Trailing comment: on the same line as `after`
            if comment.span.start().line() == after.line()
                && comment.span.start() >= after
                && comment.span.start() < before
            {
                let text = comment.text.clone();
                self.space();
                self.write(&text);
                self.comment_cursor += 1;
            }
        }
    }

    fn emit_remaining_comments(&mut self) {
        while self.comment_cursor < self.comments.len() {
            let text = self.comments[self.comment_cursor].text.clone();
            self.write(&text);
            self.newline();
            self.comment_cursor += 1;
        }
    }

    // ── file / module ───────────────────────────────────────────────

    fn emit_file_mod(&mut self, file_mod: &FileMod) {
        let stmts = &file_mod.statements;
        for (i, stmt) in stmts.iter().enumerate() {
            let stmt_start = self.mod_stmt_start(stmt);
            if i == 0 {
                self.emit_comments_before(stmt_start);
            } else {
                let prev_end = self.mod_stmt_end(&stmts[i - 1]);
                // Blank line between top-level statements
                self.newline();
                self.emit_comments_between(prev_end, stmt_start);
            }
            self.emit_mod_stmt(stmt);
            self.newline();
        }
    }

    fn mod_stmt_start(&self, stmt: &ModStmt) -> Position {
        match stmt {
            ModStmt::Import(import) => import.span().start(),
            ModStmt::Let(bind) => bind.var.span().start(),
            ModStmt::Export(bind) => bind.var.span().start(),
            ModStmt::TypeDef(td) => td.var.span().start(),
            ModStmt::ExportTypeDef(td) => td.var.span().start(),
            ModStmt::Expr(expr) => expr.span().start(),
        }
    }

    fn mod_stmt_end(&self, stmt: &ModStmt) -> Position {
        match stmt {
            ModStmt::Import(import) => import.span().end(),
            ModStmt::Let(bind) => bind.expr.span().end(),
            ModStmt::Export(bind) => bind.expr.span().end(),
            ModStmt::TypeDef(td) => td.ty.span().end(),
            ModStmt::ExportTypeDef(td) => td.ty.span().end(),
            ModStmt::Expr(expr) => expr.span().end(),
        }
    }

    fn emit_mod_stmt(&mut self, stmt: &ModStmt) {
        match stmt {
            ModStmt::Import(import) => self.emit_import(import),
            ModStmt::Let(bind) => {
                self.emit_doc_comment(&bind.doc_comment);
                self.write("let ");
                self.emit_let_bind(bind);
            }
            ModStmt::Export(bind) => {
                self.emit_doc_comment(&bind.doc_comment);
                self.write("export let ");
                self.emit_let_bind(bind);
            }
            ModStmt::TypeDef(td) => {
                self.emit_doc_comment(&td.doc_comment);
                self.write("type ");
                self.emit_type_def(td);
            }
            ModStmt::ExportTypeDef(td) => {
                self.emit_doc_comment(&td.doc_comment);
                self.write("export type ");
                self.emit_type_def(td);
            }
            ModStmt::Expr(expr) => self.emit_expr(expr),
        }
    }

    fn emit_import(&mut self, import: &Loc<ImportStmt>) {
        self.write("import ");
        for (i, var) in import.vars.iter().enumerate() {
            if i > 0 {
                self.write("/");
            }
            self.write(&var.name);
        }
    }

    fn emit_doc_comment(&mut self, doc: &Option<String>) {
        if let Some(doc) = doc {
            for line in doc.lines() {
                if line.is_empty() {
                    self.write("///");
                } else {
                    self.write("/// ");
                    self.write(line);
                }
                self.newline();
            }
        }
    }

    fn emit_let_bind(&mut self, bind: &LetBind) {
        self.write(&bind.var.name);
        if let Some(ty) = &bind.ty {
            self.write(": ");
            self.emit_type_expr(ty);
        }
        self.write(" = ");
        self.emit_expr(&bind.expr);
    }

    fn emit_type_def(&mut self, td: &TypeDef) {
        self.write(&td.var.name);
        self.emit_type_params(&td.type_params);
        self.space();
        self.emit_type_expr(&td.ty);
    }

    // ── type expressions ────────────────────────────────────────────

    fn emit_type_params(&mut self, params: &[TypeParam]) {
        if params.is_empty() {
            return;
        }
        self.write("<");
        for (i, param) in params.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.write(&param.var.name);
            if let Some(bound) = &param.bound {
                self.write(" <: ");
                self.emit_type_expr(bound);
            }
        }
        self.write(">");
    }

    fn emit_type_expr(&mut self, ty: &Loc<TypeExpr>) {
        match ty.as_ref() {
            TypeExpr::Var(var) => self.write(&var.name),
            TypeExpr::Optional(inner) => {
                self.emit_type_expr(inner);
                self.write("?");
            }
            TypeExpr::List(inner) => {
                self.write("[");
                self.emit_type_expr(inner);
                self.write("]");
            }
            TypeExpr::Fn(fn_ty) => {
                self.write("fn");
                self.emit_type_params(&fn_ty.type_params);
                self.write("(");
                for (i, param) in fn_ty.params.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.emit_type_expr(param);
                }
                self.write(") ");
                self.emit_type_expr(&fn_ty.ret);
            }
            TypeExpr::Record(record) => {
                self.emit_record_type(record, ty.span());
            }
            TypeExpr::Dict(dict) => {
                self.write("#{");
                self.emit_type_expr(&dict.key);
                self.write(": ");
                self.emit_type_expr(&dict.value);
                self.write("}");
            }
            TypeExpr::PropertyAccess(access) => {
                self.emit_type_expr(&access.expr);
                self.write(".");
                self.write(&access.property.name);
            }
            TypeExpr::Application(app) => {
                self.emit_type_expr(&app.base);
                self.write("<");
                for (i, arg) in app.args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.emit_type_expr(arg);
                }
                self.write(">");
            }
        }
    }

    fn emit_record_type(&mut self, record: &RecordTypeExpr, span: Span) {
        if record.fields.is_empty() {
            self.write("{}");
            return;
        }
        self.write("{");
        self.newline();
        self.indent();
        for (i, field) in record.fields.iter().enumerate() {
            if i > 0 {
                let prev_end = record.fields[i - 1].ty.span().end();
                self.emit_comments_between(prev_end, field.var.span().start());
            } else {
                self.emit_comments_before(field.var.span().start());
            }
            self.emit_doc_comment(&field.doc_comment);
            self.write(&field.var.name);
            self.write(": ");
            self.emit_type_expr(&field.ty);
            self.write(",");
            // Trailing comment after the field
            let next_start = record
                .fields
                .get(i + 1)
                .map(|f| f.var.span().start())
                .unwrap_or(span.end());
            self.emit_trailing_comment(field.ty.span().end(), next_start);
            self.newline();
        }
        self.dedent();
        self.write("}");
    }

    // ── expressions ─────────────────────────────────────────────────

    fn emit_expr(&mut self, expr: &Loc<Expr>) {
        self.emit_expr_inner(expr, false);
    }

    fn emit_expr_inner(&mut self, expr: &Loc<Expr>, in_parens: bool) {
        match expr.as_ref() {
            Expr::Int(int) => self.write(&int.value.to_string()),
            Expr::Float(float) => self.write(&float.value.to_string()),
            Expr::Bool(b) => self.write(if b.value { "true" } else { "false" }),
            Expr::Nil => self.write("nil"),
            Expr::Str(s) => {
                self.write("\"");
                self.write(&encode_string(&s.value));
                self.write("\"");
            }
            Expr::Path(p) => {
                let mut first = true;
                for segment in &p.value {
                    if !first {
                        self.write("/");
                    }
                    first = false;
                    let needs_quoting = segment != "."
                        && segment != ".."
                        && !segment
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-');
                    if needs_quoting {
                        self.write("\"");
                        self.write(&encode_string(segment));
                        self.write("\"");
                    } else {
                        self.write(segment);
                    }
                }
            }
            Expr::Interp(interp) => self.emit_interp(interp),
            Expr::Var(var) => self.write(&var.name),
            Expr::Unary(unary) => {
                self.write(&unary.op.to_string());
                self.emit_expr(&unary.expr);
            }
            Expr::Binary(binary) => self.emit_binary(binary, in_parens),
            Expr::If(if_expr) => self.emit_if(if_expr),
            Expr::Let(let_expr) => self.emit_let_expr(let_expr),
            Expr::Fn(fn_expr) => self.emit_fn(fn_expr),
            Expr::Call(call) => self.emit_call(call),
            Expr::Record(record) => self.emit_record(record, expr.span()),
            Expr::Dict(dict) => self.emit_dict(dict, expr.span()),
            Expr::List(list) => self.emit_list(list, expr.span()),
            Expr::PropertyAccess(access) => {
                self.emit_expr(&access.expr);
                if access.optional {
                    self.write("?.");
                } else {
                    self.write(".");
                }
                self.write(&access.property.name);
            }
            Expr::TypeCast(cast) => {
                self.emit_expr(&cast.expr);
                self.write(" as ");
                self.emit_type_expr(&cast.ty);
            }
            Expr::Extern(ext) => self.emit_extern(ext),
            Expr::Exception(exc) => self.emit_exception(exc),
            Expr::Raise(raise) => {
                self.write("raise ");
                self.emit_expr(&raise.expr);
            }
            Expr::Try(try_expr) => self.emit_try(try_expr),
            Expr::IndexedAccess(indexed_access) => {
                self.emit_expr(&indexed_access.expr);
                self.write("[");
                self.emit_expr(&indexed_access.index);
                self.write("]");
            }
        }
    }

    fn emit_interp(&mut self, interp: &InterpExpr) {
        // InterpExpr parts alternate: Str, Expr, Str, Expr, ..., Str
        self.write("\"");
        for part in interp.parts.iter() {
            match part.as_ref() {
                Expr::Str(s) => {
                    self.write(&encode_string(&s.value));
                }
                _ => {
                    // This is an interpolated expression
                    self.write("{");
                    self.emit_expr(part);
                    self.write("}");
                }
            }
        }
        self.write("\"");
    }

    fn binary_precedence(op: &BinaryOp) -> u8 {
        match op {
            BinaryOp::Or => 1,
            BinaryOp::NilCoalesce => 2,
            BinaryOp::And => 3,
            BinaryOp::Eq | BinaryOp::Neq => 4,
            BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => 5,
            BinaryOp::Add | BinaryOp::Sub => 6,
            BinaryOp::Mul | BinaryOp::Div => 7,
        }
    }

    fn emit_binary(&mut self, binary: &BinaryExpr, in_parens: bool) {
        let prec = Self::binary_precedence(&binary.op);

        // Check if LHS needs parens
        let lhs_needs_parens = match &**binary.lhs.as_ref() {
            Expr::Binary(lhs_bin) => Self::binary_precedence(&lhs_bin.op) < prec,
            _ => false,
        };

        if lhs_needs_parens {
            self.write("(");
            self.emit_expr_inner(&binary.lhs, true);
            self.write(")");
        } else {
            self.emit_expr_inner(&binary.lhs, in_parens);
        }

        self.write(" ");
        self.write(&binary.op.to_string());
        self.write(" ");

        // Check if RHS needs parens
        let rhs_needs_parens = match &**binary.rhs.as_ref() {
            Expr::Binary(rhs_bin) => Self::binary_precedence(&rhs_bin.op) <= prec,
            _ => false,
        };

        if rhs_needs_parens {
            self.write("(");
            self.emit_expr_inner(&binary.rhs, true);
            self.write(")");
        } else {
            self.emit_expr_inner(&binary.rhs, in_parens);
        }
    }

    fn emit_if(&mut self, if_expr: &IfExpr) {
        self.write("if (");
        self.emit_expr(&if_expr.condition);
        self.write(")");
        self.newline();
        self.indent();
        self.emit_expr(&if_expr.then_expr);
        self.dedent();
        if let Some(else_expr) = &if_expr.else_expr {
            self.newline();
            self.write("else");
            // Check if the else branch is itself an if expression
            if matches!(&**else_expr.as_ref(), Expr::If(_)) {
                self.space();
                self.emit_expr(else_expr);
            } else {
                self.newline();
                self.indent();
                self.emit_expr(else_expr);
                self.dedent();
            }
        }
    }

    fn emit_let_expr(&mut self, let_expr: &LetExpr) {
        self.write("let ");
        self.emit_let_bind(&let_expr.bind);
        self.write(";");
        self.newline();
        self.emit_expr(&let_expr.expr);
    }

    fn emit_fn(&mut self, fn_expr: &FnExpr) {
        self.write("fn");
        self.emit_type_params(&fn_expr.type_params);
        self.write("(");
        for (i, param) in fn_expr.params.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.write(&param.var.name);
            if let Some(ty) = &param.ty {
                self.write(": ");
                self.emit_type_expr(ty);
            }
        }
        self.write(")");
        self.newline();
        self.indent();
        self.emit_expr(&fn_expr.body);
        self.dedent();
    }

    fn emit_call(&mut self, call: &CallExpr) {
        self.emit_expr(&call.callee);
        if !call.type_args.is_empty() {
            self.write("<");
            for (i, arg) in call.type_args.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.emit_type_expr(arg);
            }
            self.write(">");
        }
        self.write("(");
        for (i, arg) in call.args.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.emit_expr(arg);
        }
        self.write(")");
    }

    fn emit_record(&mut self, record: &RecordExpr, span: Span) {
        if record.fields.is_empty() {
            self.write("{}");
            return;
        }
        self.write("{");
        self.newline();
        self.indent();
        for (i, field) in record.fields.iter().enumerate() {
            if i > 0 {
                let prev_end = record.fields[i - 1].expr.span().end();
                self.emit_comments_between(prev_end, field.var.span().start());
            }
            self.emit_doc_comment(&field.doc_comment);
            // Detect shorthand: if the field expression is a Var with the same name as the field
            let is_shorthand =
                matches!(field.expr.as_ref(), Expr::Var(v) if v.name == field.var.name);
            if is_shorthand {
                self.write(&field.var.name);
            } else {
                self.write(&field.var.name);
                self.write(": ");
                self.emit_expr(&field.expr);
            }
            self.write(",");
            // Trailing comment
            let next_start = record
                .fields
                .get(i + 1)
                .map(|f| f.var.span().start())
                .unwrap_or(span.end());
            self.emit_trailing_comment(field.expr.span().end(), next_start);
            self.newline();
        }
        self.dedent();
        self.write("}");
    }

    fn emit_dict(&mut self, dict: &DictExpr, span: Span) {
        if dict.entries.is_empty() {
            self.write("#{}");
            return;
        }
        self.write("#{");
        self.newline();
        self.indent();
        for (i, entry) in dict.entries.iter().enumerate() {
            if i > 0 {
                let prev_end = dict.entries[i - 1].value.span().end();
                self.emit_comments_between(prev_end, entry.key.span().start());
            }
            self.emit_expr(&entry.key);
            self.write(": ");
            self.emit_expr(&entry.value);
            self.write(",");
            let next_start = dict
                .entries
                .get(i + 1)
                .map(|e| e.key.span().start())
                .unwrap_or(span.end());
            self.emit_trailing_comment(entry.value.span().end(), next_start);
            self.newline();
        }
        self.dedent();
        self.write("}");
    }

    fn emit_list(&mut self, list: &ListExpr, _span: Span) {
        if list.items.is_empty() {
            self.write("[]");
            return;
        }
        // Check if all items are simple expressions (no comprehensions)
        let all_simple = list
            .items
            .iter()
            .all(|item| matches!(item, ListItem::Expr(_)));
        if all_simple && list.items.len() <= 4 {
            // Try inline
            self.write("[");
            for (i, item) in list.items.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                if let ListItem::Expr(expr) = item {
                    self.emit_expr(expr);
                }
            }
            self.write("]");
        } else {
            self.write("[");
            for (i, item) in list.items.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.emit_list_item(item);
            }
            self.write("]");
        }
    }

    fn emit_list_item(&mut self, item: &ListItem) {
        match item {
            ListItem::Expr(expr) => self.emit_expr(expr),
            ListItem::If(if_item) => {
                self.write("if (");
                self.emit_expr(&if_item.condition);
                self.write(") ");
                self.emit_list_item(&if_item.then_item);
            }
            ListItem::For(for_item) => {
                self.write("for (");
                self.write(&for_item.var.name);
                self.write(" in ");
                self.emit_expr(&for_item.iterable);
                self.write(") ");
                self.emit_list_item(&for_item.emit_item);
            }
        }
    }

    fn emit_extern(&mut self, ext: &ExternExpr) {
        self.write("extern \"");
        self.write(&encode_string(&ext.name));
        self.write("\": ");
        self.emit_type_expr(&ext.ty);
    }

    fn emit_exception(&mut self, exc: &ExceptionExpr) {
        self.write("exception");
        if let Some(ty) = &exc.ty {
            self.write("(");
            self.emit_type_expr(ty);
            self.write(")");
        }
    }

    fn emit_try(&mut self, try_expr: &TryExpr) {
        self.write("try ");
        self.emit_expr(&try_expr.expr);
        for catch in &try_expr.catches {
            self.newline();
            self.write("catch ");
            self.write(&catch.exception_var.name);
            if let Some(arg) = &catch.catch_arg {
                self.write("(");
                self.write(&arg.name);
                self.write(")");
            }
            self.write(": ");
            self.emit_expr(&catch.body);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{ModuleId, parse_file_mod};

    use super::Formatter;

    fn format(source: &str) -> String {
        let module_id = ModuleId::new(vec!["Test".to_owned()]);
        let diagnosed = parse_file_mod(source, &module_id);
        assert!(
            !diagnosed.diags().has_errors(),
            "parse errors: {:?}",
            diagnosed
                .diags()
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        let file_mod = diagnosed.into_inner();
        Formatter::format(source, &file_mod)
    }

    #[test]
    fn formats_let_binding() {
        let result = format("let x = 42");
        assert_eq!(result, "let x = 42\n");
    }

    #[test]
    fn formats_export_let() {
        let result = format("export let x = 42");
        assert_eq!(result, "export let x = 42\n");
    }

    #[test]
    fn formats_import() {
        let result = format("import Std/List");
        assert_eq!(result, "import Std/List\n");
    }

    #[test]
    fn formats_fn_expr() {
        let result = format("let f = fn(x: Int)\n\tx + 1");
        assert_eq!(result, "let f = fn(x: Int)\n\tx + 1\n");
    }

    #[test]
    fn formats_if_else() {
        let result = format("let x = if (a) b else c");
        assert_eq!(result, "let x = if (a)\n\tb\nelse\n\tc\n");
    }

    #[test]
    fn formats_record_shorthand() {
        let result = format("let r = { a: a, b: 1 }");
        assert_eq!(result, "let r = {\n\ta,\n\tb: 1,\n}\n");
    }

    #[test]
    fn formats_empty_record() {
        let result = format("let r = {}");
        assert_eq!(result, "let r = {}\n");
    }

    #[test]
    fn formats_type_def() {
        let result = format("type Foo { bar: Int, baz: Str }");
        assert_eq!(result, "type Foo {\n\tbar: Int,\n\tbaz: Str,\n}\n");
    }

    #[test]
    fn formats_binary_ops() {
        let result = format("let x = 1 + 2 * 3");
        assert_eq!(result, "let x = 1 + 2 * 3\n");
    }

    #[test]
    fn formats_string_interpolation() {
        let result = format(r#"let x = "hello {name}!""#);
        assert_eq!(result, "let x = \"hello {name}!\"\n");
    }

    #[test]
    fn formats_list_comprehension() {
        let result = format("let xs = [for (x in list) x]");
        assert_eq!(result, "let xs = [for (x in list) x]\n");
    }

    #[test]
    fn preserves_comments() {
        let result = format("// top comment\nlet x = 42");
        assert_eq!(result, "// top comment\nlet x = 42\n");
    }

    #[test]
    fn preserves_trailing_comments() {
        let result = format("type Foo {\n\tbar: Int, // a comment\n\tbaz: Str,\n}");
        assert_eq!(
            result,
            "type Foo {\n\tbar: Int, // a comment\n\tbaz: Str,\n}\n"
        );
    }

    #[test]
    fn formats_exception() {
        let result = format("let E = exception");
        assert_eq!(result, "let E = exception\n");
    }

    #[test]
    fn formats_exception_with_type() {
        let result = format("let E = exception(Str)");
        assert_eq!(result, "let E = exception(Str)\n");
    }

    #[test]
    fn formats_try_catch() {
        let result = format("let x = try f()\ncatch E(e): e");
        assert_eq!(result, "let x = try f()\ncatch E(e): e\n");
    }

    #[test]
    fn formats_dict() {
        let result = format(r#"let d = #{"a": 1}"#);
        assert_eq!(result, "let d = #{\n\t\"a\": 1,\n}\n");
    }

    #[test]
    fn formats_optional_type() {
        let result = format("type Foo { x: Int? }");
        assert_eq!(result, "type Foo {\n\tx: Int?,\n}\n");
    }

    #[test]
    fn formats_nil() {
        let result = format("let x = nil");
        assert_eq!(result, "let x = nil\n");
    }

    #[test]
    fn formats_property_access() {
        let result = format("let x = a.b.c");
        assert_eq!(result, "let x = a.b.c\n");
    }

    #[test]
    fn formats_generic_call() {
        let result = format("let x = f<Int>(1)");
        assert_eq!(result, "let x = f<Int>(1)\n");
    }

    #[test]
    fn formats_extern() {
        let result = format(r#"let f = extern "Foo.bar": fn(Int) Str"#);
        assert_eq!(result, "let f = extern \"Foo.bar\": fn(Int) Str\n");
    }

    #[test]
    fn formats_blank_line_between_stmts() {
        let result = format("let x = 1\nlet y = 2");
        assert_eq!(result, "let x = 1\n\nlet y = 2\n");
    }

    #[test]
    fn formats_unary_negate() {
        let result = format("let x = -1");
        assert_eq!(result, "let x = -1\n");
    }

    #[test]
    fn formats_let_in_expr() {
        let result = format("let x = let y = 1; y + 1");
        assert_eq!(result, "let x = let y = 1;\ny + 1\n");
    }

    #[test]
    fn formats_bool_literals() {
        let result = format("let x = true");
        assert_eq!(result, "let x = true\n");
    }

    #[test]
    fn formats_raise() {
        let result = format("let x = raise E");
        assert_eq!(result, "let x = raise E\n");
    }

    #[test]
    fn formats_empty_list() {
        let result = format("let x = []");
        assert_eq!(result, "let x = []\n");
    }

    #[test]
    fn formats_empty_dict() {
        let result = format("let x = #{}");
        assert_eq!(result, "let x = #{}\n");
    }

    #[test]
    fn formats_fn_type() {
        let result = format("type F fn(Int, Str) Bool");
        assert_eq!(result, "type F fn(Int, Str) Bool\n");
    }

    #[test]
    fn formats_dict_type() {
        let result = format("type D #{Str: Int}");
        assert_eq!(result, "type D #{Str: Int}\n");
    }

    #[test]
    fn formats_list_type() {
        let result = format("type L [Int]");
        assert_eq!(result, "type L [Int]\n");
    }

    #[test]
    fn formats_generic_type_def() {
        let result = format("type Pair<A, B> { first: A, second: B }");
        assert_eq!(result, "type Pair<A, B> {\n\tfirst: A,\n\tsecond: B,\n}\n");
    }

    #[test]
    fn formats_type_application() {
        let result = format("type IntList List<Int>");
        assert_eq!(result, "type IntList List<Int>\n");
    }

    #[test]
    fn formats_bounded_type_param() {
        let result = format("let f = fn<T <: { name: Str }>(x: T)\n\tx.name");
        assert_eq!(
            result,
            "let f = fn<T <: {\n\tname: Str,\n}>(x: T)\n\tx.name\n"
        );
    }

    #[test]
    fn formats_float() {
        let result = format("let x = 3.14");
        assert_eq!(result, "let x = 3.14\n");
    }
}
