use std::collections::BTreeSet;

use ids::ResourceId;

use crate::{EvalError, EvalErrorKind, TrackedValue, Value, ValueAssertions};

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
    eval.add_extern_fn("Std/Str.length", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        let result = Value::Int(s.chars().count() as i64);
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.toUpper", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        Ok(TrackedValue::new(Value::Str(s.to_uppercase())).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.toLower", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        Ok(TrackedValue::new(Value::Str(s.to_lowercase())).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.trim", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        Ok(TrackedValue::new(Value::Str(s.trim().to_owned())).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.trimStart", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        Ok(TrackedValue::new(Value::Str(s.trim_start().to_owned())).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.trimEnd", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        Ok(TrackedValue::new(Value::Str(s.trim_end().to_owned())).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.split", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let sep = values.remove(1).assert_str()?;
        let s = values.remove(0).assert_str()?;
        if sep.is_empty() {
            return Err(
                EvalErrorKind::Custom("Std/Str.split: separator must not be empty".into()).into(),
            );
        }
        let parts: Vec<Value> = s
            .split(&sep as &str)
            .map(|p| Value::Str(p.to_owned()))
            .collect();
        Ok(TrackedValue::new(Value::List(parts)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.join", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let sep = values.remove(1).assert_str()?;
        let list = match values.remove(0) {
            Value::List(items) => items,
            other => {
                return Err(EvalErrorKind::UnexpectedValue(other).into());
            }
        };
        let mut pieces: Vec<String> = Vec::with_capacity(list.len());
        for item in list {
            pieces.push(item.assert_str()?);
        }
        Ok(TrackedValue::new(Value::Str(pieces.join(&sep as &str))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.contains", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let needle = values.remove(1).assert_str()?;
        let s = values.remove(0).assert_str()?;
        Ok(TrackedValue::new(Value::Bool(s.contains(&needle as &str))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.startsWith", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let prefix = values.remove(1).assert_str()?;
        let s = values.remove(0).assert_str()?;
        Ok(TrackedValue::new(Value::Bool(s.starts_with(&prefix as &str))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.endsWith", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let suffix = values.remove(1).assert_str()?;
        let s = values.remove(0).assert_str()?;
        Ok(TrackedValue::new(Value::Bool(s.ends_with(&suffix as &str))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.replace", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 3)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let to = values.remove(2).assert_str()?;
        let from = values.remove(1).assert_str()?;
        let s = values.remove(0).assert_str()?;
        if from.is_empty() {
            return Err(
                EvalErrorKind::Custom("Std/Str.replace: `from` must not be empty".into()).into(),
            );
        }
        Ok(
            TrackedValue::new(Value::Str(s.replace(&from as &str, &to as &str)))
                .with_dependencies(deps),
        )
    });

    eval.add_extern_fn("Std/Str.slice", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 3)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let end_val = values.remove(2);
        let start = values.remove(1).assert_int()?;
        let s = values.remove(0).assert_str()?;

        if start < 0 {
            return Err(EvalErrorKind::Custom(
                "Std/Str.slice: indices must be non-negative".into(),
            )
            .into());
        }

        let end_opt: Option<i64> = match end_val {
            Value::Nil => None,
            other => {
                let e = other.assert_int()?;
                if e < 0 {
                    return Err(EvalErrorKind::Custom(
                        "Std/Str.slice: indices must be non-negative".into(),
                    )
                    .into());
                }
                Some(e)
            }
        };

        let len = s.chars().count() as i64;
        let clamped_start = start.min(len);
        let end = match end_opt {
            None => len,
            Some(e) => e.min(len),
        };

        if end <= clamped_start {
            return Ok(TrackedValue::new(Value::Str(String::new())).with_dependencies(deps));
        }

        let result: String = s
            .chars()
            .skip(clamped_start as usize)
            .take((end - clamped_start) as usize)
            .collect();
        Ok(TrackedValue::new(Value::Str(result)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.indexOf", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let needle = values.remove(1).assert_str()?;
        let s = values.remove(0).assert_str()?;

        if needle.is_empty() {
            return Ok(TrackedValue::new(Value::Int(0)).with_dependencies(deps));
        }

        let result = match s.match_indices(&needle as &str).next() {
            Some((byte_idx, _)) => {
                let scalar_idx = s[..byte_idx].chars().count() as i64;
                Value::Int(scalar_idx)
            }
            None => Value::Nil,
        };
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.repeat", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let n = values.remove(1).assert_int()?;
        let s = values.remove(0).assert_str()?;
        if n < 0 {
            return Err(
                EvalErrorKind::Custom("Std/Str.repeat: times must be non-negative".into()).into(),
            );
        }
        Ok(TrackedValue::new(Value::Str(s.repeat(n as usize))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.padStart", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 3)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let fill_val = values.remove(2);
        let width = values.remove(1).assert_int()?;
        let s = values.remove(0).assert_str()?;

        if width < 0 {
            return Err(EvalErrorKind::Custom(
                "Std/Str.padStart: width must be non-negative".into(),
            )
            .into());
        }

        let fill_str = match fill_val {
            Value::Nil => " ".to_owned(),
            other => other.assert_str()?,
        };
        if fill_str.is_empty() {
            return Err(
                EvalErrorKind::Custom("Std/Str.padStart: fill must not be empty".into()).into(),
            );
        }

        let len = s.chars().count() as i64;
        if len >= width {
            return Ok(TrackedValue::new(Value::Str(s)).with_dependencies(deps));
        }

        let missing = (width - len) as usize;
        let padding: String = fill_str.chars().cycle().take(missing).collect();
        Ok(TrackedValue::new(Value::Str(format!("{padding}{s}"))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Str.padEnd", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 3)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let fill_val = values.remove(2);
        let width = values.remove(1).assert_int()?;
        let s = values.remove(0).assert_str()?;

        if width < 0 {
            return Err(
                EvalErrorKind::Custom("Std/Str.padEnd: width must be non-negative".into()).into(),
            );
        }

        let fill_str = match fill_val {
            Value::Nil => " ".to_owned(),
            other => other.assert_str()?,
        };
        if fill_str.is_empty() {
            return Err(
                EvalErrorKind::Custom("Std/Str.padEnd: fill must not be empty".into()).into(),
            );
        }

        let len = s.chars().count() as i64;
        if len >= width {
            return Ok(TrackedValue::new(Value::Str(s)).with_dependencies(deps));
        }

        let missing = (width - len) as usize;
        let padding: String = fill_str.chars().cycle().take(missing).collect();
        Ok(TrackedValue::new(Value::Str(format!("{s}{padding}"))).with_dependencies(deps))
    });
}
