pub fn register_extern(eval: &mut crate::Eval) {
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
}
