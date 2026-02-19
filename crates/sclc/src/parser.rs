use std::collections::HashSet;

use peg::{Parse, ParseElem, RuleResult};
use thiserror::Error;

use crate::{
    Bool, CallExpr, Diag, DiagList, Diagnosed, Expr, FileMod, FnExpr, FnParam, ImportStmt, Int,
    InterpExpr, LetBind, LetExpr, Lexer, Loc, ModStmt, ModuleId, Position, PropertyAccessExpr,
    RecordExpr, RecordField, RecordTypeExpr, RecordTypeFieldExpr, ReplLine, Span, StrExpr, Token,
    TypeExpr, Var,
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
            .filter(|token| !matches!(token.as_ref(), Token::Whitepace(_)))
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
            = let_expr:let_expr() { let_expr }
            / fn_expr:fn_expr() { fn_expr }
            / extern_expr:extern_expr() { extern_expr }
            / property_expr()

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
            = fn_type_expr:type_expr_fn() { fn_type_expr }
            / record_type_expr:type_expr_record() { record_type_expr }
            / var:var() {
                let span = var.span();
                Loc::new(TypeExpr::Var(var), span)
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
            / int:int() {
                let span = int.span();
                Loc::new(Expr::Int(int.into_inner()), span)
            }
            / bool_lit:bool_lit() {
                let span = bool_lit.span();
                Loc::new(Expr::Bool(bool_lit.into_inner()), span)
            }
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

        rule equals() -> Span
            = [token if matches!(token.as_ref(), Token::Equals)] { token.span() }

        rule semicolon() -> Span
            = [token if matches!(token.as_ref(), Token::Semicolon)] { token.span() }

        rule slash() = [token if matches!(token.as_ref(), Token::Slash)]

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

        rule bool_lit() -> Loc<Bool>
            = [token] {? match *token.as_ref() {
                Token::TrueKeyword => Ok(Loc::new(Bool { value: true }, token.span())),
                Token::FalseKeyword => Ok(Loc::new(Bool { value: false }, token.span())),
                _ => Err("boolean"),
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
}
