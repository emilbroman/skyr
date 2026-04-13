use crate::ast::*;
use crate::{BinaryOp, Loc, Position, Span, TypeExpr};

use super::Comment;
use super::block::*;

fn encode_string(s: &str) -> String {
    crate::string_escape::encode_string(s)
}

pub struct BlockBuilder {
    comments: Vec<Comment>,
    comment_cursor: usize,
}

impl BlockBuilder {
    pub fn new(comments: Vec<Comment>) -> Self {
        BlockBuilder {
            comments,
            comment_cursor: 0,
        }
    }

    // ── Comment helpers ──────────────────────────────────────────

    fn collect_comments_before(&mut self, pos: Position) -> Vec<Block> {
        let mut blocks = vec![];
        while self.comment_cursor < self.comments.len() {
            let comment = &self.comments[self.comment_cursor];
            if comment.span.start() < pos {
                blocks.push(Block::Literal(comment.text.clone()));
                blocks.push(Block::Newline);
                self.comment_cursor += 1;
            } else {
                break;
            }
        }
        blocks
    }

    fn collect_comments_between(&mut self, after: Position, before: Position) -> Vec<Block> {
        let mut blocks = vec![];
        while self.comment_cursor < self.comments.len() {
            let comment = &self.comments[self.comment_cursor];
            if comment.span.start() >= after && comment.span.start() < before {
                blocks.push(Block::Literal(comment.text.clone()));
                blocks.push(Block::Newline);
                self.comment_cursor += 1;
            } else {
                break;
            }
        }
        blocks
    }

    fn take_trailing_comment(&mut self, after: Position, before: Position) -> Option<String> {
        if self.comment_cursor < self.comments.len() {
            let comment = &self.comments[self.comment_cursor];
            if comment.span.start().line() == after.line()
                && comment.span.start() >= after
                && comment.span.start() < before
            {
                let text = comment.text.clone();
                self.comment_cursor += 1;
                return Some(text);
            }
        }
        None
    }

    fn collect_remaining_comments(&mut self) -> Vec<Block> {
        let mut blocks = vec![];
        while self.comment_cursor < self.comments.len() {
            blocks.push(Block::Literal(
                self.comments[self.comment_cursor].text.clone(),
            ));
            blocks.push(Block::Newline);
            self.comment_cursor += 1;
        }
        blocks
    }

    // ── Doc comments ─────────────────────────────────────────────

    fn build_doc_comment(doc: &Option<String>) -> Vec<Block> {
        let mut blocks = vec![];
        if let Some(doc) = doc {
            for line in doc.lines() {
                if line.is_empty() {
                    blocks.push(Block::Literal("///".into()));
                } else {
                    blocks.push(Block::Literal(format!("/// {line}")));
                }
                blocks.push(Block::Newline);
            }
        }
        blocks
    }

    // ── SCLE Module ──────────────────────────────────────────────

    pub fn build_scle_mod(&mut self, scle: &ScleMod) -> Block {
        let mut blocks = vec![];

        for (i, import) in scle.imports.iter().enumerate() {
            if i == 0 {
                blocks.extend(self.collect_comments_before(import.span().start()));
            } else {
                let prev_end = scle.imports[i - 1].span().end();
                blocks.extend(self.collect_comments_between(prev_end, import.span().start()));
            }
            blocks.push(self.build_import(import));
            blocks.push(Block::Newline);
        }

        // Determine the anchor position for comment collection before the
        // type expression (if present) or body (if no type expression).
        let next_start = scle
            .type_expr
            .as_ref()
            .map(|t| t.span().start())
            .or_else(|| scle.body.as_ref().map(|b| b.span().start()));

        if !scle.imports.is_empty() {
            blocks.push(Block::Newline);
            if let Some(start) = next_start {
                let prev_end = scle.imports.last().unwrap().span().end();
                blocks.extend(self.collect_comments_between(prev_end, start));
            }
        } else if let Some(start) = next_start {
            blocks.extend(self.collect_comments_before(start));
        }

        if let Some(type_expr) = &scle.type_expr {
            blocks.push(self.build_type_expr(type_expr));
            blocks.push(Block::Newline);

            // Blank line between type expression and body
            if let Some(body) = &scle.body {
                blocks.push(Block::Newline);
                blocks.extend(
                    self.collect_comments_between(type_expr.span().end(), body.span().start()),
                );
            }
        }

        if let Some(body) = &scle.body {
            blocks.push(self.build_expr(body));
            blocks.push(Block::Newline);
        }

        blocks.extend(self.collect_remaining_comments());

        Block::Seq(blocks)
    }

