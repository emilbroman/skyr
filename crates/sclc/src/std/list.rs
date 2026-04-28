use std::collections::BTreeSet;

use ids::ResourceId;

use crate::{EvalError, EvalErrorKind, Record, TrackedValue, Value, ValueAssertions};

type CollectResult = Result<Result<(Vec<Value>, BTreeSet<ResourceId>), TrackedValue>, EvalError>;

/// Collect up to N args, merge dependencies, short-circuit on pending.
/// Returns Ok(Ok((values, deps))) or Ok(Err(pending)) or Err(eval_error).
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

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/List.range", |args, _ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));

        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }

        first.try_map(|value| {
            let n = value.assert_int()?;
            if n < 0 {
                return Err(crate::EvalErrorKind::Custom(format!(
                    "List.range: expected non-negative integer, got {n}"
                ))
                .into());
            }
            Ok(crate::Value::List((0..n).map(crate::Value::Int).collect()))
        })
    });

    eval.add_extern_fn("Std/List.length", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        Ok(TrackedValue::new(Value::Int(items.len() as i64)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/List.reverse", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let mut items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        items.reverse();
        Ok(TrackedValue::new(Value::List(items)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/List.first", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let result = items.into_iter().next().unwrap_or(Value::Nil);
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/List.last", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let result = items.into_iter().next_back().unwrap_or(Value::Nil);
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/List.slice", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 3)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let end_val = values.remove(2);
        let start = values.remove(1).assert_int()?;
        let items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };

        if start < 0 {
            return Err(EvalErrorKind::Custom(
                "Std/List.slice: indices must be non-negative".into(),
            )
            .into());
        }

        let end_opt: Option<i64> = match end_val {
            Value::Nil => None,
            other => {
                let e = other.assert_int()?;
                if e < 0 {
                    return Err(EvalErrorKind::Custom(
                        "Std/List.slice: indices must be non-negative".into(),
                    )
                    .into());
                }
                Some(e)
            }
        };

        let len = items.len() as i64;
        let clamped_start = start.min(len);
        let end = match end_opt {
            None => len,
            Some(e) => e.min(len),
        };

        if end <= clamped_start {
            return Ok(TrackedValue::new(Value::List(vec![])).with_dependencies(deps));
        }

        let result: Vec<Value> = items
            .into_iter()
            .skip(clamped_start as usize)
            .take((end - clamped_start) as usize)
            .collect();
        Ok(TrackedValue::new(Value::List(result)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/List.contains", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let needle = values.remove(1);
        let items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let found = items.iter().any(|item| item == &needle);
        Ok(TrackedValue::new(Value::Bool(found)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/List.indexOf", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let needle = values.remove(1);
        let items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let result = items
            .iter()
            .position(|item| item == &needle)
            .map(|i| Value::Int(i as i64))
            .unwrap_or(Value::Nil);
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/List.zip", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let b_items = match values.remove(1) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let a_items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let result: Vec<Value> = a_items
            .into_iter()
            .zip(b_items)
            .map(|(a, b)| {
                let mut record = Record::default();
                record.insert("a".into(), a);
                record.insert("b".into(), b);
                Value::Record(record)
            })
            .collect();
        Ok(TrackedValue::new(Value::List(result)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/List.distinct", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let items = match values.remove(0) {
            Value::List(items) => items,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let mut seen: Vec<Value> = Vec::new();
        let mut result: Vec<Value> = Vec::new();
        for item in items {
            if !seen.contains(&item) {
                seen.push(item.clone());
                result.push(item);
            }
        }
        Ok(TrackedValue::new(Value::List(result)).with_dependencies(deps))
    });
}
