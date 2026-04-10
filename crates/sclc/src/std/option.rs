pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn("Std/Option.uncheckedUnwrap", |args, _ctx| {
        let mut args = args.into_iter();
        Ok(args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil)))
    });
}
