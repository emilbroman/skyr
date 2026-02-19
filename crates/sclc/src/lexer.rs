use std::iter::Peekable;
use unicode_segmentation::{GraphemeIndices, UnicodeSegmentation};

use crate::{Loc, Position, Span};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Token<'a> {
    OpenCurly,
    CloseCurly,
    Colon,
    Comma,
    Dot,
    OpenParen,
    CloseParen,
    Equals,
    Semicolon,
    Slash,
    ImportKeyword,
    LetKeyword,
    FnKeyword,
    ExportKeyword,
    ExternKeyword,
    Int(&'a str),
    StrBegin(&'a str),
    StrCont(&'a str),
    StrEnd(&'a str),
    StrSimple(&'a str),
    Symbol(&'a str),
    Whitepace(&'a str),
    Unknown(&'a str),
}

#[derive(Clone, Copy)]
enum LexerState {
    InterpExpr { brace_depth: usize },
}

pub struct Lexer<'a> {
    source: &'a str,
    graphemes: Peekable<GraphemeIndices<'a>>,
    current_position: Position,
    state_stack: Vec<LexerState>,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            graphemes: source.grapheme_indices(true).peekable(),
            current_position: Position::default(),
            state_stack: Vec::new(),
        }
    }

    fn is_letter_grapheme(grapheme: &str) -> bool {
        grapheme
            .chars()
            .next()
            .is_some_and(|character| character.is_alphabetic())
    }

    fn is_letter_or_number_grapheme(grapheme: &str) -> bool {
        grapheme
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric())
    }

    fn is_whitespace_grapheme(grapheme: &str) -> bool {
        !grapheme.is_empty() && grapheme.chars().all(char::is_whitespace)
    }

    fn is_non_zero_ascii_digit_grapheme(grapheme: &str) -> bool {
        grapheme
            .chars()
            .next()
            .is_some_and(|character| matches!(character, '1'..='9'))
    }

    fn is_ascii_digit_grapheme(grapheme: &str) -> bool {
        grapheme
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    }

    fn advance_position_for_grapheme(&mut self, grapheme: &str) {
        if grapheme == "\n" || grapheme == "\r\n" {
            self.current_position.next_line();
        } else {
            self.current_position.next_char();
        }
    }

    fn next_grapheme(&mut self) -> Option<(usize, &'a str, Position)> {
        let (index, grapheme) = self.graphemes.next()?;
        let start = self.current_position;
        self.advance_position_for_grapheme(grapheme);
        Some((index, grapheme, start))
    }

    fn consume_string_from_quote(
        &mut self,
        quote_index: usize,
        quote_start: Position,
    ) -> Loc<Token<'a>> {
        let chunk_start = quote_index + "\"".len();
        let mut escaped = false;

        while let Some((index, grapheme, _)) = self.next_grapheme() {
            if escaped {
                escaped = false;
                continue;
            }
            if grapheme == "\\" {
                escaped = true;
                continue;
            }

            if grapheme == "{" {
                let raw = &self.source[chunk_start..index];
                self.state_stack
                    .push(LexerState::InterpExpr { brace_depth: 0 });
                return Loc::new(
                    Token::StrBegin(raw),
                    Span::new(quote_start, self.current_position),
                );
            }

            if grapheme == "\"" {
                let raw = &self.source[chunk_start..index];
                return Loc::new(
                    Token::StrSimple(raw),
                    Span::new(quote_start, self.current_position),
                );
            }
        }

        Loc::new(
            Token::Unknown("\""),
            Span::new(quote_start, self.current_position),
        )
    }

    fn consume_string_after_interp(
        &mut self,
        close_curly_start: Position,
        close_curly_index: usize,
    ) -> Loc<Token<'a>> {
        let chunk_start = close_curly_index + "}".len();
        let mut escaped = false;

        while let Some((index, grapheme, _)) = self.next_grapheme() {
            if escaped {
                escaped = false;
                continue;
            }
            if grapheme == "\\" {
                escaped = true;
                continue;
            }

            if grapheme == "{" {
                let raw = &self.source[chunk_start..index];
                return Loc::new(
                    Token::StrCont(raw),
                    Span::new(close_curly_start, self.current_position),
                );
            }

            if grapheme == "\"" {
                let raw = &self.source[chunk_start..index];
                let _ = self.state_stack.pop();
                return Loc::new(
                    Token::StrEnd(raw),
                    Span::new(close_curly_start, self.current_position),
                );
            }
        }

        Loc::new(
            Token::Unknown("}"),
            Span::new(close_curly_start, self.current_position),
        )
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Loc<Token<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        let (grapheme_index, grapheme, start) = self.next_grapheme()?;

        if let Some(LexerState::InterpExpr { brace_depth }) = self.state_stack.last_mut() {
            match grapheme {
                "{" => {
                    *brace_depth += 1;
                    return Some(Loc::new(
                        Token::OpenCurly,
                        Span::new(start, self.current_position),
                    ));
                }
                "}" => {
                    if *brace_depth > 0 {
                        *brace_depth -= 1;
                        return Some(Loc::new(
                            Token::CloseCurly,
                            Span::new(start, self.current_position),
                        ));
                    }

                    return Some(self.consume_string_after_interp(start, grapheme_index));
                }
                _ => {}
            }
        }

        let token = if Self::is_letter_grapheme(grapheme) {
            let symbol_start = grapheme_index;
            let mut symbol_end = grapheme_index + grapheme.len();

            while let Some((_, next_grapheme)) = self.graphemes.peek().copied() {
                if !Self::is_letter_or_number_grapheme(next_grapheme) {
                    break;
                }

                let (next_index, next_grapheme, _) =
                    self.next_grapheme().expect("peek returned Some");
                symbol_end = next_index + next_grapheme.len();
            }

            let symbol = &self.source[symbol_start..symbol_end];
            if symbol == "import" {
                Token::ImportKeyword
            } else if symbol == "let" {
                Token::LetKeyword
            } else if symbol == "fn" {
                Token::FnKeyword
            } else if symbol == "export" {
                Token::ExportKeyword
            } else if symbol == "extern" {
                Token::ExternKeyword
            } else {
                Token::Symbol(symbol)
            }
        } else if Self::is_non_zero_ascii_digit_grapheme(grapheme) {
            let int_start = grapheme_index;
            let mut int_end = grapheme_index + grapheme.len();

            while let Some((_, next_grapheme)) = self.graphemes.peek().copied() {
                if !Self::is_ascii_digit_grapheme(next_grapheme) {
                    break;
                }

                let (next_index, next_grapheme, _) =
                    self.next_grapheme().expect("peek returned Some");
                int_end = next_index + next_grapheme.len();
            }

            Token::Int(&self.source[int_start..int_end])
        } else if Self::is_whitespace_grapheme(grapheme) {
            let whitespace_start = grapheme_index;
            let mut whitespace_end = grapheme_index + grapheme.len();

            while let Some((_, next_grapheme)) = self.graphemes.peek().copied() {
                if !Self::is_whitespace_grapheme(next_grapheme) {
                    break;
                }

                let (next_index, next_grapheme, _) =
                    self.next_grapheme().expect("peek returned Some");
                whitespace_end = next_index + next_grapheme.len();
            }

            let whitespace = &self.source[whitespace_start..whitespace_end];
            Token::Whitepace(whitespace)
        } else {
            match grapheme {
                "\"" => return Some(self.consume_string_from_quote(grapheme_index, start)),
                "{" => Token::OpenCurly,
                "}" => Token::CloseCurly,
                ":" => Token::Colon,
                "," => Token::Comma,
                "." => Token::Dot,
                "(" => Token::OpenParen,
                ")" => Token::CloseParen,
                "=" => Token::Equals,
                ";" => Token::Semicolon,
                "/" => Token::Slash,
                _ => Token::Unknown(grapheme),
            }
        };

        let end = self.current_position;
        Some(Loc::new(token, Span::new(start, end)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Lexer, Token};

    #[test]
    fn lexes_simple_string() {
        let tokens = Lexer::new("\"hello\"").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::StrSimple("hello")));
    }

    #[test]
    fn lexes_interpolated_string_segments() {
        let tokens = Lexer::new("\"a:{x}.\"").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens[0].as_ref(), Token::StrBegin("a:")));
        assert!(matches!(tokens[1].as_ref(), Token::Symbol("x")));
        assert!(matches!(tokens[2].as_ref(), Token::StrEnd(".")));
    }

    #[test]
    fn escaped_open_curly_does_not_start_interpolation() {
        let tokens = Lexer::new("\"x\\{y\"").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::StrSimple("x\\{y")));
    }
}
