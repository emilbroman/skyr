use std::iter::Peekable;
use unicode_segmentation::{Graphemes, UnicodeSegmentation};

use crate::{Loc, Position, Span};

pub enum Token<'a> {
    Unknown(&'a str),
}

pub struct Lexer<'a> {
    graphemes: Peekable<Graphemes<'a>>,
    current_position: Position,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            graphemes: source.graphemes(true).peekable(),
            current_position: Position::default(),
        }
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Loc<Token<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next()?;
        let start = self.current_position;
        self.current_position.next_char();
        let end = self.current_position;
        Some(Loc::new(Token::Unknown(grapheme), Span::new(start, end)))
    }
}
