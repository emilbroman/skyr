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
    eval.add_extern_fn("Std/Num.fromStr", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        let result = s.parse::<i64>().map_or(Value::Nil, Value::Int);
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.fromHex", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        let stripped = s
            .strip_prefix("0x")
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(&s);
        let result = if stripped.is_empty() {
            Value::Nil
        } else {
            i64::from_str_radix(stripped, 16).map_or(Value::Nil, Value::Int)
        };
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.toStr", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let i = values.remove(0).assert_int()?;
        Ok(TrackedValue::new(Value::Str(format!("{i}"))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.abs", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let i = values.remove(0).assert_int()?;
        let result = i
            .checked_abs()
            .ok_or_else(|| EvalError::from(EvalErrorKind::Custom("Std/Num.abs: overflow".into())))?;
        Ok(TrackedValue::new(Value::Int(result)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.min", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let b = values.remove(1).assert_int()?;
        let a = values.remove(0).assert_int()?;
        Ok(TrackedValue::new(Value::Int(a.min(b))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.max", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let b = values.remove(1).assert_int()?;
        let a = values.remove(0).assert_int()?;
        Ok(TrackedValue::new(Value::Int(a.max(b))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.clamp", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 3)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let hi = values.remove(2).assert_int()?;
        let lo = values.remove(1).assert_int()?;
        let value = values.remove(0).assert_int()?;
        if lo > hi {
            return Err(
                EvalErrorKind::Custom("Std/Num.clamp: lo must be <= hi".into()).into(),
            );
        }
        Ok(TrackedValue::new(Value::Int(value.clamp(lo, hi))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.toFloat", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let i = values.remove(0).assert_int()?;
        let nn = ordered_float::NotNan::new(i as f64).expect("i64 cast cannot be NaN");
        Ok(TrackedValue::new(Value::Float(nn)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.pow", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let exp = values.remove(1).assert_int()?;
        let base = values.remove(0).assert_int()?;
        if exp < 0 {
            return Err(
                EvalErrorKind::Custom("Std/Num.pow: exponent must be non-negative".into()).into(),
            );
        }
        let result = u32::try_from(exp)
            .ok()
            .and_then(|e| base.checked_pow(e))
            .ok_or_else(|| EvalError::from(EvalErrorKind::Custom("Std/Num.pow: overflow".into())))?;
        Ok(TrackedValue::new(Value::Int(result)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Num.toHex", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let i = values.remove(0).assert_int()?;
        Ok(TrackedValue::new(Value::Str(format!("{i:x}"))).with_dependencies(deps))
    });
}
