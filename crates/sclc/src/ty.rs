use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Int,
    Record(RecordType),
    Never,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordType {
    fields: BTreeMap<String, Type>,
}

impl RecordType {
    pub fn insert(&mut self, name: String, ty: Type) {
        self.fields.insert(name, ty);
    }

    pub fn get(&self, name: &str) -> Option<&Type> {
        self.fields.get(name)
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Record(record) => write!(f, "{record}"),
            Type::Never => write!(f, "Never"),
        }
    }
}

impl std::fmt::Display for RecordType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{")?;

        let mut fields = self.fields.iter().peekable();
        while let Some((name, ty)) = fields.next() {
            write!(f, "{name}: {ty}")?;
            if fields.peek().is_some() {
                write!(f, ", ")?;
            }
        }

        write!(f, "}}")
    }
}
