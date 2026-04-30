use ids::ObjId;
use ordered_float::NotNan;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathValue {
    pub path: String,
    pub hash: ObjId,
}

impl Serialize for PathValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("PathValue", 2)?;
        s.serialize_field("path", &self.path)?;
        s.serialize_field("hash", &self.hash.to_string())?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for PathValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            path: String,
            hash: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        let hash: ObjId = raw.hash.parse().map_err(serde::de::Error::custom)?;
        Ok(PathValue {
            path: raw.path,
            hash,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    Nil,
    Pending(PendingValue),
    Int(i64),
    Float(NotNan<f64>),
    Bool(bool),
    Str(String),
    Path(PathValue),
    List(Vec<Value>),
    ExternFn(ExternFnValue),
    Fn(FnValue),
    Record(Record),
    Dict(Dict),
    Exception(ExceptionValue),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackedValue {
    pub value: Value,
    pub dependencies: BTreeSet<ids::ResourceId>,
}

impl TrackedValue {
    pub fn new(value: Value) -> Self {
        Self {
            value,
            dependencies: BTreeSet::new(),
        }
    }

    pub fn pending() -> Self {
        Self::new(Value::Pending(PendingValue))
    }

    pub fn with_dependencies(mut self, dependencies: BTreeSet<ids::ResourceId>) -> Self {
        self.dependencies.extend(dependencies);
        self
    }

    pub fn with_dependency(mut self, dependency: ids::ResourceId) -> Self {
        self.dependencies.insert(dependency);
        self
    }

    pub fn map(self, f: impl FnOnce(Value) -> Value) -> Self {
        Self {
            value: f(self.value),
            dependencies: self.dependencies,
        }
    }

    pub fn try_map<E>(self, f: impl FnOnce(Value) -> Result<Value, E>) -> Result<Self, E> {
        Ok(Self {
            value: f(self.value)?,
            dependencies: self.dependencies,
        })
    }

    pub fn flat_map(self, f: impl FnOnce(Value) -> Self) -> Self {
        let mut mapped = f(self.value);
        mapped.dependencies.extend(self.dependencies);
        mapped
    }

    pub fn try_flat_map<E>(self, f: impl FnOnce(Value) -> Result<Self, E>) -> Result<Self, E> {
        let mut mapped = f(self.value)?;
        mapped.dependencies.extend(self.dependencies);
        Ok(mapped)
    }
}

impl From<Value> for TrackedValue {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExceptionValue {
    pub exception_id: u64,
    pub payload: Box<Value>,
}

impl Serialize for ExceptionValue {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(serde::ser::Error::custom(
            "cannot serialize exception value",
        ))
    }
}

impl<'de> Deserialize<'de> for ExceptionValue {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "cannot deserialize exception value",
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingValue;

impl Serialize for PendingValue {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(serde::ser::Error::custom("cannot serialize pending value"))
    }
}

impl<'de> Deserialize<'de> for PendingValue {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(serde::de::Error::custom("cannot deserialize pending value"))
    }
}

pub trait ExternFn: Send + Sync + 'static {
    fn call(
        &self,
        args: Vec<TrackedValue>,
        ctx: &crate::EvalCtx,
    ) -> Result<TrackedValue, crate::EvalError>;
    fn clone_extern_fn(&self) -> Box<dyn ExternFn>;
}