    // ── File / Module ────────────────────────────────────────────

    pub fn build_file_mod(&mut self, file_mod: &FileMod) -> Block {
        let stmts = &file_mod.statements;
        let mut blocks = vec![];

        for (i, stmt) in stmts.iter().enumerate() {
            let stmt_start = Self::mod_stmt_start(stmt);

            if i == 0 {
                blocks.extend(self.collect_comments_before(stmt_start));
            } else {
                let prev_end = Self::mod_stmt_end(&stmts[i - 1]);
                let both_imports = matches!(&stmts[i - 1], ModStmt::Import(_))
                    && matches!(stmt, ModStmt::Import(_));
                if !both_imports {
                    blocks.push(Block::Newline);
                }
                blocks.extend(self.collect_comments_between(prev_end, stmt_start));
            }

            blocks.push(self.build_mod_stmt(stmt));
            blocks.push(Block::Newline);
        }

        blocks.extend(self.collect_remaining_comments());

        Block::Seq(blocks)
    }

    fn mod_stmt_start(stmt: &ModStmt) -> Position {
        match stmt {
            ModStmt::Import(import) => import.span().start(),
            ModStmt::Let(bind) | ModStmt::Export(bind) => bind.var.span().start(),
            ModStmt::TypeDef(td) | ModStmt::ExportTypeDef(td) => td.var.span().start(),
            ModStmt::Expr(expr) => expr.span().start(),
        }
    }

    fn mod_stmt_end(stmt: &ModStmt) -> Position {
        match stmt {
            ModStmt::Import(import) => import.span().end(),
            ModStmt::Let(bind) | ModStmt::Export(bind) => bind.expr.span().end(),
            ModStmt::TypeDef(td) | ModStmt::ExportTypeDef(td) => td.ty.span().end(),
            ModStmt::Expr(expr) => expr.span().end(),
        }
    }

    fn build_mod_stmt(&mut self, stmt: &ModStmt) -> Block {
        match stmt {
            ModStmt::Import(import) => self.build_import(import),
            ModStmt::Let(bind) => {
                let mut parts = Self::build_doc_comment(&bind.doc_comment);
                parts.push(Block::Literal("let ".into()));
                parts.push(self.build_let_bind(bind));
                Block::Seq(parts)
            }
            ModStmt::Export(bind) => {
                let mut parts = Self::build_doc_comment(&bind.doc_comment);
                parts.push(Block::Literal("export let ".into()));
                parts.push(self.build_let_bind(bind));
                Block::Seq(parts)
            }
            ModStmt::TypeDef(td) => {
                let mut parts = Self::build_doc_comment(&td.doc_comment);
                parts.push(Block::Literal("type ".into()));
                parts.push(self.build_type_def(td));
                Block::Seq(parts)
            }
            ModStmt::ExportTypeDef(td) => {
                let mut parts = Self::build_doc_comment(&td.doc_comment);
                parts.push(Block::Literal("export type ".into()));
                parts.push(self.build_type_def(td));
                Block::Seq(parts)
            }
            ModStmt::Expr(expr) => self.build_expr(expr),
        }
    }

