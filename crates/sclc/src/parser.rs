use std::collections::HashSet;

use peg::{Parse, ParseElem, RuleResult};
use thiserror::Error;

use crate::{
    BinaryExpr, BinaryOp, Bool, CallExpr, Diag, DiagList, Diagnosed, Expr, FileMod, Float, FnExpr,
    FnParam, IfExpr, ImportStmt, Int, InterpExpr, LetBind, LetExpr, Lexer, ListExpr, ListForItem,
    ListIfItem, ListItem, Loc, ModStmt, ModuleId, Position, PropertyAccessExpr, RecordExpr,
    RecordField, RecordTypeExpr, RecordTypeFieldExpr, ReplLine, Span, StrExpr, Token, TypeExpr,
    UnaryExpr, UnaryOp, Var,
};

#[derive(Error, Debug)]
#[error("duplicate record field: {name}")]
pub struct DuplicateRecordField {
    pub module_id: ModuleId,
    pub name: String,
    pub span: Span,
}

impl Diag for DuplicateRecordField {
    fn locate(&self) -> (ModuleId, Span) {
        (self.module_id.clone(), self.span)
    }
}

enum Postfix {
    Property(Loc<Var>),
    Call(Vec<Loc<Expr>>, Span),
}

fn decode_string(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();

    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }

        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('{') => out.push('{'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
}

pub struct TokenStream<'a> {
    tokens: Vec<crate::Loc<Token<'a>>>,
    eof_position: Position,
}

impl<'a> TokenStream<'a> {
    fn new(source: &'a str) -> Self {
        let tokens = Lexer::new(source)
            .filter(|token| !matches!(token.as_ref(), Token::Whitepace(_) | Token::Comment(_)))
            .collect::<Vec<_>>();
        let eof_position = tokens
            .last()
            .map(|token| token.span().end())
            .unwrap_or_default();
        Self {
            tokens,
            eof_position,
        }
    }
}

impl<'a> Parse for TokenStream<'a> {
    type PositionRepr = Position;

    fn start<'input>(&'input self) -> usize {
        0
    }

    fn is_eof<'input>(&'input self, p: usize) -> bool {
        p >= self.tokens.len()
    }

    fn position_repr<'input>(&'input self, p: usize) -> Self::PositionRepr {
        self.tokens
            .get(p)
            .map(|token| token.span().start())
            .unwrap_or(self.eof_position)
    }
}

impl<'input: 'a, 'a> ParseElem<'input> for TokenStream<'a> {
    type Element = Loc<Token<'a>>;

    fn parse_elem(&'input self, pos: usize) -> RuleResult<Self::Element> {
        match self.tokens.get(pos) {
            Some(token) => RuleResult::Matched(pos + 1, *token),
            None => RuleResult::Failed,
        }
    }
}

