use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    Nil,
    Int(i64),
    Str(String),
    ExternFn(ExternFnValue),
    Fn(FnValue),
    Record(Record),
}

pub trait ExternFn: Send + Sync + 'static {
    fn call(&self, args: Vec<Value>) -> Result<Value, crate::EvalError>;
    fn clone_extern_fn(&self) -> Box<dyn ExternFn>;
}

impl<F> ExternFn for F
where
    F: Fn(Vec<Value>) -> Result<Value, crate::EvalError> + Clone + Send + Sync + 'static,
{
    fn call(&self, args: Vec<Value>) -> Result<Value, crate::EvalError> {
        self(args)
    }

    fn clone_extern_fn(&self) -> Box<dyn ExternFn> {
        Box::new(self.clone())
    }
}

pub struct ExternFnValue(Box<dyn ExternFn>);

impl ExternFnValue {
    pub fn new(inner: Box<dyn ExternFn>) -> Self {
        Self(inner)
    }

    pub fn call(&self, args: Vec<Value>) -> Result<Value, crate::EvalError> {
        self.0.call(args)
    }
}

impl Clone for ExternFnValue {
    fn clone(&self) -> Self {
        Self(self.0.clone_extern_fn())
    }
}

impl std::fmt::Debug for ExternFnValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ExternFnValue(<dyn ExternFn>)")
    }
}

impl PartialEq for ExternFnValue {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Eq for ExternFnValue {}

impl Serialize for ExternFnValue {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(serde::ser::Error::custom("cannot serialize function value"))
    }
}

impl<'de> Deserialize<'de> for ExternFnValue {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "cannot deserialize function value",
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnValue {
    pub env: crate::FnEnv,
    pub body: crate::Loc<crate::ast::Expr>,
}

impl Serialize for FnValue {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(serde::ser::Error::custom("cannot serialize function value"))
    }
}

impl<'de> Deserialize<'de> for FnValue {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "cannot deserialize function value",
        ))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
            Value::ExternFn(_) => write!(f, "<extern fn>"),
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
