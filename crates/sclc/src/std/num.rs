pub fn register_extern(eval: &mut crate::Eval) {
    eval.add_extern_fn("Std/Num.toHex", |args, _ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let i = args.next().assert_int()?;
        Ok(crate::Value::Str(format!("{i:x}")))
    });
}