peg::parser! {
    grammar grammar<'tok>(diags: &mut DiagList, module_id: &ModuleId) for TokenStream<'tok> {
        pub rule file_mod() -> FileMod
            = statements:mod_stmt()* eof() { FileMod { statements } }

        pub rule repl_line() -> ReplLine
            = statement:mod_stmt() eof() { ReplLine { statement } }

        rule eof() = ![_]

        rule mod_stmt() -> ModStmt
            = import_stmt:import_stmt() { ModStmt::Import(import_stmt) }
            / export_let_bind:export_let_bind() { ModStmt::Export(export_let_bind) }
            / expr:expr() { ModStmt::Expr(expr) }
            / let_bind:let_bind() { ModStmt::Let(let_bind) }

        rule expr() -> Loc<Expr>
            = if_expr:if_expr() { if_expr }
            / let_expr:let_expr() { let_expr }
            / fn_expr:fn_expr() { fn_expr }
            / extern_expr:extern_expr() { extern_expr }
            / logical_or_expr()

        rule logical_or_expr() -> Loc<Expr>
            = head:logical_and_expr() tail:(
                or_or() rhs:logical_and_expr() { (BinaryOp::Or, rhs) }
            )* {
                let mut expr = head;
                for (op, rhs) in tail {
                    let span = Span::new(expr.span().start(), rhs.span().end());
                    expr = Loc::new(
                        Expr::Binary(BinaryExpr {
                            op,
                            lhs: Box::new(expr),
                            rhs: Box::new(rhs),
                        }),
                        span,
                    );
                }
                expr
            }

        rule logical_and_expr() -> Loc<Expr>
            = head:equality_expr() tail:(
                and_and() rhs:equality_expr() { (BinaryOp::And, rhs) }
            )* {
                let mut expr = head;
                for (op, rhs) in tail {
                    let span = Span::new(expr.span().start(), rhs.span().end());
                    expr = Loc::new(
                        Expr::Binary(BinaryExpr {
                            op,
                            lhs: Box::new(expr),
                            rhs: Box::new(rhs),
                        }),
                        span,
                    );
                }
                expr
            }

        rule equality_expr() -> Loc<Expr>
            = head:comparison_expr() tail:(
                eq_eq() rhs:comparison_expr() { (BinaryOp::Eq, rhs) }
                / bang_eq() rhs:comparison_expr() { (BinaryOp::Neq, rhs) }
            )* {
                let mut expr = head;
                for (op, rhs) in tail {
                    let span = Span::new(expr.span().start(), rhs.span().end());
                    expr = Loc::new(
                        Expr::Binary(BinaryExpr {
                            op,
                            lhs: Box::new(expr),
                            rhs: Box::new(rhs),
                        }),
                        span,
                    );
                }
                expr
            }

        rule comparison_expr() -> Loc<Expr>
            = head:add_expr() tail:(
                less() rhs:add_expr() { (BinaryOp::Lt, rhs) }
                / less_eq() rhs:add_expr() { (BinaryOp::Lte, rhs) }
                / greater() rhs:add_expr() { (BinaryOp::Gt, rhs) }
                / greater_eq() rhs:add_expr() { (BinaryOp::Gte, rhs) }
            )* {
                let mut expr = head;
                for (op, rhs) in tail {
                    let span = Span::new(expr.span().start(), rhs.span().end());
                    expr = Loc::new(
                        Expr::Binary(BinaryExpr {
                            op,
                            lhs: Box::new(expr),
                            rhs: Box::new(rhs),
                        }),
                        span,
                    );
                }
                expr
            }

        rule add_expr() -> Loc<Expr>
            = head:mul_expr() tail:(
                plus() rhs:mul_expr() { (BinaryOp::Add, rhs) }
                / minus() rhs:mul_expr() { (BinaryOp::Sub, rhs) }
            )* {
                let mut expr = head;
                for (op, rhs) in tail {
                    let span = Span::new(expr.span().start(), rhs.span().end());
                    expr = Loc::new(
                        Expr::Binary(BinaryExpr {
                            op,
                            lhs: Box::new(expr),
                            rhs: Box::new(rhs),
                        }),
                        span,
                    );
                }
                expr
            }

        rule mul_expr() -> Loc<Expr>
            = head:unary_expr() tail:(
                star() rhs:unary_expr() { (BinaryOp::Mul, rhs) }
                / slash() rhs:unary_expr() { (BinaryOp::Div, rhs) }
            )* {
                let mut expr = head;
                for (op, rhs) in tail {
                    let span = Span::new(expr.span().start(), rhs.span().end());
                    expr = Loc::new(
                        Expr::Binary(BinaryExpr {
                            op,
                            lhs: Box::new(expr),
                            rhs: Box::new(rhs),
                        }),
                        span,
                    );
                }
                expr
            }

        rule unary_expr() -> Loc<Expr>
            = minus_span:minus() expr:unary_expr() {
                let span = Span::new(minus_span.start(), expr.span().end());
                Loc::new(
                    Expr::Unary(UnaryExpr {
                        op: UnaryOp::Negate,
                        expr: Box::new(expr),
                    }),
                    span,
                )
            }
            / property_expr()

        rule if_expr() -> Loc<Expr>
            = if_kw_span:if_keyword() open_paren() condition:expr() close_paren() then_expr:expr() else_expr:(else_keyword() else_expr:expr() { else_expr })? {
                let end = else_expr
                    .as_ref()
                    .map(|expr| expr.span().end())
                    .unwrap_or_else(|| then_expr.span().end());
                Loc::new(
                    Expr::If(IfExpr {
                        condition: Box::new(condition),
                        then_expr: Box::new(then_expr),
                        else_expr: else_expr.map(Box::new),
                    }),
                    Span::new(if_kw_span.start(), end),
                )
            }

        // fn expressions are right-associative because the body is parsed as a full Expr.
        rule fn_expr() -> Loc<Expr>
            = fn_kw_span:fn_keyword() open_paren() params:fn_params() close_paren() body:expr() {
                let end = body.span().end();
                Loc::new(Expr::Fn(FnExpr {
                    params,
                    body: Box::new(body),
                }), Span::new(fn_kw_span.start(), end))
            }

        rule fn_params() -> Vec<FnParam>
            = params:(fn_param() ++ comma()) comma()? { params }
            / { vec![] }

        rule fn_param() -> FnParam
            = var:var() colon() ty:type_expr() { FnParam { var, ty } }

        rule type_expr() -> Loc<TypeExpr>
            = base:type_expr_base() optional:question_mark()? {
                if let Some(optional_span) = optional {
                    let span = Span::new(base.span().start(), optional_span.end());
                    Loc::new(TypeExpr::Optional(Box::new(base)), span)
                } else {
                    base
                }
            }

        rule type_expr_base() -> Loc<TypeExpr>
            = fn_type_expr:type_expr_fn() { fn_type_expr }
            / record_type_expr:type_expr_record() { record_type_expr }
            / list_type_expr:type_expr_list() { list_type_expr }
            / var:var() {
                let span = var.span();
                Loc::new(TypeExpr::Var(var), span)
            }

        rule type_expr_list() -> Loc<TypeExpr>
            = open_square_span:open_square() item:type_expr() close_square_span:close_square() {
                Loc::new(
                    TypeExpr::List(Box::new(item)),
                    Span::new(open_square_span.start(), close_square_span.end()),
                )
            }

        rule type_expr_fn() -> Loc<TypeExpr>
            = fn_kw_span:fn_keyword() open_paren() params:type_expr_params() close_paren() ret:type_expr() {
                let end = ret.span().end();
                Loc::new(TypeExpr::Fn(crate::FnTypeExpr {
                    params,
                    ret: Box::new(ret),
                }), Span::new(fn_kw_span.start(), end))
            }

        rule type_expr_params() -> Vec<Loc<TypeExpr>>
            = params:(type_expr() ++ comma()) comma()? { params }
            / { vec![] }

        rule type_expr_record() -> Loc<TypeExpr>
            = open_curly_span:open_curly() close_curly_span:close_curly() {
                Loc::new(
                    TypeExpr::Record(RecordTypeExpr { fields: vec![] }),
                    Span::new(open_curly_span.start(), close_curly_span.end()),
                )
            }
            / open_curly_span:open_curly() fields:(type_expr_record_field() ++ comma()) comma()? close_curly_span:close_curly() {
                Loc::new(
                    TypeExpr::Record(RecordTypeExpr { fields }),
                    Span::new(open_curly_span.start(), close_curly_span.end()),
                )
            }

        rule type_expr_record_field() -> RecordTypeFieldExpr
            = var:var() colon() ty:type_expr() { RecordTypeFieldExpr { var, ty } }

        rule extern_expr() -> Loc<Expr>
            = extern_kw_span:extern_keyword() name:str_simple() colon() ty:type_expr() {
                let end = ty.span().end();
                Loc::new(
                    Expr::Extern(crate::ExternExpr {
                        name: name.0,
                        ty,
                    }),
                    Span::new(extern_kw_span.start(), end),
                )
            }

        rule property_expr() -> Loc<Expr>
            = head:atom_expr() suffixes:postfix_suffix()* {
                let mut expr = head;
                for suffix in suffixes {
                    expr = match suffix {
                        Postfix::Property(property) => {
                            let start = expr.span().start();
                            let end = property.span().end();
                            Loc::new(Expr::PropertyAccess(PropertyAccessExpr {
                                expr: Box::new(expr),
                                property,
                            }), Span::new(start, end))
                        }
                        Postfix::Call(args, close_paren_span) => {
                            let start = expr.span().start();
                            let end = close_paren_span.end();
                            Loc::new(Expr::Call(CallExpr {
                                callee: Box::new(expr),
                                args,
                            }), Span::new(start, end))
                        }
                    };
                }
                expr
            }

        rule postfix_suffix() -> Postfix
            = dot() property:var() { Postfix::Property(property) }
            / open_paren() args:call_args() close_paren_span:close_paren() { Postfix::Call(args, close_paren_span) }

        rule call_args() -> Vec<Loc<Expr>>
            = args:(expr() ++ comma()) comma()? { args }
            / { vec![] }

        rule atom_expr() -> Loc<Expr>
            = open_paren_span:open_paren() expr:expr() close_paren_span:close_paren() {
                Loc::new(expr.into_inner(), Span::new(open_paren_span.start(), close_paren_span.end()))
            }
            / string_expr:string_expr() { string_expr }
            / record_expr:record_expr() { record_expr }
            / list_expr:list_expr() { list_expr }
            / float:float() {
                let span = float.span();
                Loc::new(Expr::Float(float.into_inner()), span)
            }
            / int:int() {
                let span = int.span();
                Loc::new(Expr::Int(int.into_inner()), span)
            }
            / bool_lit:bool_lit() {
                let span = bool_lit.span();
                Loc::new(Expr::Bool(bool_lit.into_inner()), span)
            }
            / nil_lit:nil_lit() { nil_lit }
            / var:var() {
                let span = var.span();
                Loc::new(Expr::Var(var), span)
            }

        rule string_expr() -> Loc<Expr>
            = simple:str_simple() {
                Loc::new(Expr::Str(StrExpr { value: simple.0 }), simple.1)
            }
            / begin:str_begin() first:expr() rest:(cont:str_cont() expr:expr() { (cont, expr) })* end:str_end() {
                let mut parts = Vec::new();
                parts.push(Loc::new(Expr::Str(StrExpr { value: begin.0 }), begin.1));
                parts.push(first);
                for (cont, expr) in rest {
                    parts.push(Loc::new(Expr::Str(StrExpr { value: cont.0 }), cont.1));
                    parts.push(expr);
                }
                parts.push(Loc::new(Expr::Str(StrExpr { value: end.0 }), end.1));
                let span = Span::new(begin.1.start(), end.1.end());
                Loc::new(Expr::Interp(InterpExpr { parts }), span)
            }

        rule record_expr() -> Loc<Expr>
            = open_curly_span:open_curly() close_curly_span:close_curly() {
                Loc::new(Expr::Record(RecordExpr { fields: vec![] }), Span::new(open_curly_span.start(), close_curly_span.end()))
            }
            / open_curly_span:open_curly() fields:(record_field() ++ comma()) comma()? close_curly_span:close_curly() {
                let mut seen_fields = HashSet::new();
                for field in &fields {
                    if !seen_fields.insert(field.var.name.clone()) {
                        diags.push(DuplicateRecordField {
                            module_id: module_id.clone(),
                            name: field.var.name.clone(),
                            span: field.var.span(),
                        });
                    }
                }
                Loc::new(Expr::Record(RecordExpr { fields }), Span::new(open_curly_span.start(), close_curly_span.end()))
            }

        rule record_field() -> RecordField
            = var:var() colon() expr:expr() { RecordField { var, expr } }

        rule list_expr() -> Loc<Expr>
            = open_square_span:open_square() close_square_span:close_square() {
                Loc::new(
                    Expr::List(ListExpr { items: vec![] }),
                    Span::new(open_square_span.start(), close_square_span.end()),
                )
            }
            / open_square_span:open_square() items:(list_item() ++ comma()) comma()? close_square_span:close_square() {
                Loc::new(
                    Expr::List(ListExpr { items }),
                    Span::new(open_square_span.start(), close_square_span.end()),
                )
            }

        rule list_item() -> ListItem
            = list_for_item:list_for_item() { list_for_item }
            / list_if_item:list_if_item() { list_if_item }
            / expr:expr() { ListItem::Expr(expr) }

        rule list_for_item() -> ListItem
            = for_keyword() open_paren() var:var() in_keyword() iterable:expr() close_paren() emit_item:list_item() {
                ListItem::For(ListForItem {
                    var,
                    iterable: Box::new(iterable),
                    emit_item: Box::new(emit_item),
                })
            }

        rule list_if_item() -> ListItem
            = if_keyword() open_paren() condition:expr() close_paren() then_item:list_item() !else_keyword() {
                ListItem::If(ListIfItem {
                    condition: Box::new(condition),
                    then_item: Box::new(then_item),
                })
            }

        rule let_expr() -> Loc<Expr>
            = bind:let_bind() semicolon() expr:expr() {
                let span = Span::new(bind.var.span().start(), expr.span().end());
                Loc::new(Expr::Let(LetExpr { bind, expr: Box::new(expr) }), span)
            }

        rule let_bind() -> LetBind
            = let_keyword() var:var() equals() expr:expr() {
                LetBind { var, expr: Box::new(expr) }
            }

        rule export_let_bind() -> LetBind
            = export_keyword() let_bind:let_bind() { let_bind }

        rule import_stmt() -> Loc<ImportStmt>
            = keyword_span:import_keyword_span() vars:import_path() {
                let end = vars
                    .last()
                    .map(|var| var.span().end())
                    .unwrap_or_else(|| keyword_span.end());
                let span = Span::new(keyword_span.start(), end);
                Loc::new(ImportStmt { vars }, span)
            }

        rule import_path() -> Vec<Loc<Var>>
            = first:var() rest:(slash() var:var() { var })* {
                let mut vars = vec![first];
                vars.extend(rest);
                vars
            }

        rule import_keyword_span() -> Span
            = [token if matches!(token.as_ref(), Token::ImportKeyword)] { token.span() }

        rule let_keyword() -> Span
            = [token if matches!(token.as_ref(), Token::LetKeyword)] { token.span() }

        rule export_keyword() -> Span
            = [token if matches!(token.as_ref(), Token::ExportKeyword)] { token.span() }

        rule fn_keyword() -> Span
            = [token if matches!(token.as_ref(), Token::FnKeyword)] { token.span() }

        rule extern_keyword() -> Span
            = [token if matches!(token.as_ref(), Token::ExternKeyword)] { token.span() }

        rule if_keyword() -> Span
            = [token if matches!(token.as_ref(), Token::IfKeyword)] { token.span() }

        rule else_keyword() -> Span
            = [token if matches!(token.as_ref(), Token::ElseKeyword)] { token.span() }

        rule for_keyword() -> Span
            = [token if matches!(token.as_ref(), Token::ForKeyword)] { token.span() }

        rule in_keyword() -> Span
            = [token if matches!(token.as_ref(), Token::InKeyword)] { token.span() }

        rule equals() -> Span
            = [token if matches!(token.as_ref(), Token::Equals)] { token.span() }
        rule eq_eq() -> Span
            = [token if matches!(token.as_ref(), Token::EqEq)] { token.span() }
        rule bang_eq() -> Span
            = [token if matches!(token.as_ref(), Token::BangEq)] { token.span() }
        rule less() -> Span
            = [token if matches!(token.as_ref(), Token::Less)] { token.span() }
        rule less_eq() -> Span
            = [token if matches!(token.as_ref(), Token::LessEq)] { token.span() }
        rule greater() -> Span
            = [token if matches!(token.as_ref(), Token::Greater)] { token.span() }
        rule greater_eq() -> Span
            = [token if matches!(token.as_ref(), Token::GreaterEq)] { token.span() }
        rule and_and() -> Span
            = [token if matches!(token.as_ref(), Token::AndAnd)] { token.span() }
        rule or_or() -> Span
            = [token if matches!(token.as_ref(), Token::OrOr)] { token.span() }

        rule semicolon() -> Span
            = [token if matches!(token.as_ref(), Token::Semicolon)] { token.span() }

        rule question_mark() -> Span
            = [token if matches!(token.as_ref(), Token::QuestionMark)] { token.span() }

        rule slash() = [token if matches!(token.as_ref(), Token::Slash)]
        rule plus() -> Span
            = [token if matches!(token.as_ref(), Token::Plus)] { token.span() }
        rule minus() -> Span
            = [token if matches!(token.as_ref(), Token::Minus)] { token.span() }
        rule star() -> Span
            = [token if matches!(token.as_ref(), Token::Star)] { token.span() }

        rule open_curly() -> Span
            = [token if matches!(token.as_ref(), Token::OpenCurly)] { token.span() }

        rule close_curly() -> Span
            = [token if matches!(token.as_ref(), Token::CloseCurly)] { token.span() }

        rule colon() -> Span
            = [token if matches!(token.as_ref(), Token::Colon)] { token.span() }

        rule comma() -> Span
            = [token if matches!(token.as_ref(), Token::Comma)] { token.span() }

        rule dot() -> Span
            = [token if matches!(token.as_ref(), Token::Dot)] { token.span() }

        rule open_paren() -> Span
            = [token if matches!(token.as_ref(), Token::OpenParen)] { token.span() }

        rule close_paren() -> Span
            = [token if matches!(token.as_ref(), Token::CloseParen)] { token.span() }

        rule open_square() -> Span
            = [token if matches!(token.as_ref(), Token::OpenSquare)] { token.span() }

        rule close_square() -> Span
            = [token if matches!(token.as_ref(), Token::CloseSquare)] { token.span() }

        rule var() -> Loc<Var>
            = [token] {? match *token.as_ref() {
                Token::Symbol(name) => Ok(Loc::new(Var { name: name.to_owned() }, token.span())),
                _ => Err("symbol"),
            } }

        rule int() -> Loc<Int>
            = [token] {? match *token.as_ref() {
                Token::Int(value) => match value.parse::<i64>() {
                    Ok(parsed) => Ok(Loc::new(Int { value: parsed }, token.span())),
                    Err(_) => Err("integer"),
                },
                _ => Err("integer"),
            } }

        rule float() -> Loc<Float>
            = [token] {? match *token.as_ref() {
                Token::Float(value) => match value.parse::<f64>() {
                    Ok(parsed) if parsed.is_finite() => match ordered_float::NotNan::new(parsed) {
                        Ok(parsed) => Ok(Loc::new(Float { value: parsed }, token.span())),
                        Err(_) => Err("float"),
                    },
                    Ok(_) | Err(_) => Err("float"),
                },
                _ => Err("float"),
            } }

        rule bool_lit() -> Loc<Bool>
            = [token] {? match *token.as_ref() {
                Token::TrueKeyword => Ok(Loc::new(Bool { value: true }, token.span())),
                Token::FalseKeyword => Ok(Loc::new(Bool { value: false }, token.span())),
                _ => Err("boolean"),
            } }

        rule nil_lit() -> Loc<Expr>
            = [token] {? match *token.as_ref() {
                Token::NilKeyword => Ok(Loc::new(Expr::Nil, token.span())),
                _ => Err("nil"),
            } }

        rule str_simple() -> (String, Span)
            = [token] {? match *token.as_ref() {
                Token::StrSimple(raw) => Ok((decode_string(raw), token.span())),
                _ => Err("string"),
            } }

        rule str_begin() -> (String, Span)
            = [token] {? match *token.as_ref() {
                Token::StrBegin(raw) => Ok((decode_string(raw), token.span())),
                _ => Err("string interpolation begin"),
            } }

        rule str_cont() -> (String, Span)
            = [token] {? match *token.as_ref() {
                Token::StrCont(raw) => Ok((decode_string(raw), token.span())),
                _ => Err("string interpolation continue"),
            } }

        rule str_end() -> (String, Span)
            = [token] {? match *token.as_ref() {
                Token::StrEnd(raw) => Ok((decode_string(raw), token.span())),
                _ => Err("string interpolation end"),
            } }
    }
}