    fn build_import(&self, import: &Loc<ImportStmt>) -> Block {
        let mut s = "import ".to_string();
        for (i, var) in import.vars.iter().enumerate() {
            if i > 0 {
                s.push('/');
            }
            s.push_str(&var.name);
        }
        Block::Literal(s)
    }

    fn build_let_bind(&mut self, bind: &LetBind) -> Block {
        let mut parts = vec![Block::Literal(bind.var.name.clone())];
        if let Some(ty) = &bind.ty {
            parts.push(Block::Literal(": ".into()));
            parts.push(self.build_type_expr(ty));
        }
        parts.push(Block::Literal(" = ".into()));
        parts.push(self.build_expr(&bind.expr));
        Block::Seq(parts)
    }

    fn build_type_def(&mut self, td: &TypeDef) -> Block {
        let mut parts = vec![Block::Literal(td.var.name.clone())];
        parts.push(self.build_type_params(&td.type_params));
        parts.push(Block::Literal(" ".into()));
        parts.push(self.build_type_expr(&td.ty));
        Block::Seq(parts)
    }

    // ── Type expressions ─────────────────────────────────────────

    fn build_type_params(&mut self, params: &[TypeParam]) -> Block {
        if params.is_empty() {
            return Block::Seq(vec![]);
        }
        let mut items = Vec::new();
        for param in params {
            let mut parts = vec![Block::Literal(param.var.name.clone())];
            if let Some(bound) = &param.bound {
                parts.push(Block::Literal(" <: ".into()));
                parts.push(self.build_type_expr(bound));
            }
            items.push(CommaSepItem {
                leading_comments: vec![],
                doc_comment: None,
                content: Block::Seq(parts),
                trailing_comment: None,
            });
        }
        Block::CommaSep(CommaSepBlock {
            open: "<",
            close: ">",
            items,
            space_around: false,
            force_unfolded: false,
        })
    }

    fn build_type_expr(&mut self, ty: &Loc<TypeExpr>) -> Block {
        match ty.as_ref() {
            TypeExpr::Var(var) => Block::Literal(var.name.clone()),
            TypeExpr::Optional(inner) => Block::Seq(vec![
                self.build_type_expr(inner),
                Block::Literal("?".into()),
            ]),
            TypeExpr::List(inner) => Block::Seq(vec![
                Block::Literal("[".into()),
                self.build_type_expr(inner),
                Block::Literal("]".into()),
            ]),
            TypeExpr::Fn(fn_ty) => {
                let mut parts = vec![Block::Literal("fn".into())];
                parts.push(self.build_type_params(&fn_ty.type_params));
                let mut param_items = Vec::new();
                for param in fn_ty.params.iter() {
                    param_items.push(CommaSepItem {
                        leading_comments: vec![],
                        doc_comment: None,
                        content: self.build_type_expr(param),
                        trailing_comment: None,
                    });
                }
                parts.push(Block::CommaSep(CommaSepBlock {
                    open: "(",
                    close: ")",
                    items: param_items,
                    space_around: false,
                    force_unfolded: false,
                }));
                parts.push(Block::Literal(" ".into()));
                parts.push(self.build_type_expr(&fn_ty.ret));
                Block::Seq(parts)
            }
            TypeExpr::Record(record) => self.build_record_type(record, ty.span()),
            TypeExpr::Dict(dict) => Block::Seq(vec![
                Block::Literal("#{".into()),
                self.build_type_expr(&dict.key),
                Block::Literal(": ".into()),
                self.build_type_expr(&dict.value),
                Block::Literal("}".into()),
            ]),
            TypeExpr::PropertyAccess(access) => Block::Seq(vec![
                self.build_type_expr(&access.expr),
                Block::Literal(".".into()),
                Block::Literal(access.property.name.clone()),
            ]),
            TypeExpr::Application(app) => {
                let mut parts = vec![self.build_type_expr(&app.base)];
                let mut items = Vec::new();
                for arg in app.args.iter() {
                    items.push(CommaSepItem {
                        leading_comments: vec![],
                        doc_comment: None,
                        content: self.build_type_expr(arg),
                        trailing_comment: None,
                    });
                }
                parts.push(Block::CommaSep(CommaSepBlock {
                    open: "<",
                    close: ">",
                    items,
                    space_around: false,
                    force_unfolded: false,
                }));
                Block::Seq(parts)
            }
        }
    }

