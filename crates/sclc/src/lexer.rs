use std::iter::Peekable;
use unicode_segmentation::{GraphemeIndices, UnicodeSegmentation};

use crate::{Loc, Position, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token<'a> {
    OpenCurly,
    CloseCurly,
    Hash,
    Colon,
    Comma,
    Dot,
    OpenParen,
    CloseParen,
    OpenSquare,
    CloseSquare,
    Equals,
    EqEq,
    Semicolon,
    Slash,
    Plus,
    Minus,
    Star,
    BangEq,
    Less,
    LessColon,
    LessEq,
    Greater,
    GreaterEq,
    AndAnd,
    OrOr,
    ImportKeyword,
    LetKeyword,
    FnKeyword,
    ExportKeyword,
    ExternKeyword,
    IfKeyword,
    ElseKeyword,
    ForKeyword,
    InKeyword,
    NilKeyword,
    TrueKeyword,
    FalseKeyword,
    ExceptionKeyword,
    RaiseKeyword,
    TryKeyword,
    CatchKeyword,
    TypeKeyword,
    AsKeyword,
    QuestionMark,
    QuestionQuestion,
    Int(&'a str),
    Float(&'a str),
    StrBegin(&'a str),
    StrCont(&'a str),
    StrEnd(&'a str),
    StrSimple(&'a str),
    Symbol(&'a str),
    Whitepace(&'a str),
    Comment(&'a str),
    DocComment(&'a str),
    Unknown(&'a str),
    Cursor { content: &'a str, offset: usize },
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
    cursor: Option<crate::Cursor>,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            graphemes: source.grapheme_indices(true).peekable(),
            current_position: Position::default(),
            state_stack: Vec::new(),
            cursor: None,
        }
    }

    pub fn with_cursor(source: &'a str, cursor: crate::Cursor) -> Self {
        Self {
            source,
            graphemes: source.grapheme_indices(true).peekable(),
            current_position: Position::default(),
            state_stack: Vec::new(),
            cursor: Some(cursor),
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

    fn peek_is_float_fraction_start(&self) -> bool {
        let mut iter = self.graphemes.clone();
        let Some((_, dot)) = iter.next() else {
            return false;
        };
        if dot != "." {
            return false;
        }
        let Some((_, digit)) = iter.next() else {
            return false;
        };
        Self::is_ascii_digit_grapheme(digit)
    }

    fn consume_float_fraction(&mut self) -> usize {
        let (dot_index, dot_grapheme, _) = self.next_grapheme().expect("peek returned Some");
        let mut float_end = dot_index + dot_grapheme.len();

        while let Some((_, next_grapheme)) = self.graphemes.peek().copied() {
            if !Self::is_ascii_digit_grapheme(next_grapheme) {
                break;
            }

            let (next_index, next_grapheme, _) = self.next_grapheme().expect("peek returned Some");
            float_end = next_index + next_grapheme.len();
        }

        float_end
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

    fn consume_line_comment(
        &mut self,
        comment_start: usize,
        comment_start_position: Position,
    ) -> Loc<Token<'a>> {
        let mut comment_end = comment_start + "//".len();

        while let Some((_, next_grapheme)) = self.graphemes.peek().copied() {
            if next_grapheme == "\n" || next_grapheme == "\r\n" {
                break;
            }

            let (next_index, next_grapheme, _) = self.next_grapheme().expect("peek returned Some");
            comment_end = next_index + next_grapheme.len();
        }

        let comment = &self.source[comment_start..comment_end];
        Loc::new(
            Token::Comment(comment),
            Span::new(comment_start_position, self.current_position),
        )
    }

    fn consume_doc_comment(
        &mut self,
        comment_start: usize,
        comment_start_position: Position,
    ) -> Loc<Token<'a>> {
        let mut comment_end = comment_start + "///".len();

        while let Some((_, next_grapheme)) = self.graphemes.peek().copied() {
            if next_grapheme == "\n" || next_grapheme == "\r\n" {
                break;
            }

            let (next_index, next_grapheme, _) = self.next_grapheme().expect("peek returned Some");
            comment_end = next_index + next_grapheme.len();
        }

        let comment = &self.source[comment_start..comment_end];
        Loc::new(
            Token::DocComment(comment),
            Span::new(comment_start_position, self.current_position),
        )
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Loc<Token<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        // Case A: cursor is between tokens (at the current position before consuming).
        // However, if the next character starts a symbol, defer to Case B so that the
        // cursor token carries the symbol content (needed for hover/go-to-def on the
        // first character of a variable).
        if self
            .cursor
            .as_ref()
            .is_some_and(|c| c.position == self.current_position)
        {
            let next_is_symbol = self
                .graphemes
                .peek()
                .is_some_and(|(_, g)| Self::is_letter_grapheme(g));
            if !next_is_symbol {
                self.cursor.take();
                let span = Span::new(self.current_position, self.current_position);
                return Some(Loc::new(
                    Token::Cursor {
                        content: "",
                        offset: 0,
                    },
                    span,
                ));
            }
        }

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
            } else if symbol == "if" {
                Token::IfKeyword
            } else if symbol == "else" {
                Token::ElseKeyword
            } else if symbol == "for" {
                Token::ForKeyword
            } else if symbol == "in" {
                Token::InKeyword
            } else if symbol == "nil" {
                Token::NilKeyword
            } else if symbol == "true" {
                Token::TrueKeyword
            } else if symbol == "false" {
                Token::FalseKeyword
            } else if symbol == "exception" {
                Token::ExceptionKeyword
            } else if symbol == "raise" {
                Token::RaiseKeyword
            } else if symbol == "try" {
                Token::TryKeyword
            } else if symbol == "catch" {
                Token::CatchKeyword
            } else if symbol == "type" {
                Token::TypeKeyword
            } else if symbol == "as" {
                Token::AsKeyword
            } else {
                // Case B: cursor inside a symbol (not a keyword)
                if let Some(cursor_pos) = self.cursor.as_ref().map(|c| c.position) {
                    let symbol_start_pos = start;
                    let symbol_end_pos = self.current_position;
                    if cursor_pos >= symbol_start_pos && cursor_pos <= symbol_end_pos {
                        self.cursor.take();
                        // Compute byte offset: count characters from start to cursor
                        let char_offset = cursor_pos.character() - symbol_start_pos.character();
                        // Convert character offset to byte offset
                        let byte_offset = symbol
                            .char_indices()
                            .nth(char_offset as usize)
                            .map(|(i, _)| i)
                            .unwrap_or(symbol.len());
                        return Some(Loc::new(
                            Token::Cursor {
                                content: symbol,
                                offset: byte_offset,
                            },
                            Span::new(symbol_start_pos, symbol_end_pos),
                        ));
                    }
                }
                Token::Symbol(symbol)
            }
        } else if grapheme == "0" {
            let int_start = grapheme_index;
            let mut int_end = grapheme_index + grapheme.len();
            let mut has_trailing_digit = false;

            while let Some((_, next_grapheme)) = self.graphemes.peek().copied() {
                if !Self::is_ascii_digit_grapheme(next_grapheme) {
                    break;
                }

                has_trailing_digit = true;
                let (next_index, next_grapheme, _) =
                    self.next_grapheme().expect("peek returned Some");
                int_end = next_index + next_grapheme.len();
            }

            if self.peek_is_float_fraction_start() {
                let float_end = self.consume_float_fraction();
                if has_trailing_digit {
                    Token::Unknown(&self.source[int_start..float_end])
                } else {
                    Token::Float(&self.source[int_start..float_end])
                }
            } else if has_trailing_digit {
                Token::Unknown(&self.source[int_start..int_end])
            } else {
                Token::Int(&self.source[int_start..int_end])
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

            if self.peek_is_float_fraction_start() {
                let float_end = self.consume_float_fraction();
                Token::Float(&self.source[int_start..float_end])
            } else {
                Token::Int(&self.source[int_start..int_end])
            }
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
                "!" => {
                    if let Some((_, "=")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        Token::BangEq
                    } else {
                        Token::Unknown(grapheme)
                    }
                }
                "&" => {
                    if let Some((_, "&")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        Token::AndAnd
                    } else {
                        Token::Unknown(grapheme)
                    }
                }
                "|" => {
                    if let Some((_, "|")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        Token::OrOr
                    } else {
                        Token::Unknown(grapheme)
                    }
                }
                "\"" => return Some(self.consume_string_from_quote(grapheme_index, start)),
                "/" => {
                    if let Some((_, "/")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        // Check for a third slash: `///` is a doc comment
                        if let Some((_, "/")) = self.graphemes.peek().copied() {
                            self.next_grapheme().expect("peek returned Some");
                            return Some(self.consume_doc_comment(grapheme_index, start));
                        }
                        return Some(self.consume_line_comment(grapheme_index, start));
                    }

                    Token::Slash
                }
                "{" => Token::OpenCurly,
                "}" => Token::CloseCurly,
                "#" => Token::Hash,
                ":" => Token::Colon,
                "," => Token::Comma,
                "." => Token::Dot,
                "(" => Token::OpenParen,
                ")" => Token::CloseParen,
                "[" => Token::OpenSquare,
                "]" => Token::CloseSquare,
                "=" => {
                    if let Some((_, "=")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        Token::EqEq
                    } else {
                        Token::Equals
                    }
                }
                "<" => {
                    if let Some((_, ":")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        Token::LessColon
                    } else if let Some((_, "=")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        Token::LessEq
                    } else {
                        Token::Less
                    }
                }
                ">" => {
                    if let Some((_, "=")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        Token::GreaterEq
                    } else {
                        Token::Greater
                    }
                }
                ";" => Token::Semicolon,
                "+" => Token::Plus,
                "-" => Token::Minus,
                "*" => Token::Star,
                "?" => {
                    if let Some((_, "?")) = self.graphemes.peek().copied() {
                        self.next_grapheme().expect("peek returned Some");
                        Token::QuestionQuestion
                    } else {
                        Token::QuestionMark
                    }
                }
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

    #[test]
    fn lexes_zero_as_int() {
        let tokens = Lexer::new("0").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::Int("0")));
    }

    #[test]
    fn rejects_leading_zero_in_integer() {
        let tokens = Lexer::new("012").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::Unknown("012")));
    }

    #[test]
    fn lexes_float_literal() {
        let tokens = Lexer::new("3.14").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::Float("3.14")));
    }

    #[test]
    fn lexes_float_with_leading_zero() {
        let tokens = Lexer::new("0.12").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::Float("0.12")));
    }

    #[test]
    fn rejects_leading_zero_in_float() {
        let tokens = Lexer::new("01.2").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::Unknown("01.2")));
    }

    #[test]
    fn lexes_plus_token() {
        let tokens = Lexer::new("+").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::Plus));
    }

    #[test]
    fn lexes_minus_token() {
        let tokens = Lexer::new("-").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::Minus));
    }

    #[test]
    fn lexes_star_token() {
        let tokens = Lexer::new("*").collect::<Vec<_>>();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0].as_ref(), Token::Star));
    }

    #[test]
    fn lexes_equality_and_comparison_tokens() {
        let tokens = Lexer::new("== != < <= > >=")
            .filter(|token| !matches!(token.as_ref(), Token::Whitepace(_)))
            .collect::<Vec<_>>();
        assert_eq!(tokens.len(), 6);
        assert!(matches!(tokens[0].as_ref(), Token::EqEq));
        assert!(matches!(tokens[1].as_ref(), Token::BangEq));
        assert!(matches!(tokens[2].as_ref(), Token::Less));
        assert!(matches!(tokens[3].as_ref(), Token::LessEq));
        assert!(matches!(tokens[4].as_ref(), Token::Greater));
        assert!(matches!(tokens[5].as_ref(), Token::GreaterEq));
    }

    #[test]
    fn lexes_boolean_operator_tokens() {
        let tokens = Lexer::new("&& ||")
            .filter(|token| !matches!(token.as_ref(), Token::Whitepace(_)))
            .collect::<Vec<_>>();
        assert_eq!(tokens.len(), 2);
        assert!(matches!(tokens[0].as_ref(), Token::AndAnd));
        assert!(matches!(tokens[1].as_ref(), Token::OrOr));
    }

    #[test]
    fn lexes_line_comment() {
        let tokens = Lexer::new("let x = 1 // hi\nlet y = 2")
            .filter(|token| !matches!(token.as_ref(), Token::Whitepace(_)))
            .collect::<Vec<_>>();
        assert!(
            tokens
                .iter()
                .any(|token| matches!(token.as_ref(), Token::Comment("// hi")))
        );
    }

    #[test]
    fn cursor_between_tokens() {
        // "a = b" with cursor at 1:3 (the `=` sign position, but checked before consuming `=`)
        // After consuming 'a' (1:1) and ' ' (1:2), current_position is 1:3
        // Cursor at 1:3 fires Case A before consuming `=`
        let cursor = crate::Cursor::new(crate::Position::new(1, 3));
        let tokens: Vec<_> = Lexer::with_cursor("a = b", cursor).collect();
        let non_ws: Vec<_> = tokens
            .iter()
            .filter(|t| !matches!(t.as_ref(), Token::Whitepace(_)))
            .collect();
        assert!(matches!(non_ws[0].as_ref(), Token::Symbol("a")));
        assert!(
            matches!(
                non_ws[1].as_ref(),
                Token::Cursor {
                    content: "",
                    offset: 0,
                    ..
                }
            ),
            "expected cursor between tokens, got {:?}",
            non_ws[1].as_ref()
        );
        assert!(matches!(non_ws[2].as_ref(), Token::Equals));
        assert!(matches!(non_ws[3].as_ref(), Token::Symbol("b")));
    }

    #[test]
    fn cursor_at_start_of_symbol_is_inside_symbol() {
        // "foo" with cursor at 1:1 (start of foo) — should be Case B so that
        // the cursor token carries the symbol content for hover/go-to-def.
        let cursor = crate::Cursor::new(crate::Position::new(1, 1));
        let tokens: Vec<_> = Lexer::with_cursor("foo", cursor).collect();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(
                tokens[0].as_ref(),
                Token::Cursor {
                    content: "foo",
                    offset: 0,
                    ..
                }
            ),
            "cursor at start of symbol should be Case B (inside symbol), got {:?}",
            tokens[0].as_ref()
        );
    }

    #[test]
    fn cursor_in_middle_of_symbol() {
        // "hello" with cursor at 1:4 (after "hel", before "lo")
        let cursor = crate::Cursor::new(crate::Position::new(1, 4));
        let tokens: Vec<_> = Lexer::with_cursor("hello", cursor).collect();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(
                tokens[0].as_ref(),
                Token::Cursor {
                    content: "hello",
                    offset: 3,
                    ..
                }
            ),
            "expected cursor inside symbol at offset 3, got {:?}",
            tokens[0].as_ref()
        );
    }

    #[test]
    fn cursor_at_end_of_symbol() {
        // "bar" with cursor at 1:4 (after "bar")
        let cursor = crate::Cursor::new(crate::Position::new(1, 4));
        let tokens: Vec<_> = Lexer::with_cursor("bar", cursor).collect();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(
                tokens[0].as_ref(),
                Token::Cursor {
                    content: "bar",
                    offset: 3,
                    ..
                }
            ),
            "expected cursor at end of symbol, got {:?}",
            tokens[0].as_ref()
        );
    }

    #[test]
    fn cursor_inside_keyword_emits_keyword() {
        // "export" with cursor at 1:4 (inside the keyword)
        let cursor = crate::Cursor::new(crate::Position::new(1, 4));
        let tokens: Vec<_> = Lexer::with_cursor("export", cursor).collect();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(tokens[0].as_ref(), Token::ExportKeyword),
            "cursor inside keyword should still emit keyword token"
        );
    }

    #[test]
    fn no_cursor_no_cursor_token() {
        let tokens: Vec<_> = Lexer::new("foo bar").collect();
        assert!(
            !tokens
                .iter()
                .any(|t| matches!(t.as_ref(), Token::Cursor { .. })),
            "no Cursor token should be emitted without a cursor"
        );
    }
}
