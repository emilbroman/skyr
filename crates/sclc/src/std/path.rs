use std::collections::BTreeSet;

use ids::{ObjId, ResourceId};

use crate::eval::PathLookupError;
use crate::{EvalCtx, EvalError, EvalErrorKind, PathValue, Record, TrackedValue, Value};

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

fn null_hash() -> ObjId {
    ObjId::null()
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

/// Outcome of looking up a synthesized path against the calling
/// package, mirrored as a `_PathLookup` record on the SCL side.
enum LookupOutcome {
    /// Path was found (or no packages were registered, in which case
    /// the hash is null but the value is still considered "found" so
    /// that compile-time eval and tests without fixture files keep
    /// working). Carries the resolved path value.
    Found(PathValue),
    /// Path was not found in the calling package; cannot carry a
    /// content hash. The SCL wrapper raises `Path.NotFound` with the
    /// path string.
    Missing(String),
    /// `parent` of the root path; the SCL wrapper returns `nil`.
    Root,
    /// `fromStr` was given a string that is not a valid absolute path;
    /// the SCL wrapper returns `nil`.
    Invalid,
}

fn lookup_path(ctx: &EvalCtx, path: String) -> LookupOutcome {
    let hash = match ctx.current_caller_package() {
        Some(caller_pkg) => match ctx.resolve_path_hash(&path, &caller_pkg) {
            Ok(Some(h)) => h,
            Ok(None) => null_hash(),
            Err(PathLookupError::NotFound) => return LookupOutcome::Missing(path),
        },
        None => null_hash(),
    };
    LookupOutcome::Found(PathValue { path, hash })
}

fn lookup_to_record(outcome: LookupOutcome) -> Record {
    let mut record = Record::default();
    let (tag, path, missing) = match outcome {
        LookupOutcome::Found(pv) => ("found", Value::Path(pv), String::new()),
        LookupOutcome::Missing(path) => (
            "missing",
            Value::Path(PathValue {
                path: "/".into(),
                hash: null_hash(),
            }),
            path,
        ),
        LookupOutcome::Root => (
            "root",
            Value::Path(PathValue {
                path: "/".into(),
                hash: null_hash(),
            }),
            String::new(),
        ),
        LookupOutcome::Invalid => (
            "invalid",
            Value::Path(PathValue {
                path: "/".into(),
                hash: null_hash(),
            }),
            String::new(),
        ),
    };
    record.insert("tag".into(), Value::Str(tag.into()));
    record.insert("path".into(), path);
    record.insert("missing".into(), Value::Str(missing));
    record
}

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/Path.join", |args, ctx| {
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
                .into());
            }
        };
        let path_str = segments_to_path_str(&segs);
        let record = lookup_to_record(lookup_path(ctx, path_str));
        Ok(TrackedValue::new(Value::Record(record)).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Path.parent", |args, ctx| {
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
        let outcome = if segs.is_empty() {
            LookupOutcome::Root
        } else {
            let parent_segs = &segs[..segs.len() - 1];
            lookup_path(ctx, segments_to_path_str(parent_segs))
        };
        let record = lookup_to_record(outcome);
        Ok(TrackedValue::new(Value::Record(record)).with_dependencies(deps))
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

    eval.add_extern_fn("Std/Path.fromStr", |args, ctx| {
        let (mut values, deps) = match collect_args(args, 1)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let s = match values.remove(0) {
            Value::Str(s) => s,
            other => return Err(EvalErrorKind::UnexpectedValue(other).into()),
        };
        let outcome = if s.is_empty() || !s.starts_with('/') {
            LookupOutcome::Invalid
        } else {
            let raw = split_path_str(&s);
            match normalize_segments(raw) {
                Some(segs) => lookup_path(ctx, segments_to_path_str(&segs)),
                None => LookupOutcome::Invalid,
            }
        };
        let record = lookup_to_record(outcome);
        Ok(TrackedValue::new(Value::Record(record)).with_dependencies(deps))
    });
}