    fn build_record_type(&mut self, record: &RecordTypeExpr, span: Span) -> Block {
        if record.fields.is_empty() {
            return Block::Literal("{}".into());
        }

        let mut items = Vec::new();
        for (i, field) in record.fields.iter().enumerate() {
            let leading = if i > 0 {
                let prev_end = record.fields[i - 1].ty.span().end();
                self.collect_comments_between(prev_end, field.var.span().start())
            } else {
                self.collect_comments_before(field.var.span().start())
            };

            let content = Block::Seq(vec![
                Block::Literal(format!("{}: ", field.var.name)),
                self.build_type_expr(&field.ty),
            ]);

            let next_start = record
                .fields
                .get(i + 1)
                .map(|f| f.var.span().start())
                .unwrap_or(span.end());
            let trailing = self.take_trailing_comment(field.ty.span().end(), next_start);

            items.push(CommaSepItem {
                leading_comments: leading,
                doc_comment: field.doc_comment.clone(),
                content,
                trailing_comment: trailing,
            });
        }

        let force_unfolded = span.start().line() < record.fields[0].var.span().start().line();

        Block::CommaSep(CommaSepBlock {
            open: "{",
            close: "}",
            items,
            space_around: true,
            force_unfolded,
        })
    }

    // ── Expressions ──────────────────────────────────────────────

    pub fn build_expr(&mut self, expr: &Loc<Expr>) -> Block {
        match expr.as_ref() {
            Expr::Int(int) => Block::Literal(int.value.to_string()),
            Expr::Float(float) => Block::Literal(float.value.to_string()),
            Expr::Bool(b) => Block::Literal(if b.value { "true" } else { "false" }.into()),
            Expr::Nil => Block::Literal("nil".into()),
            Expr::Str(s) => Block::Literal(format!("\"{}\"", encode_string(&s.value))),
            Expr::Path(p) => Self::build_path(p),
            Expr::Interp(interp) => self.build_interp(interp),
            Expr::Var(var) => Block::Literal(var.name.clone()),
            Expr::Unary(unary) => Block::Seq(vec![
                Block::Literal(unary.op.to_string()),
                self.build_expr(&unary.expr),
            ]),
            Expr::Binary(binary) => self.build_binary(binary, false),
            Expr::If(if_expr) => self.build_if(if_expr, expr.span()),
            Expr::Let(let_expr) => self.build_let_expr(let_expr),
            Expr::Fn(fn_expr) => self.build_fn(fn_expr, expr.span()),
            Expr::Call(call) => self.build_call(call),
            Expr::Record(record) => self.build_record(record, expr.span()),
            Expr::Dict(dict) => self.build_dict(dict, expr.span()),
            Expr::List(list) => self.build_list(list),
            Expr::PropertyAccess(access) => {
                let dot = if access.optional { "?." } else { "." };
                Block::Seq(vec![
                    self.build_expr(&access.expr),
                    Block::Literal(dot.into()),
                    Block::Literal(access.property.name.clone()),
                ])
            }
            Expr::TypeCast(cast) => Block::Seq(vec![
                self.build_expr(&cast.expr),
                Block::Literal(" as ".into()),
                self.build_type_expr(&cast.ty),
            ]),
            Expr::Extern(ext) => self.build_extern(ext),
            Expr::Exception(exc) => self.build_exception(exc),
            Expr::Raise(raise) => Block::Seq(vec![
                Block::Literal("raise ".into()),
                self.build_expr(&raise.expr),
            ]),
            Expr::Try(try_expr) => self.build_try(try_expr),
            Expr::IndexedAccess(ia) => Block::Seq(vec![
                self.build_expr(&ia.expr),
                Block::Literal("[".into()),
                self.build_expr(&ia.index),
                Block::Literal("]".into()),
            ]),
        }
    }

