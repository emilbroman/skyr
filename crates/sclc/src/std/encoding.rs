use base64::Engine as _;
use ordered_float::NotNan;
use serde_json::{Map, Number, Value as JsonValue};

use crate::{Dict, EvalError, EvalErrorKind, Record, Value};

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
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

    eval.add_extern_fn("Std/Encoding.toBase64", |args, _ctx| {
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
            let encoded = base64::engine::general_purpose::STANDARD.encode(input.as_bytes());
            Ok(Value::Str(encoded))
        })
    });

    eval.add_extern_fn("Std/Encoding.fromBase64", |args, _ctx| {
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
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(input.as_bytes())
                .map_err(|err| {
                    EvalErrorKind::Custom(format!("invalid base64: {err}"))
                })?;
            let s = String::from_utf8(bytes).map_err(|err| {
                EvalErrorKind::Custom(format!(
                    "base64-decoded bytes are not valid UTF-8: {err}"
                ))
            })?;
            Ok(Value::Str(s))
        })
    });

    eval.add_extern_fn("Std/Encoding.toBase64Url", |args, _ctx| {
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
            let encoded =
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input.as_bytes());
            Ok(Value::Str(encoded))
        })
    });

    eval.add_extern_fn("Std/Encoding.fromBase64Url", |args, _ctx| {
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
            // Accept both padded and unpadded by trying URL_SAFE_NO_PAD first (strips padding if present)
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(input.as_bytes())
                .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(input.as_bytes()))
                .map_err(|err| {
                    EvalErrorKind::Custom(format!("invalid URL-safe base64: {err}"))
                })?;
            let s = String::from_utf8(bytes).map_err(|err| {
                EvalErrorKind::Custom(format!(
                    "base64url-decoded bytes are not valid UTF-8: {err}"
                ))
            })?;
            Ok(Value::Str(s))
        })
    });

    eval.add_extern_fn("Std/Encoding.toHex", |args, _ctx| {
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
            let encoded = hex::encode(input.as_bytes());
            Ok(Value::Str(encoded))
        })
    });

    eval.add_extern_fn("Std/Encoding.fromHex", |args, _ctx| {
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
            let bytes = hex::decode(&input)
                .map_err(|err| EvalErrorKind::Custom(format!("invalid hex: {err}")))?;
            let s = String::from_utf8(bytes).map_err(|err| {
                EvalErrorKind::Custom(format!(
                    "hex-decoded bytes are not valid UTF-8: {err}"
                ))
            })?;
            Ok(Value::Str(s))
        })
    });

    eval.add_extern_fn("Std/Encoding.toYaml", |args, _ctx| {
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));

        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }

        first.try_map(|value| {
            let yaml = to_yaml_value(&value)?;
            let encoded = serde_yaml::to_string(&yaml)
                .map_err(|err| EvalErrorKind::Custom(format!("failed to encode YAML: {err}")))?;
            Ok(Value::Str(encoded))
        })
    });

    eval.add_extern_fn("Std/Encoding.fromYaml", |args, _ctx| {
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
            let yaml = serde_yaml::from_str::<serde_yaml::Value>(&input)
                .map_err(|err| EvalErrorKind::Custom(format!("invalid YAML: {err}")))?;
            from_yaml_value(yaml)
        })
    });

    eval.add_extern_fn("Std/Encoding.toToml", |args, _ctx| {
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(Value::Nil));

        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }

        first.try_map(|value| {
            let toml_val = to_toml_value(&value)?;
            // Top-level must be a table
            match &toml_val {
                toml::Value::Table(_) => {}
                _ => {
                    return Err(EvalErrorKind::Custom(
                        "toToml requires a Record or Dict at the top level".into(),
                    )
                    .into());
                }
            }
            let encoded = toml::to_string(&toml_val)
                .map_err(|err| EvalErrorKind::Custom(format!("failed to encode TOML: {err}")))?;
            Ok(Value::Str(encoded))
        })
    });

    eval.add_extern_fn("Std/Encoding.fromToml", |args, _ctx| {
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
            let toml_val = toml::from_str::<toml::Value>(&input)
                .map_err(|err| EvalErrorKind::Custom(format!("invalid TOML: {err}")))?;
            from_toml_value(toml_val)
        })
    });

    eval.add_extern_fn("Std/Encoding.urlEncode", |args, _ctx| {
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
            let encoded = urlencoding::encode(&input).into_owned();
            Ok(Value::Str(encoded))
        })
    });

    eval.add_extern_fn("Std/Encoding.urlDecode", |args, _ctx| {
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
            let decoded = urlencoding::decode(&input)
                .map_err(|err| {
                    EvalErrorKind::Custom(format!("invalid URL encoding: {err}"))
                })?
                .into_owned();
            Ok(Value::Str(decoded))
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

// ─── YAML helpers ─────────────────────────────────────────────────────────────

fn to_yaml_value(value: &Value) -> Result<serde_yaml::Value, EvalError> {
    match value {
        Value::Nil => Ok(serde_yaml::Value::Null),
        Value::Pending(_) => {
            Err(EvalErrorKind::Custom("cannot encode pending value as YAML".into()).into())
        }
        Value::Int(v) => Ok(serde_yaml::Value::Number(serde_yaml::Number::from(*v))),
        Value::Float(v) => {
            let f = v.into_inner();
            if f.is_nan() || f.is_infinite() {
                return Err(
                    EvalErrorKind::Custom("invalid float for YAML encoding".into()).into(),
                );
            }
            Ok(serde_yaml::Value::Number(serde_yaml::Number::from(f)))
        }
        Value::Bool(v) => Ok(serde_yaml::Value::Bool(*v)),
        Value::Str(v) => Ok(serde_yaml::Value::String(v.clone())),
        Value::List(values) => {
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                out.push(to_yaml_value(v)?);
            }
            Ok(serde_yaml::Value::Sequence(out))
        }
        Value::ExternFn(_) | Value::Fn(_) => {
            Err(EvalErrorKind::Custom("cannot encode function value as YAML".into()).into())
        }
        Value::Exception(_) => {
            Err(EvalErrorKind::Custom("cannot encode exception value as YAML".into()).into())
        }
        Value::Path(pv) => Ok(serde_yaml::Value::String(pv.path.clone())),
        Value::Record(record) => {
            let mut map = serde_yaml::Mapping::new();
            for (name, v) in record.iter() {
                map.insert(
                    serde_yaml::Value::String(name.to_owned()),
                    to_yaml_value(v)?,
                );
            }
            Ok(serde_yaml::Value::Mapping(map))
        }
        Value::Dict(dict) => {
            let mut map = serde_yaml::Mapping::new();
            for (key, v) in dict.iter() {
                let yaml_key = match key {
                    Value::Str(s) => serde_yaml::Value::String(s.clone()),
                    other => to_yaml_value(other)?,
                };
                map.insert(yaml_key, to_yaml_value(v)?);
            }
            Ok(serde_yaml::Value::Mapping(map))
        }
    }
}

fn from_yaml_value(value: serde_yaml::Value) -> Result<Value, EvalError> {
    match value {
        serde_yaml::Value::Null => Ok(Value::Nil),
        serde_yaml::Value::Bool(v) => Ok(Value::Bool(v)),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else if let Some(f) = n.as_f64() {
                let nn = NotNan::new(f).map_err(|_| -> EvalError {
                    EvalErrorKind::Custom("YAML number is NaN".into()).into()
                })?;
                Ok(Value::Float(nn))
            } else {
                Err(EvalErrorKind::Custom("YAML number is out of range".into()).into())
            }
        }
        serde_yaml::Value::String(s) => Ok(Value::Str(s)),
        serde_yaml::Value::Sequence(values) => {
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                out.push(from_yaml_value(v)?);
            }
            Ok(Value::List(out))
        }
        serde_yaml::Value::Mapping(map) => {
            let mut dict = Dict::default();
            for (key, v) in map {
                let key_str = match key {
                    serde_yaml::Value::String(s) => s,
                    other => {
                        return Err(EvalErrorKind::Custom(format!(
                            "YAML mapping key must be a string, got: {other:?}"
                        ))
                        .into());
                    }
                };
                dict.insert(Value::Str(key_str), from_yaml_value(v)?);
            }
            Ok(Value::Dict(dict))
        }
        serde_yaml::Value::Tagged(tagged) => from_yaml_value(tagged.value),
    }
}

