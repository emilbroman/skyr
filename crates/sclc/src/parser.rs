use peg::{Parse, ParseElem, RuleResult};

use crate::{
    Expr, FileMod, ImportStmt, Int, LetBind, LetExpr, Lexer, Loc, ModStmt, Position, PrintStmt,
    Span, Token, Var,
};

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
    grammar grammar<'tok>() for TokenStream<'tok> {
        pub rule file_mod() -> FileMod
            = statements:mod_stmt()* eof() { FileMod { statements } }

        rule eof() = ![_]

        rule mod_stmt() -> ModStmt
            = import_stmt:import_stmt() { ModStmt::Import(import_stmt) }
            / print_stmt:print_stmt() { ModStmt::Print(print_stmt) }
            / expr:expr() { ModStmt::Expr(expr) }
            / let_bind:let_bind() { ModStmt::Let(let_bind) }

        rule expr() -> Expr
            = let_expr:let_expr() { Expr::Let(let_expr) }
            / int:int() { Expr::Int(int.into_inner()) }
            / var:var() { Expr::Var(var.into_inner()) }

        rule let_expr() -> LetExpr
            = bind:let_bind() semicolon() expr:expr() { LetExpr { bind, expr: Box::new(expr) } }

        rule let_bind() -> LetBind
            = let_keyword() var:var() equals() expr:expr() { LetBind { var: var.into_inner(), expr: Box::new(expr) } }

        rule import_stmt() -> Loc<ImportStmt>
            = keyword_span:import_keyword_span() vars:import_path() {
                let end = vars
                    .last()
                    .map(|var| var.span().end())
                    .unwrap_or_else(|| keyword_span.end());
                let span = Span::new(keyword_span.start(), end);
                let vars = vars.into_iter().map(Loc::into_inner).collect();
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

pub fn parse_file_mod(source: &str) -> Result<FileMod, peg::error::ParseError<Position>> {
    grammar::file_mod(&TokenStream::new(source))
}

#[cfg(test)]
mod tests {
    use super::parse_file_mod;

    #[test]
    fn parse_error_uses_position_repr() {
        let err = parse_file_mod("{x").expect_err("expected parse failure");
        eprintln!("{err}");
        assert_eq!(err.location.line(), 1);
        assert_eq!(err.location.character(), 1);
    }
}
