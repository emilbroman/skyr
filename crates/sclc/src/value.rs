use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Value {
    Nil,
    Int(i64),
    Record(Record),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
            Value::Record(record) => write!(f, "{record}"),
        }
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