// ─── TOML helpers ─────────────────────────────────────────────────────────────

fn to_toml_value(value: &Value) -> Result<toml::Value, EvalError> {
    match value {
        Value::Nil => {
            Err(EvalErrorKind::Custom("TOML does not support null/nil values".into()).into())
        }
        Value::Pending(_) => {
            Err(EvalErrorKind::Custom("cannot encode pending value as TOML".into()).into())
        }
        Value::Int(v) => Ok(toml::Value::Integer(*v)),
        Value::Float(v) => {
            let f = v.into_inner();
            if f.is_nan() || f.is_infinite() {
                return Err(
                    EvalErrorKind::Custom("invalid float for TOML encoding".into()).into(),
                );
            }
            Ok(toml::Value::Float(f))
        }
        Value::Bool(v) => Ok(toml::Value::Boolean(*v)),
        Value::Str(v) => Ok(toml::Value::String(v.clone())),
        Value::List(values) => {
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                out.push(to_toml_value(v)?);
            }
            Ok(toml::Value::Array(out))
        }
        Value::ExternFn(_) | Value::Fn(_) => {
            Err(EvalErrorKind::Custom("cannot encode function value as TOML".into()).into())
        }
        Value::Exception(_) => {
            Err(EvalErrorKind::Custom("cannot encode exception value as TOML".into()).into())
        }
        Value::Path(pv) => Ok(toml::Value::String(pv.path.clone())),
        Value::Record(record) => {
            let mut table = toml::map::Map::new();
            for (name, v) in record.iter() {
                table.insert(name.to_owned(), to_toml_value(v)?);
            }
            Ok(toml::Value::Table(table))
        }
        Value::Dict(dict) => {
            let mut table = toml::map::Map::new();
            for (key, v) in dict.iter() {
                let key_str = match key {
                    Value::Str(s) => s.clone(),
                    other => other.to_string(),
                };
                table.insert(key_str, to_toml_value(v)?);
            }
            Ok(toml::Value::Table(table))
        }
    }
}

