pub fn register_extern<S: crate::SourceRepo>(eval: &mut crate::Eval<'_, S>) {
    eval.add_extern_fn("Std/Option.uncheckedUnwrap", |args, _ctx| {
        let mut args = args.into_iter();
        Ok(args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil)))
    });
}
