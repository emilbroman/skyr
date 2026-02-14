use std::iter::Peekable;
use unicode_segmentation::{GraphemeIndices, UnicodeSegmentation};

use crate::{Loc, Position, Span};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Token<'a> {
    OpenCurly,
    CloseCurly,
    Slash,
    ImportKeyword,
    Symbol(&'a str),
    Whitepace(&'a str),
    Unknown(&'a str),
}

pub struct Lexer<'a> {
    source: &'a str,
    graphemes: Peekable<GraphemeIndices<'a>>,
    current_position: Position,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            graphemes: source.grapheme_indices(true).peekable(),
            current_position: Position::default(),
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
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Loc<Token<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        let (grapheme_index, grapheme, start) = self.next_grapheme()?;

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
            } else {
                Token::Symbol(symbol)
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
                "{" => Token::OpenCurly,
                "}" => Token::CloseCurly,
                "/" => Token::Slash,
                _ => Token::Unknown(grapheme),
            }
        };

        let end = self.current_position;
        Some(Loc::new(token, Span::new(start, end)))
    }
}
