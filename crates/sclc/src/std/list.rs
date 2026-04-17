pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/List.range", |args, _ctx| {
        use crate::ValueAssertions;

        let first = match super::extract_arg(args) {
            super::ExternArg::Ready(arg) => arg,
            super::ExternArg::Pending(p) => return Ok(p),
        };

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