fn from_toml_value(value: toml::Value) -> Result<Value, EvalError> {
    match value {
        toml::Value::Integer(i) => Ok(Value::Int(i)),
        toml::Value::Float(f) => {
            let nn = NotNan::new(f).map_err(|_| -> EvalError {
                EvalErrorKind::Custom("TOML float is NaN".into()).into()
            })?;
            Ok(Value::Float(nn))
        }
        toml::Value::Boolean(b) => Ok(Value::Bool(b)),
        toml::Value::String(s) => Ok(Value::Str(s)),
        toml::Value::Datetime(dt) => Ok(Value::Str(dt.to_string())),
        toml::Value::Array(values) => {
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                out.push(from_toml_value(v)?);
            }
            Ok(Value::List(out))
        }
        toml::Value::Table(table) => {
            let mut dict = Dict::default();
            for (key, v) in table {
                dict.insert(Value::Str(key), from_toml_value(v)?);
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

    #[test]
    fn base64_round_trip() {
        let input = "hello, world!";
        let encoded = base64::engine::general_purpose::STANDARD.encode(input.as_bytes());
        assert_eq!(encoded, "aGVsbG8sIHdvcmxkIQ==");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), input);
    }

    #[test]
    fn base64url_round_trip() {
        let input = "hello";
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input.as_bytes());
        assert_eq!(encoded, "aGVsbG8");
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded.as_bytes())
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), input);
    }

    #[test]
    fn hex_round_trip() {
        let input = "hi";
        let encoded = hex::encode(input.as_bytes());
        assert_eq!(encoded, "6869");
        let decoded = hex::decode(&encoded).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), input);
    }

    #[test]
    fn yaml_round_trip() {
        let mut record = Record::default();
        record.insert("name".into(), Value::Str("test".into()));
        record.insert("count".into(), Value::Int(42));

        let yaml = to_yaml_value(&Value::Record(record)).unwrap();
        let encoded = serde_yaml::to_string(&yaml).unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&encoded).unwrap();
        let value = from_yaml_value(parsed).unwrap();

        let Value::Dict(dict) = value else {
            panic!("expected dict");
        };

        let mut found_name = false;
        let mut found_count = false;
        for (key, val) in dict.iter() {
            if let Value::Str(k) = key {
                if k == "name" {
                    assert!(matches!(val, Value::Str(s) if s == "test"));
                    found_name = true;
                } else if k == "count" {
                    assert!(matches!(val, Value::Int(42)));
                    found_count = true;
                }
            }
        }
        assert!(found_name && found_count);
    }

    #[test]
    fn toml_round_trip() {
        let mut record = Record::default();
        record.insert("name".into(), Value::Str("test".into()));
        record.insert("count".into(), Value::Int(42));

        let toml_val = to_toml_value(&Value::Record(record)).unwrap();
        let encoded = toml::to_string(&toml_val).unwrap();

        let parsed: toml::Value = toml::from_str(&encoded).unwrap();
        let value = from_toml_value(parsed).unwrap();

        let Value::Dict(dict) = value else {
            panic!("expected dict");
        };

        let mut found_name = false;
        let mut found_count = false;
        for (key, val) in dict.iter() {
            if let Value::Str(k) = key {
                if k == "name" {
                    assert!(matches!(val, Value::Str(s) if s == "test"));
                    found_name = true;
                } else if k == "count" {
                    assert!(matches!(val, Value::Int(42)));
                    found_count = true;
                }
            }
        }
        assert!(found_name && found_count);
    }

    #[test]
    fn url_encode_decode_round_trip() {
        let input = "hello world & more=stuff";
        let encoded = urlencoding::encode(input).into_owned();
        assert_eq!(encoded, "hello%20world%20%26%20more%3Dstuff");
        let decoded = urlencoding::decode(&encoded).unwrap().into_owned();
        assert_eq!(decoded, input);
    }
}
