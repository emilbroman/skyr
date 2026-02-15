#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Int,
    Never,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Never => write!(f, "Never"),
        }
    }
}
