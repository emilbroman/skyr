use peg::{Parse, ParseElem, RuleResult};

use crate::{Expr, FileMod, ImportStmt, Lexer, ModStmt, Position, Token, Var};

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
    type Element = Token<'a>;

    fn parse_elem(&'input self, pos: usize) -> RuleResult<Self::Element> {
        match self.tokens.get(pos) {
            Some(token) => RuleResult::Matched(pos + 1, *token.as_ref()),
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
            / expr:expr() { ModStmt::Expr(expr) }

        rule expr() -> Expr
            = var:var() { Expr::Var(var) }

        rule import_stmt() -> ImportStmt
            = import_keyword() vars:import_path() { ImportStmt { vars } }

        rule import_path() -> Vec<Var>
            = first:var() rest:(slash() var:var() { var })* {
                let mut vars = vec![first];
                vars.extend(rest);
                vars
            }

        rule import_keyword() = [Token::ImportKeyword]

        rule slash() = [Token::Slash]

        rule var() -> Var
            = [Token::Symbol(name)] { Var { name: name.to_owned() } }
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
