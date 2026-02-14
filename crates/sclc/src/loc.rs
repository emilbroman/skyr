#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(packed)]
pub struct Position {
    line: u32,
    character: u32,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span {
    start: Position,
    end: Position,
}

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Loc<T> {
    value: T,
    span: Span,
}

impl Position {
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }

    pub const fn line(self) -> u32 {
        self.line
    }

    pub const fn character(self) -> u32 {
        self.character
    }

    pub fn next_char(&mut self) {
        self.character += 1;
    }

    pub fn next_line(&mut self) {
        self.line += 1;
        self.character = 1;
    }
}

impl Default for Position {
    fn default() -> Self {
        Self::new(1, 1)
    }
}

impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = *self;
        write!(f, "{}:{}", p.line(), p.character())
    }
}

impl std::fmt::Debug for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl Span {
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    pub const fn start(self) -> Position {
        self.start
    }

    pub const fn end(self) -> Position {
        self.end
    }
}

impl<T> Loc<T> {
    pub const fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }

    pub const fn span(&self) -> Span {
        self.span
    }

    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T> AsRef<T> for Loc<T> {
    fn as_ref(&self) -> &T {
        &self.value
    }
}

impl<T> AsMut<T> for Loc<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T> std::ops::Deref for Loc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> std::ops::DerefMut for Loc<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = *self;
        write!(f, "{},{}", s.start(), s.end())
    }
}

impl std::fmt::Debug for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::{Loc, Position, Span};

    #[test]
    fn position_size_is_8_bytes() {
        assert_eq!(std::mem::size_of::<Position>(), 8);
    }

    #[test]
    fn span_holds_start_and_end_positions() {
        let start = Position::new(1, 2);
        let end = Position::new(3, 4);
        let span = Span::new(start, end);
        assert!(span.start() == start);
        assert!(span.end() == end);
    }

    #[test]
    fn formatting() {
        let start = Position::new(1, 3);
        let end = Position::new(2, 5);
        let span = Span::new(start, end);

        assert_eq!(format!("{}", start), "1:3");
        assert_eq!(format!("{:?}", start), "1:3");
        assert_eq!(format!("{}", span), "1:3,2:5");
        assert_eq!(format!("{:?}", span), "1:3,2:5");
    }

    #[test]
    fn position_default_is_one_based() {
        assert_eq!(Position::default().line(), 1);
        assert_eq!(Position::default().character(), 1);
    }

    #[test]
    fn loc_wraps_value_with_span() {
        let span = Span::new(Position::new(1, 1), Position::new(1, 4));
        let mut loc = Loc::new(String::from("abc"), span);

        assert_eq!(loc.span(), span);
        assert_eq!(loc.as_ref(), "abc");

        loc.as_mut().push('d');
        assert_eq!(&*loc, "abcd");
        assert_eq!(loc.into_inner(), "abcd");
    }
}
