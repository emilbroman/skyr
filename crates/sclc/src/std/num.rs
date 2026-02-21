pub fn register_extern(eval: &mut crate::Eval) {
    eval.add_extern_fn("Std/Num.toHex", |args, _ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));

        first.try_map(|value| {
            value
                .assert_int()
                .map(|i| crate::Value::Str(format!("{i:x}")))
        })
    });
}
