const INT_RESOURCE_TYPE: &str = "Std/Random.Int";

pub fn register_extern(eval: &mut crate::Eval) {
    eval.add_extern_fn(INT_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let name = args.next().assert_str()?;
        let arg1 = args.next().assert_int()?;
        let arg2 = args.next().assert_int()?;
        let min = arg1.min(arg2);
        let max = arg1.max(arg2);

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("min"), crate::Value::Int(min));
        inputs.insert(String::from("max"), crate::Value::Int(max));

        let Some(outputs) = eval_ctx.resource(INT_RESOURCE_TYPE, name, &inputs)? else {
            return Ok(crate::Value::Pending(crate::PendingValue));
        };

        let mut merged = inputs;
        for (name, value) in outputs.iter() {
            merged.insert(name.to_owned(), value.clone());
        }
        Ok(crate::Value::Record(merged))
    })
}