pub fn parse_file_mod(
    source: &str,
    module_id: &ModuleId,
) -> Result<Diagnosed<FileMod>, peg::error::ParseError<Position>> {
    let mut diags = DiagList::new();
    let file_mod = grammar::file_mod(&TokenStream::new(source), &mut diags, module_id)?;
    Ok(Diagnosed::new(file_mod, diags))
}

pub fn parse_repl_line(
    source: &str,
    module_id: &ModuleId,
) -> Result<Diagnosed<ReplLine>, peg::error::ParseError<Position>> {
    let mut diags = DiagList::new();
    let repl_line = grammar::repl_line(&TokenStream::new(source), &mut diags, module_id)?;
    Ok(Diagnosed::new(repl_line, diags))
}

#[cfg(test)]
mod tests {
    use crate::ModuleId;

    use super::{parse_file_mod, parse_repl_line};

    #[test]
    fn parse_error_uses_position_repr() {
        let err = parse_file_mod("{x", &ModuleId::default()).expect_err("expected parse failure");
        eprintln!("{err}");
        assert_eq!(err.location.line(), 1);
        assert_eq!(err.location.character(), 3);
    }

    #[test]
    fn parses_record_with_trailing_comma() {
        let line = parse_repl_line("{ a: 1, b: 2, }", &ModuleId::default())
            .expect("record should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Record(record) = expr.into_inner() else {
            panic!("expected record expression");
        };
        assert_eq!(record.fields.len(), 2);
        assert_eq!(record.fields[0].var.name, "a");
        assert_eq!(record.fields[1].var.name, "b");
    }

