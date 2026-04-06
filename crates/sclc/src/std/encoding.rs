use ordered_float::NotNan;
use serde_json::{Map, Number, Value as JsonValue};

use crate::{Dict, EvalError, EvalErrorKind, Record, Value};

pub fn register_extern(eval: &mut crate::Eval<'_>) {
    eval.add_extern_fn("Std/Encoding.toJson", |args, _ctx| {
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));

        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }

        first.try_map(|value| {
            let json = to_json_value(&value)?;
            let encoded = serde_json::to_string(&json)
                .map_err(|err| EvalErrorKind::Custom(format!("failed to encode JSON: {err}")))?;
            Ok(Value::Str(encoded))
        })
    });

    eval.add_extern_fn("Std/Encoding.fromJson", |args, _ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));

        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }

        first.try_map(|value| {
            let input = value.assert_str()?;
            let json = serde_json::from_str::<JsonValue>(&input)
                .map_err(|err| EvalErrorKind::Custom(format!("invalid JSON: {err}")))?;
            from_json_value(json)
        })
    });
}

fn to_json_value(value: &Value) -> Result<JsonValue, EvalError> {
    match value {
        Value::Nil => Ok(JsonValue::Null),
        Value::Pending(_) => {
            Err(EvalErrorKind::Custom("cannot encode pending value as JSON".into()).into())
        }
        Value::Int(value) => Ok(JsonValue::Number(Number::from(*value))),
        Value::Float(value) => Number::from_f64(value.into_inner())
            .map(JsonValue::Number)
            .ok_or_else(|| EvalErrorKind::Custom("invalid float for JSON encoding".into()).into()),
        Value::Bool(value) => Ok(JsonValue::Bool(*value)),
        Value::Str(value) => Ok(JsonValue::String(value.clone())),
        Value::List(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(to_json_value(value)?);
            }
            Ok(JsonValue::Array(out))
        }
        Value::ExternFn(_) | Value::Fn(_) => {
            Err(EvalErrorKind::Custom("cannot encode function value as JSON".into()).into())
        }
        Value::Exception(_) => {
            Err(EvalErrorKind::Custom("cannot encode exception value as JSON".into()).into())
        }
        Value::Path(pv) => Ok(JsonValue::String(pv.path.clone())),
        Value::Record(record) => Ok(JsonValue::Object(record_to_map(record)?)),
        Value::Dict(dict) => Ok(JsonValue::Object(dict_to_map(dict)?)),
    }
}

fn record_to_map(record: &Record) -> Result<Map<String, JsonValue>, EvalError> {
    let mut map = Map::new();
    for (name, value) in record.iter() {
        map.insert(name.to_owned(), to_json_value(value)?);
    }
    Ok(map)
}

fn dict_to_map(dict: &Dict) -> Result<Map<String, JsonValue>, EvalError> {
    let mut map = Map::new();
    for (key, value) in dict.iter() {
        let key = match key {
            Value::Str(value) => value.clone(),
            other => {
                let json_key = to_json_value(other)?;
                serde_json::to_string(&json_key).map_err(|err| -> EvalError {
                    EvalErrorKind::Custom(format!("failed to encode dict key as JSON: {err}"))
                        .into()
                })?
            }
        };
        map.insert(key, to_json_value(value)?);
    }
    Ok(map)
}

fn from_json_value(value: JsonValue) -> Result<Value, EvalError> {
    match value {
        JsonValue::Null => Ok(Value::Nil),
        JsonValue::Bool(value) => Ok(Value::Bool(value)),
        JsonValue::Number(value) => {
            let value = value.as_f64().ok_or_else(|| -> EvalError {
                EvalErrorKind::Custom("JSON number is out of range for f64".into()).into()
            })?;
            let value = NotNan::new(value).map_err(|_| -> EvalError {
                EvalErrorKind::Custom("JSON number is NaN".into()).into()
            })?;
            Ok(Value::Float(value))
        }
        JsonValue::String(value) => Ok(Value::Str(value)),
        JsonValue::Array(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(from_json_value(value)?);
            }
            Ok(Value::List(out))
        }
        JsonValue::Object(values) => {
            let mut dict = Dict::default();
            for (key, value) in values {
                dict.insert(Value::Str(key), from_json_value(value)?);
            }
            Ok(Value::Dict(dict))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_json_encodes_records_and_dicts() {
        let mut record = Record::default();
        record.insert("answer".into(), Value::Int(42));
        record.insert("name".into(), Value::Str("sky".into()));

        let mut dict = Dict::default();
        dict.insert(Value::Int(1), Value::Bool(true));
        dict.insert(Value::Str("x".into()), Value::Nil);

        let mut root = Record::default();
        root.insert("record".into(), Value::Record(record));
        root.insert("dict".into(), Value::Dict(dict));

        let json = to_json_value(&Value::Record(root)).unwrap();
        let encoded = serde_json::to_string(&json).unwrap();

        assert_eq!(
            encoded,
            r#"{"dict":{"1":true,"x":null},"record":{"answer":42,"name":"sky"}}"#
        );
    }

    #[test]
    fn from_json_decodes_numbers_to_floats() {
        let json = serde_json::json!({
            "values": [1, 2.5, true, null, "hi"]
        });

        let value = from_json_value(json).unwrap();
        let Value::Dict(dict) = value else {
            panic!("expected dict");
        };

        let mut found = None;
        for (key, value) in dict.iter() {
            if let Value::Str(key) = key
                && key == "values"
            {
                found = Some(value.clone());
            }
        }

        let Some(Value::List(values)) = found else {
            panic!("expected list");
        };

        assert!(matches!(values[0], Value::Float(_)));
        assert!(matches!(values[1], Value::Float(_)));
        assert!(matches!(values[2], Value::Bool(true)));
        assert!(matches!(values[3], Value::Nil));
        assert!(matches!(values[4], Value::Str(_)));
    }
}
