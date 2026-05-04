use std::collections::BTreeSet;

use ids::ResourceId;
use ordered_float::NotNan;

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
    eval.add_extern_fn("Std/Float.fromStr", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = values.remove(0).assert_str()?;
        let result = s
            .parse::<f64>()
            .ok()
            .and_then(|f| if f.is_finite() { NotNan::new(f).ok() } else { None })
            .map_or(Value::Nil, Value::Float);
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.toStr", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let nn = values.remove(0).assert_float()?;
        let f: f64 = nn.into_inner();
        Ok(TrackedValue::new(Value::Str(format!("{f}"))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.abs", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let nn = values.remove(0).assert_float()?;
        let result = NotNan::new(nn.into_inner().abs()).expect("abs of NotNan is not NaN");
        Ok(TrackedValue::new(Value::Float(result)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.min", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let b = values.remove(1).assert_float()?;
        let a = values.remove(0).assert_float()?;
        Ok(TrackedValue::new(Value::Float(a.min(b))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.max", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let b = values.remove(1).assert_float()?;
        let a = values.remove(0).assert_float()?;
        Ok(TrackedValue::new(Value::Float(a.max(b))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.clamp", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 3)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let hi = values.remove(2).assert_float()?;
        let lo = values.remove(1).assert_float()?;
        let value = values.remove(0).assert_float()?;
        if lo > hi {
            return Err(
                EvalErrorKind::Custom("Std/Float.clamp: lo must be <= hi".into()).into(),
            );
        }
        Ok(TrackedValue::new(Value::Float(value.clamp(lo, hi))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.floor", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let nn = values.remove(0).assert_float()?;
        let f: f64 = nn.into_inner();
        let result = f.floor();
        if !result.is_finite() || result < (i64::MIN as f64) || result >= (i64::MAX as f64) {
            return Err(
                EvalErrorKind::Custom("Std/Float.floor: out of i64 range".into()).into(),
            );
        }
        Ok(TrackedValue::new(Value::Int(result as i64)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.ceil", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let nn = values.remove(0).assert_float()?;
        let f: f64 = nn.into_inner();
        let result = f.ceil();
        if !result.is_finite() || result < (i64::MIN as f64) || result >= (i64::MAX as f64) {
            return Err(
                EvalErrorKind::Custom("Std/Float.ceil: out of i64 range".into()).into(),
            );
        }
        Ok(TrackedValue::new(Value::Int(result as i64)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.round", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let nn = values.remove(0).assert_float()?;
        let f: f64 = nn.into_inner();
        let result = f.round();
        if !result.is_finite() || result < (i64::MIN as f64) || result >= (i64::MAX as f64) {
            return Err(
                EvalErrorKind::Custom("Std/Float.round: out of i64 range".into()).into(),
            );
        }
        Ok(TrackedValue::new(Value::Int(result as i64)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Float.pow", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let b = values.remove(1).assert_float()?;
        let a = values.remove(0).assert_float()?;
        let p = a.into_inner().powf(b.into_inner());
        if !p.is_finite() {
            return Err(
                EvalErrorKind::Custom("Std/Float.pow: result is not finite".into()).into(),
            );
        }
        let result = NotNan::new(p).unwrap();
        Ok(TrackedValue::new(Value::Float(result)).with_dependencies(deps))
    });
}
