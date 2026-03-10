pub fn register_extern(eval: &mut crate::Eval) {
    eval.add_extern_fn("Std/List.range", |args, _ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));

        first.try_map(|value| {
            value
                .assert_int()
                .map(|n| crate::Value::List((0..n).map(crate::Value::Int).collect()))
        })
    });
}
