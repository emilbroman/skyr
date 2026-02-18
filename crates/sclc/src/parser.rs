use std::collections::HashSet;

use peg::{Parse, ParseElem, RuleResult};
use thiserror::Error;

use crate::{
    Diag, DiagList, Diagnosed, Expr, FileMod, ImportStmt, Int, LetBind, LetExpr, Lexer, Loc,
    ModStmt, ModuleId, Position, PrintStmt, PropertyAccessExpr, RecordExpr, RecordField, ReplLine,
    Span, Token, Var,
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
            / print_stmt:print_stmt() { ModStmt::Print(print_stmt) }
            / expr:expr() { ModStmt::Expr(expr) }
            / let_bind:let_bind() { ModStmt::Let(let_bind) }

        rule expr() -> Expr
            = let_expr:let_expr() { Expr::Let(let_expr) }
            / property_expr()

        rule property_expr() -> Expr
            = head:atom_expr() accessors:(dot() property:var() { property })* {
                let mut expr = head;
                for property in accessors {
                    expr = Expr::PropertyAccess(PropertyAccessExpr {
                        expr: Box::new(expr),
                        property,
                    });
                }
                expr
            }

        rule atom_expr() -> Expr
            = open_paren() expr:expr() close_paren() { expr }
            / record_expr:record_expr() { Expr::Record(record_expr) }
            / int:int() { Expr::Int(int.into_inner()) }
            / var:var() { Expr::Var(var) }

        rule record_expr() -> RecordExpr
            = open_curly() close_curly() { RecordExpr { fields: vec![] } }
            / open_curly() fields:(record_field() ++ comma()) comma()? close_curly() {
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
                RecordExpr { fields }
            }

        rule record_field() -> RecordField
            = var:var() colon() expr:expr() { RecordField { var, expr } }

        rule let_expr() -> LetExpr
            = bind:let_bind() semicolon() expr:expr() { LetExpr { bind, expr: Box::new(expr) } }

        rule let_bind() -> LetBind
            = let_keyword() var:var() equals() expr:expr() { LetBind { var, expr: Box::new(expr) } }

        rule import_stmt() -> Loc<ImportStmt>
            = keyword_span:import_keyword_span() vars:import_path() {
                let end = vars
                    .last()
                    .map(|var| var.span().end())
                    .unwrap_or_else(|| keyword_span.end());
                let span = Span::new(keyword_span.start(), end);
                Loc::new(ImportStmt { vars }, span)
            }

        rule print_stmt() -> PrintStmt
            = print_keyword() expr:expr() { PrintStmt { expr } }

        rule import_path() -> Vec<Loc<Var>>
            = first:var() rest:(slash() var:var() { var })* {
                let mut vars = vec![first];
                vars.extend(rest);
                vars
            }

        rule import_keyword_span() -> Span
            = [token if matches!(token.as_ref(), Token::ImportKeyword)] { token.span() }

        rule let_keyword() = [token if matches!(token.as_ref(), Token::LetKeyword)]

        rule print_keyword() = [token if matches!(token.as_ref(), Token::PrintKeyword)]

        rule equals() = [token if matches!(token.as_ref(), Token::Equals)]

        rule semicolon() = [token if matches!(token.as_ref(), Token::Semicolon)]

        rule slash() = [token if matches!(token.as_ref(), Token::Slash)]

        rule open_curly() = [token if matches!(token.as_ref(), Token::OpenCurly)]

        rule close_curly() = [token if matches!(token.as_ref(), Token::CloseCurly)]

        rule colon() = [token if matches!(token.as_ref(), Token::Colon)]

        rule comma() = [token if matches!(token.as_ref(), Token::Comma)]

        rule dot() = [token if matches!(token.as_ref(), Token::Dot)]

        rule open_paren() = [token if matches!(token.as_ref(), Token::OpenParen)]

        rule close_paren() = [token if matches!(token.as_ref(), Token::CloseParen)]

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
        let crate::ModStmt::Expr(crate::Expr::Record(record)) = line.statement else {
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
        let crate::ModStmt::Expr(crate::Expr::PropertyAccess(level2)) = line.statement else {
            panic!("expected property access expression");
        };
        assert_eq!(level2.property.name, "c");
        let crate::Expr::PropertyAccess(level1) = *level2.expr else {
            panic!("expected nested property access");
        };
        assert_eq!(level1.property.name, "b");
        let crate::Expr::Var(root) = *level1.expr else {
            panic!("expected root var");
        };
        assert_eq!(root.name, "a");
    }

    #[test]
    fn parenthesized_expr_takes_precedence_in_property_access() {
        let line = parse_repl_line("({ a: 1 }).a", &ModuleId::default())
            .expect("parenthesized property access should parse")
            .into_inner();
        let crate::ModStmt::Expr(crate::Expr::PropertyAccess(access)) = line.statement else {
            panic!("expected property access expression");
        };
        assert_eq!(access.property.name, "a");
        let crate::Expr::Record(_) = *access.expr else {
            panic!("expected parenthesized inner record expression");
        };
    }
}
