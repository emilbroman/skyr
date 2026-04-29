use std::collections::BTreeSet;

use ids::ResourceId;

use crate::{EvalError, EvalErrorKind, PathValue, TrackedValue, Value};

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

fn null_hash() -> gix_hash::ObjectId {
    gix_hash::ObjectId::null(gix_hash::Kind::Sha1)
}

/// Normalize segments by handling "." and ".." entries.
/// Returns None if ".." would escape the root.
fn normalize_segments(raw: impl IntoIterator<Item = String>) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for s in raw {
        if s.is_empty() || s == "." {
            continue;
        }
        if s == ".." {
            out.pop()?;
            continue;
        }
        out.push(s);
    }
    Some(out)
}

fn segments_to_path_str(segs: &[String]) -> String {
    if segs.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segs.join("/"))
    }
}

fn split_path_str(s: &str) -> Vec<String> {
    s.split('/').map(|p| p.to_string()).collect()
}

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/Path.join", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 2)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let segment = match values.remove(1) {
            Value::Str(s) => s,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let base_path = match values.remove(0) {
            Value::Path(pv) => pv.path,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let mut raw: Vec<String> = split_path_str(&base_path);
        raw.extend(split_path_str(&segment));
        let segs = match normalize_segments(raw) {
            Some(s) => s,
            None => {
                return Err(EvalErrorKind::Custom(
                    "Std/Path.join: relative segment escapes root".into(),
                )
                .into())
            }
        };
        let path_str = segments_to_path_str(&segs);
        Ok(TrackedValue::new(Value::Path(PathValue {
            path: path_str,
            hash: null_hash(),
        }))
        .with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.parent", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let path_str = match values.remove(0) {
            Value::Path(pv) => pv.path,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let segs: Vec<String> = path_str
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        if segs.is_empty() {
            return Ok(TrackedValue::new(Value::Nil).with_dependencies(deps));
        }
        let parent_segs = &segs[..segs.len() - 1];
        let parent_str = segments_to_path_str(parent_segs);
        Ok(TrackedValue::new(Value::Path(PathValue {
            path: parent_str,
            hash: null_hash(),
        }))
        .with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.basename", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let path_str = match values.remove(0) {
            Value::Path(pv) => pv.path,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let segs: Vec<&str> = path_str.split('/').filter(|s| !s.is_empty()).collect();
        let base = segs.last().copied().unwrap_or("").to_string();
        Ok(TrackedValue::new(Value::Str(base)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.extname", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let path_str = match values.remove(0) {
            Value::Path(pv) => pv.path,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let segs: Vec<&str> = path_str.split('/').filter(|s| !s.is_empty()).collect();
        let base = segs.last().copied().unwrap_or("");
        let result = match base.rfind('.') {
            Some(pos) if pos > 0 => Value::Str(base[pos..].to_string()),
            _ => Value::Nil,
        };
        Ok(TrackedValue::new(result).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.stem", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let path_str = match values.remove(0) {
            Value::Path(pv) => pv.path,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let segs: Vec<&str> = path_str.split('/').filter(|s| !s.is_empty()).collect();
        let base = segs.last().copied().unwrap_or("");
        let stem = match base.rfind('.') {
            Some(pos) if pos > 0 => &base[..pos],
            _ => base,
        };
        Ok(TrackedValue::new(Value::Str(stem.to_string())).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.segments", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let path_str = match values.remove(0) {
            Value::Path(pv) => pv.path,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let segs: Vec<Value> = path_str
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| Value::Str(s.to_string()))
            .collect();
        Ok(TrackedValue::new(Value::List(segs)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.isRoot", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let path_str = match values.remove(0) {
            Value::Path(pv) => pv.path,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let is_root = path_str.split('/').filter(|s| !s.is_empty()).count() == 0;
        Ok(TrackedValue::new(Value::Bool(is_root)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.toStr", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let path_str = match values.remove(0) {
            Value::Path(pv) => pv.path,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        Ok(TrackedValue::new(Value::Str(path_str)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.fromStr", |args, _ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = match values.remove(0) {
            Value::Str(s) => s,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        if s.is_empty() || !s.starts_with('/') {
            return Ok(TrackedValue::new(Value::Nil).with_dependencies(deps));
        }
        let raw = split_path_str(&s);
        let segs = match normalize_segments(raw) {
            Some(s) => s,
            None => return Ok(TrackedValue::new(Value::Nil).with_dependencies(deps)),
        };
        let path_str = segments_to_path_str(&segs);
        Ok(TrackedValue::new(Value::Path(PathValue {
            path: path_str,
            hash: null_hash(),
        }))
        .with_dependencies(deps))
    });
}
