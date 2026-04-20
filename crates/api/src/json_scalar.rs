use juniper::{InputValue, ScalarValue, Value};

#[derive(Clone, Debug)]
#[juniper::graphql_scalar(with = json_scalar_impl, parse_token(String), name = "JSON")]
pub(crate) struct JsonValue(pub(crate) serde_json::Value);

mod json_scalar_impl {
    use super::*;

    pub(super) fn to_output<S: ScalarValue>(value: &JsonValue) -> Value<S> {
        json_to_graphql_value(&value.0)
    }

    pub(super) fn from_input<S: ScalarValue>(value: &InputValue<S>) -> Result<JsonValue, String> {
        let json = input_to_json(value)?;
        // Juniper rejects object/list variable values for custom scalars before
        // `from_input` is called, so clients must JSON-encode complex values as
        // strings. Transparently unwrap them here.
        if let serde_json::Value::String(s) = &json
            && let Ok(parsed @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) =
                serde_json::from_str::<serde_json::Value>(s)
        {
            return Ok(JsonValue(parsed));
        }
        Ok(JsonValue(json))
    }
}

fn json_to_graphql_value<S: ScalarValue>(value: &serde_json::Value) -> Value<S> {
    match value {
        serde_json::Value::Null => Value::null(),
        serde_json::Value::Bool(value) => Value::scalar(*value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                if let Ok(value) = i32::try_from(value) {
                    Value::scalar(value)
                } else {
                    Value::scalar(value as f64)
                }
            } else if let Some(value) = value.as_u64() {
                if let Ok(value) = i32::try_from(value) {
                    Value::scalar(value)
                } else {
                    Value::scalar(value as f64)
                }
            } else if let Some(value) = value.as_f64() {
                Value::scalar(value)
            } else {
                Value::null()
            }
        }
        serde_json::Value::String(value) => Value::scalar(value.clone()),
        serde_json::Value::Array(values) => Value::list(
            values
                .iter()
                .map(json_to_graphql_value::<S>)
                .collect::<Vec<_>>(),
        ),
        serde_json::Value::Object(values) => {
            let mut object = juniper::Object::with_capacity(values.len());
            for (name, value) in values {
                object.add_field(name.to_string(), json_to_graphql_value::<S>(value));
            }
            Value::object(object)
        }
    }
}

fn input_to_json<S: ScalarValue>(value: &InputValue<S>) -> Result<serde_json::Value, String> {
    match value {
        InputValue::Null => Ok(serde_json::Value::Null),
        InputValue::Scalar(scalar) => {
            if let Some(value) = scalar.as_str() {
                Ok(serde_json::Value::String(value.to_string()))
            } else if let Some(value) = scalar.as_bool() {
                Ok(serde_json::Value::Bool(value))
            } else if let Some(value) = scalar.as_int() {
                Ok(serde_json::Value::Number(serde_json::Number::from(value)))
            } else if let Some(value) = scalar.as_float() {
                let Some(value) = serde_json::Number::from_f64(value) else {
                    return Err("JSON cannot represent NaN or infinite floats".to_string());
                };
                Ok(serde_json::Value::Number(value))
            } else {
                Err(format!("Expected JSON scalar, found: {value}"))
            }
        }
        InputValue::Enum(value) | InputValue::Variable(value) => {
            Ok(serde_json::Value::String(value.clone()))
        }
        InputValue::List(values) => {
            let mut array = Vec::with_capacity(values.len());
            for item in values {
                array.push(input_to_json(&item.item)?);
            }
            Ok(serde_json::Value::Array(array))
        }
        InputValue::Object(values) => {
            let mut object = serde_json::Map::with_capacity(values.len());
            for (key, item) in values {
                object.insert(key.item.clone(), input_to_json(&item.item)?);
            }
            Ok(serde_json::Value::Object(object))
        }
    }
}

pub(crate) fn graphql_value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Scalar(scalar) => {
            if let Some(value) = scalar.as_str() {
                serde_json::Value::String(value.to_string())
            } else if let Some(value) = scalar.as_bool() {
                serde_json::Value::Bool(value)
            } else if let Some(value) = scalar.as_int() {
                serde_json::Value::Number(serde_json::Number::from(value))
            } else if let Some(value) = scalar.as_float() {
                serde_json::Number::from_f64(value)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::String(scalar.to_string())
            }
        }
        Value::List(values) => {
            serde_json::Value::Array(values.iter().map(graphql_value_to_json).collect::<Vec<_>>())
        }
        Value::Object(values) => {
            let mut object = serde_json::Map::with_capacity(values.field_count());
            for (name, value) in values.iter() {
                object.insert(name.to_string(), graphql_value_to_json(value));
            }
            serde_json::Value::Object(object)
        }
    }
}

pub(crate) fn serialize_execution_errors(
    errors: &[juniper::ExecutionError<juniper::DefaultScalarValue>],
) -> serde_json::Value {
    serde_json::to_value(errors).unwrap_or_else(|error| {
        serde_json::json!([{
            "message": format!("failed to serialize execution errors: {error}")
        }])
    })
}
