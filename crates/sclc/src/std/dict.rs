use std::collections::BTreeSet;

use ids::ResourceId;

use crate::{Dict, EvalError, EvalErrorKind, Record, TrackedValue, Value, ValueAssertions};

type CollectResult = Result<Result<(Vec<Value>, BTreeSet<ResourceId>), TrackedValue>, EvalError>;

fn collect_args(args: Vec<TrackedValue>, n: usize) -> CollectResult {
    let mut deps: BTreeSet<ResourceId> = BTreeSet::new();
    let mut values: Vec<Value> = Vec::with_capacity(n);
    let mut iter = args.into_iter();
    for _ in 0..n {
        let arg = iter.next().unwrap_or_else(|| TrackedValue::new(Value::Nil));
        deps.extend(arg.dependencies);
        values.push(arg.value);
    }
    if values.iter().any(Value::has_pending) {
        return Ok(Err(TrackedValue::pending().with_dependencies(deps)));
    }
    Ok(Ok((values, deps)))
}

fn dict_insert_or_replace(dict: &Dict, new_key: Value, new_value: Value) -> Dict {
    let mut new_dict = Dict::default();
    let mut replaced = false;
    for (k, v) in dict.iter() {
        if k == &new_key {
            new_dict.insert(new_key.clone(), new_value.clone());
            replaced = true;
        } else {
            new_dict.insert(k.clone(), v.clone());
        }
    }
    if !replaced {
        new_dict.insert(new_key, new_value);
    }
    new_dict
}

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/Dict.size", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let dict = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        Ok(TrackedValue::new(Value::Int(dict.iter().count() as i64)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.keys", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let dict = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let keys: Vec<Value> = dict.iter().map(|(k, _)| k.clone()).collect();
        Ok(TrackedValue::new(Value::List(keys)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.values", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let dict = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let vals: Vec<Value> = dict.iter().map(|(_, v)| v.clone()).collect();
        Ok(TrackedValue::new(Value::List(vals)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.entries", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let dict = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let entries: Vec<Value> = dict
            .iter()
            .map(|(k, v)| {
                let mut record = Record::default();
                record.insert("key".into(), k.clone());
                record.insert("value".into(), v.clone());
                Value::Record(record)
            })
            .collect();
        Ok(TrackedValue::new(Value::List(entries)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.has", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let key = values.remove(1);
        let dict = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let found = dict.iter().any(|(k, _)| k == &key);
        Ok(TrackedValue::new(Value::Bool(found)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.get", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let key = values.remove(1);
        let dict = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let result = dict
            .iter()
            .find(|(k, _)| *k == &key)
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Nil);
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.insert", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 3)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let new_value = values.remove(2);
        let new_key = values.remove(1);
        let dict = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let new_dict = dict_insert_or_replace(&dict, new_key, new_value);
        Ok(TrackedValue::new(Value::Dict(new_dict)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.remove", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let key = values.remove(1);
        let dict = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let mut new_dict = Dict::default();
        for (k, v) in dict.iter() {
            if k != &key {
                new_dict.insert(k.clone(), v.clone());
            }
        }
        Ok(TrackedValue::new(Value::Dict(new_dict)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.merge", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let right = match values.remove(1) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let left = match values.remove(0) {
            Value::Dict(d) => d,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let mut buf: Vec<(Value, Value)> = left.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (k, v) in right.iter() {
            if let Some(pos) = buf.iter().position(|(ek, _)| ek == k) {
                buf[pos] = (k.clone(), v.clone());
            } else {
                buf.push((k.clone(), v.clone()));
            }
        }
        let mut new_dict = Dict::default();
        for (k, v) in buf {
            new_dict.insert(k, v);
        }
        Ok(TrackedValue::new(Value::Dict(new_dict)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Dict.fromList", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let mut result = Dict::default();
        for item in items {
            let entry_record = item.assert_record()?;
            let key = entry_record.get("key").clone();
            let value = entry_record.get("value").clone();
            result = dict_insert_or_replace(&result, key, value);
        }
        Ok(TrackedValue::new(Value::Dict(result)).with_dependencies(deps))
    });
}
