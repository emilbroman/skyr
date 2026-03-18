const INT_RESOURCE_TYPE: &str = "Std/Random.Int";

pub fn register_extern(eval: &mut crate::Eval) {
    eval.add_extern_fn(INT_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        let config = config_arg.value.assert_record()?;

        let name = config.get("name").assert_str_ref()?;
        let arg1 = *config.get("min").assert_int_ref()?;
        let arg2 = *config.get("max").assert_int_ref()?;
        let min = arg1.min(arg2);
        let max = arg1.max(arg2);
        let resource_id = ids::ResourceId {
            typ: INT_RESOURCE_TYPE.to_string(),
            name: name.to_owned(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("min"), crate::Value::Int(min));
        inputs.insert(String::from("max"), crate::Value::Int(max));

        let Some(outputs) = eval_ctx.resource(
            INT_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            argument_dependencies.insert(resource_id);
            return Ok(crate::TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        let mut merged = inputs;
        for (name, value) in outputs.iter() {
            merged.insert(name.to_owned(), value.clone());
        }
        argument_dependencies.insert(resource_id);
        Ok(crate::TrackedValue::new(crate::Value::Record(merged))
            .with_dependencies(argument_dependencies))
    })
}