impl<F> ExternFn for F
where
    F: Fn(Vec<TrackedValue>, &crate::EvalCtx) -> Result<TrackedValue, crate::EvalError>
        + Clone
        + Send
        + Sync
        + 'static,
{
    fn call(
        &self,
        args: Vec<TrackedValue>,
        ctx: &crate::EvalCtx,
    ) -> Result<TrackedValue, crate::EvalError> {
        self(args, ctx)
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

    pub fn call(
        &self,
        args: Vec<TrackedValue>,
        ctx: &crate::EvalCtx,
    ) -> Result<TrackedValue, crate::EvalError> {
        self.0.call(args, ctx)
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dict {
    entries: Vec<(Value, Value)>,
}

impl Record {
    const CONST_NIL: Value = Value::Nil;

    pub fn insert(&mut self, name: String, value: Value) {
        self.fields.insert(name, value);
    }

    pub fn get(&self, name: &str) -> &Value {
        self.fields.get(name).unwrap_or(&Self::CONST_NIL)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.fields
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }
}

impl Dict {
    pub fn insert(&mut self, key: Value, value: Value) {
        self.entries.push((key, value));
    }

    pub fn get(&self, key: &Value) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }
}

impl Value {
    /// Recursively checks whether this value or any nested value is pending.
    pub fn has_pending(&self) -> bool {
        match self {
            Value::Pending(_) => true,
            Value::List(values) => values.iter().any(Value::has_pending),
            Value::Record(record) => record.iter().any(|(_, v)| v.has_pending()),
            Value::Dict(dict) => dict.iter().any(|(k, v)| k.has_pending() || v.has_pending()),
            _ => false,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Pending(_) => write!(f, "<pending>"),
            Value::Int(value) => write!(f, "{value}"),
            Value::Float(value) => write!(f, "{value}"),
            Value::Bool(value) => write!(f, "{value}"),
            Value::Str(value) => write!(f, "{value:?}"),
            Value::Path(pv) => write!(f, "{}", pv.path),
            Value::List(values) => {
                write!(f, "[")?;
                let mut values = values.iter().peekable();
                while let Some(value) = values.next() {
                    write!(f, "{value}")?;
                    if values.peek().is_some() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, "]")
            }
            Value::ExternFn(_) => write!(f, "<extern fn>"),
            Value::Fn(function) => write!(f, "{function}"),
            Value::Record(record) => write!(f, "{record}"),
            Value::Dict(dict) => write!(f, "{dict}"),
            Value::Exception(exc) => write!(f, "<exception#{}>", exc.exception_id),
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

        write!(f, ")")
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

impl std::fmt::Display for Dict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{{")?;

        let mut entries = self.entries.iter().peekable();
        while let Some((key, value)) = entries.next() {
            write!(f, "{key}: {value}")?;
            if entries.peek().is_some() {
                write!(f, ", ")?;
            }
        }

        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::{TrackedValue, Value};
    use ids::ResourceId;

    fn dep(name: &str) -> ResourceId {
        ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn tracked_value_flat_map_merges_dependencies() {
        let outer = TrackedValue::new(Value::Int(1)).with_dependency(dep("outer"));
        let inner = dep("inner");
        let mapped = outer.flat_map(|value| {
            assert_eq!(value, Value::Int(1));
            TrackedValue::new(Value::Int(2)).with_dependency(inner.clone())
        });

        assert_eq!(mapped.value, Value::Int(2));
        assert!(mapped.dependencies.contains(&dep("outer")));
        assert!(mapped.dependencies.contains(&dep("inner")));
    }

    #[test]
    fn tracked_value_try_flat_map_merges_dependencies() {
        let outer = TrackedValue::new(Value::Str("x".to_string())).with_dependency(dep("outer"));
        let inner = dep("inner");
        let mapped: Result<TrackedValue, ()> = outer.try_flat_map(|value| {
            assert_eq!(value, Value::Str("x".to_string()));
            Ok(TrackedValue::new(Value::Str("y".to_string())).with_dependency(inner.clone()))
        });
        let mapped = mapped.expect("mapping should succeed");

        assert_eq!(mapped.value, Value::Str("y".to_string()));
        assert!(mapped.dependencies.contains(&dep("outer")));
        assert!(mapped.dependencies.contains(&dep("inner")));
    }

    #[test]
    fn tracked_value_map_preserves_dependencies() {
        let outer = TrackedValue::new(Value::Int(1)).with_dependency(dep("outer"));
        let mapped = outer.map(|_| Value::Int(2));

        assert_eq!(mapped.value, Value::Int(2));
        assert!(mapped.dependencies.contains(&dep("outer")));
    }

    #[test]
    fn tracked_value_try_map_preserves_dependencies() {
        let outer = TrackedValue::new(Value::Int(1)).with_dependency(dep("outer"));
        let mapped: Result<TrackedValue, ()> = outer.try_map(|_| Ok(Value::Int(2)));
        let mapped = mapped.expect("mapping should succeed");

        assert_eq!(mapped.value, Value::Int(2));
        assert!(mapped.dependencies.contains(&dep("outer")));
    }
}