    fn build_expr_inner(&mut self, expr: &Loc<Expr>, in_parens: bool) -> Block {
        match expr.as_ref() {
            Expr::Binary(binary) => self.build_binary(binary, in_parens),
            _ => self.build_expr(expr),
        }
    }

    fn build_path(p: &PathExpr) -> Block {
        if p.is_root() {
            return Block::Literal("/".into());
        }
        let mut s = String::new();
        let mut first = true;
        for segment in p.values() {
            if !first || !segment.starts_with('.') {
                s.push('/');
            }
            first = false;
            let needs_quoting = segment != "."
                && segment != ".."
                && !segment
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-');
            if needs_quoting {
                s.push('"');
                s.push_str(&encode_string(segment));
                s.push('"');
            } else {
                s.push_str(segment);
            }
        }
        Block::Literal(s)
    }

    fn build_interp(&mut self, interp: &InterpExpr) -> Block {
        let mut parts = vec![Block::Literal("\"".into())];
        for part in interp.parts.iter() {
            match part.as_ref() {
                Expr::Str(s) => {
                    parts.push(Block::Literal(encode_string(&s.value)));
                }
                _ => {
                    parts.push(Block::Literal("{".into()));
                    parts.push(self.build_expr(part));
                    parts.push(Block::Literal("}".into()));
                }
            }
        }
        parts.push(Block::Literal("\"".into()));
        Block::Seq(parts)
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

    fn build_binary(&mut self, binary: &BinaryExpr, in_parens: bool) -> Block {
        let prec = Self::binary_precedence(&binary.op);

        let lhs_needs_parens = match &**binary.lhs.as_ref() {
            Expr::Binary(lhs_bin) => Self::binary_precedence(&lhs_bin.op) < prec,
            _ => false,
        };

        let rhs_needs_parens = match &**binary.rhs.as_ref() {
            Expr::Binary(rhs_bin) => Self::binary_precedence(&rhs_bin.op) <= prec,
            _ => false,
        };

        let lhs = if lhs_needs_parens {
            Block::Seq(vec![
                Block::Literal("(".into()),
                self.build_expr_inner(&binary.lhs, true),
                Block::Literal(")".into()),
            ])
        } else {
            self.build_expr_inner(&binary.lhs, in_parens)
        };

        let rhs = if rhs_needs_parens {
            Block::Seq(vec![
                Block::Literal("(".into()),
                self.build_expr_inner(&binary.rhs, true),
                Block::Literal(")".into()),
            ])
        } else {
            self.build_expr_inner(&binary.rhs, in_parens)
        };

        Block::Seq(vec![lhs, Block::Literal(format!(" {} ", binary.op)), rhs])
    }

    fn build_if(&mut self, if_expr: &IfExpr, span: Span) -> Block {
        // If the then clause starts on a different line than the if keyword,
        // always unfold branches (respect user's formatting choice).
        let force_unfolded = span.start().line() < if_expr.then_expr.span().start().line();

        if force_unfolded {
            return self.build_if_unfolded(if_expr);
        }

        let condition = self.build_expr(&if_expr.condition);
        let then_block = self.build_expr(&if_expr.then_expr);
        let mut items = vec![
            GroupItem::Block(Block::Literal("if (".into())),
            GroupItem::PotentialUnfold {
                tag: 1,
                space_when_folded: false,
                indent_children: true,
                children: vec![condition],
            },
            GroupItem::Block(Block::Literal(")".into())),
            GroupItem::PotentialUnfold {
                tag: 2,
                space_when_folded: true,
                indent_children: true,
                children: vec![then_block],
            },
        ];

        if let Some(else_expr) = &if_expr.else_expr {
            if let Expr::If(else_if) = &**else_expr.as_ref() {
                items.push(GroupItem::PotentialUnfold {
                    tag: 3,
                    space_when_folded: true,
                    indent_children: false,
                    children: vec![
                        Block::Literal("else ".into()),
                        self.build_if(else_if, else_expr.span()),
                    ],
                });
            } else {
                items.push(GroupItem::PotentialUnfold {
                    tag: 3,
                    space_when_folded: true,
                    indent_children: false,
                    children: vec![Block::Literal("else".into())],
                });
                items.push(GroupItem::PotentialUnfold {
                    tag: 2,
                    space_when_folded: true,
                    indent_children: true,
                    children: vec![self.build_expr(else_expr)],
                });
            }
        }

        Block::Group(items)
    }

    /// Build an if expression with branches always on separate lines.
    /// The condition still uses a Group for width-aware wrapping.
    fn build_if_unfolded(&mut self, if_expr: &IfExpr) -> Block {
        let condition = self.build_expr(&if_expr.condition);

        let cond_group = Block::Group(vec![
            GroupItem::Block(Block::Literal("if (".into())),
            GroupItem::PotentialUnfold {
                tag: 1,
                space_when_folded: false,
                indent_children: true,
                children: vec![condition],
            },
            GroupItem::Block(Block::Literal(")".into())),
        ]);

        let mut parts = vec![
            cond_group,
            Block::Newline,
            Block::Indent(Box::new(self.build_expr(&if_expr.then_expr))),
        ];

        if let Some(else_expr) = &if_expr.else_expr {
            if let Expr::If(else_if) = &**else_expr.as_ref() {
                parts.push(Block::Newline);
                parts.push(Block::Literal("else ".into()));
                parts.push(self.build_if(else_if, else_expr.span()));
            } else {
                parts.push(Block::Newline);
                parts.push(Block::Literal("else".into()));
                parts.push(Block::Newline);
                parts.push(Block::Indent(Box::new(self.build_expr(else_expr))));
            }
        }

        Block::Seq(parts)
    }

    fn build_let_expr(&mut self, let_expr: &LetExpr) -> Block {
        let mut parts = vec![
            Block::Literal("let ".into()),
            self.build_let_bind(&let_expr.bind),
            Block::Literal(";".into()),
        ];
        if let Some(body) = &let_expr.expr {
            parts.push(Block::Newline);
            parts.push(self.build_expr(body));
        }
        Block::Seq(parts)
    }

    fn build_fn(&mut self, fn_expr: &FnExpr, span: Span) -> Block {
        let mut header = vec![Block::Literal("fn".into())];
        header.push(self.build_type_params(&fn_expr.type_params));

        let mut param_items = Vec::new();
        for param in fn_expr.params.iter() {
            let mut parts = vec![Block::Literal(param.var.name.clone())];
            if let Some(ty) = &param.ty {
                parts.push(Block::Literal(": ".into()));
                parts.push(self.build_type_expr(ty));
            }
            param_items.push(CommaSepItem {
                leading_comments: vec![],
                doc_comment: None,
                content: Block::Seq(parts),
                trailing_comment: None,
            });
        }

        header.push(Block::CommaSep(CommaSepBlock {
            open: "(",
            close: ")",
            items: param_items,
            space_around: false,
            force_unfolded: false,
        }));

        let body = fn_expr.body.as_ref().map(|b| self.build_expr(b));

        let Some(body) = body else {
            return Block::Seq(header);
        };

        // If the body starts on a different line than the fn keyword,
        // always unfold (respect user's formatting choice).
        let force_unfolded = fn_expr
            .body
            .as_ref()
            .is_some_and(|b| span.start().line() < b.span().start().line());

        if force_unfolded {
            Block::Seq(vec![
                Block::Seq(header),
                Block::Newline,
                Block::Indent(Box::new(body)),
            ])
        } else {
            Block::Group(vec![
                GroupItem::Block(Block::Seq(header)),
                GroupItem::PotentialUnfold {
                    tag: 1,
                    space_when_folded: true,
                    indent_children: true,
                    children: vec![body],
                },
            ])
        }
    }

    fn build_call(&mut self, call: &CallExpr) -> Block {
        let mut parts = vec![self.build_expr(&call.callee)];

        if !call.type_args.is_empty() {
            let mut type_arg_items = Vec::new();
            for arg in call.type_args.iter() {
                type_arg_items.push(CommaSepItem {
                    leading_comments: vec![],
                    doc_comment: None,
                    content: self.build_type_expr(arg),
                    trailing_comment: None,
                });
            }
            parts.push(Block::CommaSep(CommaSepBlock {
                open: "<",
                close: ">",
                items: type_arg_items,
                space_around: false,
                force_unfolded: false,
            }));
        }

        let mut arg_items = Vec::new();
        for arg in call.args.iter() {
            arg_items.push(CommaSepItem {
                leading_comments: vec![],
                doc_comment: None,
                content: self.build_expr(arg),
                trailing_comment: None,
            });
        }
        parts.push(Block::CommaSep(CommaSepBlock {
            open: "(",
            close: ")",
            items: arg_items,
            space_around: false,
            force_unfolded: false,
        }));

        Block::Seq(parts)
    }

    fn build_record(&mut self, record: &RecordExpr, span: Span) -> Block {
        if record.fields.is_empty() {
            return Block::Literal("{}".into());
        }

        let mut items = Vec::new();
        for (i, field) in record.fields.iter().enumerate() {
            let leading = if i > 0 {
                let prev_end = record.fields[i - 1].expr.span().end();
                self.collect_comments_between(prev_end, field.var.span().start())
            } else {
                vec![]
            };

            let is_shorthand =
                matches!(field.expr.as_ref(), Expr::Var(v) if v.name == field.var.name);
            let content = if is_shorthand {
                Block::Literal(field.var.name.clone())
            } else {
                Block::Seq(vec![
                    Block::Literal(format!("{}: ", field.var.name)),
                    self.build_expr(&field.expr),
                ])
            };

            let next_start = record
                .fields
                .get(i + 1)
                .map(|f| f.var.span().start())
                .unwrap_or(span.end());
            let trailing = self.take_trailing_comment(field.expr.span().end(), next_start);

            items.push(CommaSepItem {
                leading_comments: leading,
                doc_comment: field.doc_comment.clone(),
                content,
                trailing_comment: trailing,
            });
        }

        let force_unfolded = span.start().line() < record.fields[0].var.span().start().line();

        Block::CommaSep(CommaSepBlock {
            open: "{",
            close: "}",
            items,
            space_around: true,
            force_unfolded,
        })
    }

    fn build_dict(&mut self, dict: &DictExpr, span: Span) -> Block {
        if dict.entries.is_empty() {
            return Block::Literal("#{}".into());
        }

        let mut items = Vec::new();
        for (i, entry) in dict.entries.iter().enumerate() {
            let leading = if i > 0 {
                let prev_end = dict.entries[i - 1].value.span().end();
                self.collect_comments_between(prev_end, entry.key.span().start())
            } else {
                vec![]
            };

            let content = Block::Seq(vec![
                self.build_expr(&entry.key),
                Block::Literal(": ".into()),
                self.build_expr(&entry.value),
            ]);

            let next_start = dict
                .entries
                .get(i + 1)
                .map(|e| e.key.span().start())
                .unwrap_or(span.end());
            let trailing = self.take_trailing_comment(entry.value.span().end(), next_start);

            items.push(CommaSepItem {
                leading_comments: leading,
                doc_comment: None,
                content,
                trailing_comment: trailing,
            });
        }

        let force_unfolded = span.start().line() < dict.entries[0].key.span().start().line();

        Block::CommaSep(CommaSepBlock {
            open: "#{",
            close: "}",
            items,
            space_around: true,
            force_unfolded,
        })
    }

    fn build_list(&mut self, list: &ListExpr) -> Block {
        if list.items.is_empty() {
            return Block::Literal("[]".into());
        }

        let mut items = Vec::new();
        for item in list.items.iter() {
            items.push(CommaSepItem {
                leading_comments: vec![],
                doc_comment: None,
                content: self.build_list_item(item),
                trailing_comment: None,
            });
        }

        Block::CommaSep(CommaSepBlock {
            open: "[",
            close: "]",
            items,
            space_around: false,
            force_unfolded: false,
        })
    }

    fn list_item_start_line(item: &ListItem) -> u32 {
        match item {
            ListItem::Expr(expr) => expr.span().start().line(),
            ListItem::If(if_item) => if_item.condition.span().start().line(),
            ListItem::For(for_item) => for_item.var.span().start().line(),
        }
    }

    fn build_list_item(&mut self, item: &ListItem) -> Block {
        match item {
            ListItem::Expr(expr) => self.build_expr(expr),
            ListItem::If(if_item) => {
                // If the then clause starts on a different line than the condition,
                // always unfold (respect user's formatting choice).
                let force_unfolded = if_item.condition.span().start().line()
                    < Self::list_item_start_line(&if_item.then_item);

                if force_unfolded {
                    Block::Seq(vec![
                        Block::Seq(vec![
                            Block::Literal("if (".into()),
                            self.build_expr(&if_item.condition),
                            Block::Literal(")".into()),
                        ]),
                        Block::Newline,
                        Block::Indent(Box::new(self.build_list_item(&if_item.then_item))),
                    ])
                } else {
                    Block::Group(vec![
                        GroupItem::Block(Block::Seq(vec![
                            Block::Literal("if (".into()),
                            self.build_expr(&if_item.condition),
                            Block::Literal(")".into()),
                        ])),
                        GroupItem::PotentialUnfold {
                            tag: 1,
                            space_when_folded: true,
                            indent_children: true,
                            children: vec![self.build_list_item(&if_item.then_item)],
                        },
                    ])
                }
            }
            ListItem::For(for_item) => Block::Group(vec![
                GroupItem::Block(Block::Seq(vec![
                    Block::Literal("for (".into()),
                    Block::Literal(for_item.var.name.clone()),
                    Block::Literal(" in ".into()),
                    self.build_expr(&for_item.iterable),
                    Block::Literal(")".into()),
                ])),
                GroupItem::PotentialUnfold {
                    tag: 1,
                    space_when_folded: true,
                    indent_children: true,
                    children: vec![self.build_list_item(&for_item.emit_item)],
                },
            ]),
        }
    }

    fn build_extern(&mut self, ext: &ExternExpr) -> Block {
        Block::Seq(vec![
            Block::Literal(format!("extern \"{}\": ", encode_string(&ext.name))),
            self.build_type_expr(&ext.ty),
        ])
    }

    fn build_exception(&mut self, exc: &ExceptionExpr) -> Block {
        if let Some(ty) = &exc.ty {
            Block::Seq(vec![
                Block::Literal("exception(".into()),
                self.build_type_expr(ty),
                Block::Literal(")".into()),
            ])
        } else {
            Block::Literal("exception".into())
        }
    }

    fn build_try(&mut self, try_expr: &TryExpr) -> Block {
        let mut parts = vec![
            Block::Literal("try ".into()),
            self.build_expr(&try_expr.expr),
        ];
        for catch in &try_expr.catches {
            parts.push(Block::Newline);
            let mut catch_header = format!("catch {}", catch.exception_var.name);
            if let Some(arg) = &catch.catch_arg {
                catch_header.push('(');
                catch_header.push_str(&arg.name);
                catch_header.push(')');
            }
            catch_header.push_str(": ");
            parts.push(Block::Literal(catch_header));
            parts.push(self.build_expr(&catch.body));
        }
        Block::Seq(parts)
    }
}