    #[test]
    fn duplicate_record_fields_emit_diagnostic() {
        let module_id = ["Org".to_owned(), "Pkg".to_owned(), "Main".to_owned()]
            .into_iter()
            .collect::<ModuleId>();
        let diagnosed = parse_repl_line("{ a: 1, a: 2 }", &module_id).expect("record should parse");
        assert!(diagnosed.diags().has_errors());
        let first_diag = diagnosed
            .diags()
            .iter()
            .next()
            .expect("expected diagnostic");
        let (diag_module_id, _) = first_diag.locate();
        assert_eq!(diag_module_id, module_id);
    }

    #[test]
    fn property_access_is_left_associative() {
        let line = parse_repl_line("a.b.c", &ModuleId::default())
            .expect("property access should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::PropertyAccess(level2) = expr.into_inner() else {
            panic!("expected property access expression");
        };
        assert_eq!(level2.property.name, "c");
        let crate::Expr::PropertyAccess(level1) = level2.expr.into_inner() else {
            panic!("expected nested property access");
        };
        assert_eq!(level1.property.name, "b");
        let crate::Expr::Var(root) = level1.expr.into_inner() else {
            panic!("expected root var");
        };
        assert_eq!(root.name, "a");
    }

    #[test]
    fn parenthesized_expr_takes_precedence_in_property_access() {
        let line = parse_repl_line("({ a: 1 }).a", &ModuleId::default())
            .expect("parenthesized property access should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::PropertyAccess(access) = expr.into_inner() else {
            panic!("expected property access expression");
        };
        assert_eq!(access.property.name, "a");
        let crate::Expr::Record(_) = access.expr.into_inner() else {
            panic!("expected parenthesized inner record expression");
        };
    }

