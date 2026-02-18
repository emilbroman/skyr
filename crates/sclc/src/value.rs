use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Nil,
    Int(i64),
    Str(String),
    Fn(FnValue),
    Record(Record),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnValue {
    pub env: crate::FnEnv,
    pub body: crate::Loc<crate::ast::Expr>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    fields: BTreeMap<String, Value>,
}

impl Record {
    pub fn insert(&mut self, name: String, value: Value) {
        self.fields.insert(name, value);
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Int(value) => write!(f, "{value}"),
            Value::Str(value) => write!(f, "{value}"),
            Value::Fn(function) => write!(f, "{function}"),
            Value::Record(record) => write!(f, "{record}"),
        }
    }
}

impl std::fmt::Display for FnValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fn(")?;

        let mut params = self.env.parameters.iter().peekable();
        while let Some(param) = params.next() {
            write!(f, "{param}")?;
            if params.peek().is_some() {
                write!(f, ", ")?;
            }
        }

        write!(f, ")>")
    }
}

impl std::fmt::Display for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{")?;

        let mut fields = self.fields.iter().peekable();
        while let Some((name, value)) = fields.next() {
            write!(f, "{name}: {value}")?;
            if fields.peek().is_some() {
                write!(f, ", ")?;
            }
        }

        write!(f, "}}")
    }
}
