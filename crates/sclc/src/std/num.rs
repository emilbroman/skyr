pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/Num.toHex", |args, _ctx| {
        use crate::ValueAssertions;

        let first = match super::extract_arg(args) {
            super::ExternArg::Ready(arg) => arg,
            super::ExternArg::Pending(p) => return Ok(p),
        };

        first.try_map(|value| {
            value
                .assert_int()
                .map(|i| crate::Value::Str(format!("{i:x}")))
        })
    });
}