    #[test]
    fn parses_fn_with_optional_trailing_param_comma() {
        let line = parse_repl_line("fn(a: A, b: B,) a", &ModuleId::default())
            .expect("fn expression should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Fn(fn_expr) = expr.into_inner() else {
            panic!("expected fn expression");
        };
        assert_eq!(fn_expr.params.len(), 2);
        assert_eq!(fn_expr.params[0].var.name, "a");
        assert_eq!(fn_expr.params[1].var.name, "b");
    }

    #[test]
    fn parses_addition_left_associative() {
        let line = parse_repl_line("1 + 2 + 3", &ModuleId::default())
            .expect("addition should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(outer) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(outer.op, crate::BinaryOp::Add));
        let crate::Expr::Binary(inner) = outer.lhs.into_inner() else {
            panic!("expected nested binary expression");
        };
        assert!(matches!(inner.op, crate::BinaryOp::Add));
        assert!(matches!(inner.lhs.as_ref().as_ref(), crate::Expr::Int(_)));
        assert!(matches!(inner.rhs.as_ref().as_ref(), crate::Expr::Int(_)));
        assert!(matches!(outer.rhs.as_ref().as_ref(), crate::Expr::Int(_)));
    }

    #[test]
    fn parses_subtraction_left_associative() {
        let line = parse_repl_line("5 - 2 - 1", &ModuleId::default())
            .expect("subtraction should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(outer) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(outer.op, crate::BinaryOp::Sub));
        let crate::Expr::Binary(inner) = outer.lhs.into_inner() else {
            panic!("expected nested binary expression");
        };
        assert!(matches!(inner.op, crate::BinaryOp::Sub));
        assert!(matches!(inner.lhs.as_ref().as_ref(), crate::Expr::Int(_)));
        assert!(matches!(inner.rhs.as_ref().as_ref(), crate::Expr::Int(_)));
        assert!(matches!(outer.rhs.as_ref().as_ref(), crate::Expr::Int(_)));
    }

    #[test]
    fn parses_unary_minus_with_addition() {
        let line = parse_repl_line("-1 + 2", &ModuleId::default())
            .expect("unary minus should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(add) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(add.op, crate::BinaryOp::Add));
        let crate::Expr::Unary(unary) = add.lhs.into_inner() else {
            panic!("expected unary expression");
        };
        assert!(matches!(unary.op, crate::UnaryOp::Negate));
        assert!(matches!(unary.expr.as_ref().as_ref(), crate::Expr::Int(_)));
        assert!(matches!(add.rhs.as_ref().as_ref(), crate::Expr::Int(_)));
    }

    #[test]
    fn parses_multiplication_precedence() {
        let line = parse_repl_line("1 + 2 * 3", &ModuleId::default())
            .expect("multiplication should bind tighter")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(add) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(add.op, crate::BinaryOp::Add));
        let crate::Expr::Binary(mul) = add.rhs.into_inner() else {
            panic!("expected multiplication on rhs");
        };
        assert!(matches!(mul.op, crate::BinaryOp::Mul));
    }

    #[test]
    fn parses_multiplication_left_associative() {
        let line = parse_repl_line("6 * 2 * 3", &ModuleId::default())
            .expect("multiplication should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(outer) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(outer.op, crate::BinaryOp::Mul));
        let crate::Expr::Binary(inner) = outer.lhs.into_inner() else {
            panic!("expected nested binary expression");
        };
        assert!(matches!(inner.op, crate::BinaryOp::Mul));
        assert!(matches!(inner.lhs.as_ref().as_ref(), crate::Expr::Int(_)));
        assert!(matches!(inner.rhs.as_ref().as_ref(), crate::Expr::Int(_)));
        assert!(matches!(outer.rhs.as_ref().as_ref(), crate::Expr::Int(_)));
    }

    #[test]
    fn parses_division_left_associative() {
        let line = parse_repl_line("8 / 2 / 2", &ModuleId::default())
            .expect("division should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(outer) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(outer.op, crate::BinaryOp::Div));
        let crate::Expr::Binary(inner) = outer.lhs.into_inner() else {
            panic!("expected nested binary expression");
        };
        assert!(matches!(inner.op, crate::BinaryOp::Div));
    }

    #[test]
    fn parses_division_precedence() {
        let line = parse_repl_line("1 + 6 / 2", &ModuleId::default())
            .expect("division should bind tighter")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(add) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(add.op, crate::BinaryOp::Add));
        let crate::Expr::Binary(div) = add.rhs.into_inner() else {
            panic!("expected division on rhs");
        };
        assert!(matches!(div.op, crate::BinaryOp::Div));
    }

    #[test]
    fn parses_comparison_precedence_over_equality() {
        let line = parse_repl_line("1 == 2 < 3", &ModuleId::default())
            .expect("comparison should bind tighter")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(eq) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(eq.op, crate::BinaryOp::Eq));
        let crate::Expr::Binary(cmp) = eq.rhs.into_inner() else {
            panic!("expected comparison on rhs");
        };
        assert!(matches!(cmp.op, crate::BinaryOp::Lt));
    }

    #[test]
    fn parses_equality_left_associative() {
        let line = parse_repl_line("a == b != c", &ModuleId::default())
            .expect("equality should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(outer) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(outer.op, crate::BinaryOp::Neq));
        let crate::Expr::Binary(inner) = outer.lhs.into_inner() else {
            panic!("expected nested binary expression");
        };
        assert!(matches!(inner.op, crate::BinaryOp::Eq));
    }

    #[test]
    fn parses_logical_precedence_over_equality() {
        let line = parse_repl_line("a || b == c", &ModuleId::default())
            .expect("equality should bind tighter than or")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(or_expr) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(or_expr.op, crate::BinaryOp::Or));
        let crate::Expr::Binary(eq_expr) = or_expr.rhs.into_inner() else {
            panic!("expected equality on rhs");
        };
        assert!(matches!(eq_expr.op, crate::BinaryOp::Eq));
    }

    #[test]
    fn parses_and_before_or() {
        let line = parse_repl_line("a && b || c", &ModuleId::default())
            .expect("and should bind tighter than or")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Binary(or_expr) = expr.into_inner() else {
            panic!("expected binary expression");
        };
        assert!(matches!(or_expr.op, crate::BinaryOp::Or));
        let crate::Expr::Binary(and_expr) = or_expr.lhs.into_inner() else {
            panic!("expected and on lhs");
        };
        assert!(matches!(and_expr.op, crate::BinaryOp::And));
    }

    #[test]
    fn parses_if_expression_with_else() {
        let line = parse_repl_line("if (true) 1 else 2", &ModuleId::default())
            .expect("if expression should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::If(if_expr) = expr.into_inner() else {
            panic!("expected if expression");
        };
        let crate::Expr::Bool(condition) = if_expr.condition.into_inner() else {
            panic!("expected bool condition");
        };
        assert!(condition.value);
    }

    #[test]
    fn parses_list_literal_with_optional_trailing_comma() {
        let line = parse_repl_line("[1, 2,]", &ModuleId::default())
            .expect("list literal should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::List(list) = expr.into_inner() else {
            panic!("expected list expression");
        };
        assert_eq!(list.items.len(), 2);
    }

    #[test]
    fn parses_list_for_comprehension_item() {
        let line = parse_repl_line("[for (x in y) x]", &ModuleId::default())
            .expect("list comprehension should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::List(list) = expr.into_inner() else {
            panic!("expected list expression");
        };
        assert_eq!(list.items.len(), 1);
        let crate::ListItem::For(for_item) = &list.items[0] else {
            panic!("expected for item");
        };
        assert_eq!(for_item.var.name, "x");
        let crate::Expr::Var(iterable) = for_item.iterable.as_ref().as_ref() else {
            panic!("expected iterable var");
        };
        assert_eq!(iterable.name, "y");
    }

    #[test]
    fn parses_mixed_list_comprehension_items() {
        let line = parse_repl_line("[1, for (x in [2, 3]) x, 4, 5]", &ModuleId::default())
            .expect("mixed list literal should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::List(list) = expr.into_inner() else {
            panic!("expected list expression");
        };
        assert_eq!(list.items.len(), 4);
        assert!(matches!(list.items[1], crate::ListItem::For(_)));
    }

    #[test]
    fn parses_list_if_comprehension_item() {
        let line = parse_repl_line("[1, if (false) 2, 3]", &ModuleId::default())
            .expect("list-if comprehension should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::List(list) = expr.into_inner() else {
            panic!("expected list expression");
        };
        assert_eq!(list.items.len(), 3);
        assert!(matches!(list.items[1], crate::ListItem::If(_)));
    }

    #[test]
    fn parses_if_expression_inside_list_item() {
        let line = parse_repl_line("[if (true) 1 else 2]", &ModuleId::default())
            .expect("if expression list item should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::List(list) = expr.into_inner() else {
            panic!("expected list expression");
        };
        assert_eq!(list.items.len(), 1);
        let crate::ListItem::Expr(expr) = &list.items[0] else {
            panic!("expected expression list item");
        };
        assert!(matches!(expr.as_ref(), crate::Expr::If(_)));
    }

    #[test]
    fn parses_nested_list_comprehension_items() {
        let line = parse_repl_line(
            "[1, 2, for (x in [3, 4]) if (true) for (y in [x, x]) y]",
            &ModuleId::default(),
        )
        .expect("nested comprehension should parse")
        .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::List(list) = expr.into_inner() else {
            panic!("expected list expression");
        };
        assert_eq!(list.items.len(), 3);
        let crate::ListItem::For(outer_for) = &list.items[2] else {
            panic!("expected outer for item");
        };
        let crate::ListItem::If(inner_if) = outer_for.emit_item.as_ref() else {
            panic!("expected nested if item");
        };
        let crate::ListItem::For(_) = inner_if.then_item.as_ref() else {
            panic!("expected nested for item");
        };
    }

    #[test]
    fn fn_body_is_right_associative() {
        let line = parse_repl_line("fn(a: A) fn(b: B) b", &ModuleId::default())
            .expect("nested fn expression should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Fn(outer) = expr.into_inner() else {
            panic!("expected outer fn expression");
        };
        let crate::Expr::Fn(inner) = outer.body.into_inner() else {
            panic!("expected inner fn expression as body");
        };
        assert_eq!(outer.params.len(), 1);
        assert_eq!(inner.params.len(), 1);
        assert_eq!(inner.params[0].var.name, "b");
    }

    #[test]
    fn parses_call_with_optional_trailing_comma() {
        let line = parse_repl_line("f(a, b,)", &ModuleId::default())
            .expect("call expression should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Call(call) = expr.into_inner() else {
            panic!("expected call expression");
        };
        let crate::Expr::Var(callee) = &**call.callee else {
            panic!("expected var callee");
        };
        assert_eq!(callee.name, "f");
        assert_eq!(call.args.len(), 2);
    }

    #[test]
    fn parses_simple_string_expr() {
        let line = parse_repl_line("\"hello\\nworld\"", &ModuleId::default())
            .expect("string should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Str(string) = expr.into_inner() else {
            panic!("expected string expression");
        };
        assert_eq!(string.value, "hello\nworld");
    }

    #[test]
    fn parses_interpolated_string_expr() {
        let line = parse_repl_line("\"value: {x.y}\"", &ModuleId::default())
            .expect("interpolated string should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Interp(interp) = expr.into_inner() else {
            panic!("expected interpolation expression");
        };
        assert_eq!(interp.parts.len(), 3);
        assert!(matches!(interp.parts[0].as_ref(), crate::Expr::Str(_)));
        assert!(matches!(
            interp.parts[1].as_ref(),
            crate::Expr::PropertyAccess(_)
        ));
        assert!(matches!(interp.parts[2].as_ref(), crate::Expr::Str(_)));
    }

    #[test]
    fn parses_extern_expr() {
        let line = parse_repl_line("extern \"clock\": Int", &ModuleId::default())
            .expect("extern expression should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Extern(extern_expr) = expr.into_inner() else {
            panic!("expected extern expression");
        };
        assert_eq!(extern_expr.name, "clock");
    }

    #[test]
    fn parses_function_type_expr() {
        let line = parse_repl_line("extern \"f\": fn(Int, Str) Int", &ModuleId::default())
            .expect("extern function type should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Extern(extern_expr) = expr.into_inner() else {
            panic!("expected extern expression");
        };
        let crate::TypeExpr::Fn(fn_ty) = extern_expr.ty.into_inner() else {
            panic!("expected fn type expression");
        };
        assert_eq!(fn_ty.params.len(), 2);
    }

    #[test]
    fn parses_list_type_expr() {
        let line = parse_repl_line("extern \"xs\": [Int]", &ModuleId::default())
            .expect("list type should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Extern(extern_expr) = expr.into_inner() else {
            panic!("expected extern expression");
        };
        let crate::TypeExpr::List(inner) = extern_expr.ty.into_inner() else {
            panic!("expected list type expression");
        };
        let crate::TypeExpr::Var(var) = inner.into_inner() else {
            panic!("expected var inner type");
        };
        assert_eq!(var.name, "Int");
    }

    #[test]
    fn parses_float_literal() {
        let line = parse_repl_line("3.14", &ModuleId::default())
            .expect("float should parse")
            .into_inner();
        let crate::ModStmt::Expr(expr) = line.statement else {
            panic!("expected expression statement");
        };
        let crate::Expr::Float(float) = expr.into_inner() else {
            panic!("expected float expression");
        };
        assert_eq!(float.value.to_string(), "3.14");
    }
}
