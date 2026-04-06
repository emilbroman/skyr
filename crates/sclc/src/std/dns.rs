const A_RECORD_RESOURCE_TYPE: &str = "Std/DNS.ARecord";

pub fn register_extern(eval: &mut crate::Eval<'_>) {
    eval.add_extern_fn(A_RECORD_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let argument_dependencies = config_arg.dependencies.clone();

        if config_arg.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(argument_dependencies));
        }

        let config = config_arg.value.assert_record()?;

        let name = config.get("name").assert_str_ref()?;
        let ttl = config.get("ttl").assert_record_ref()?;
        let addresses = match config.get("addresses") {
            crate::Value::List(list) => list.clone(),
            _ => vec![],
        };

        let resource_id = ids::ResourceId {
            typ: A_RECORD_RESOURCE_TYPE.to_string(),
            name: name.to_owned(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("name"), crate::Value::Str(name.to_owned()));
        inputs.insert(
            String::from("ttl"),
            crate::Value::Record(ttl.clone()),
        );
        inputs.insert(String::from("addresses"), crate::Value::List(addresses));

        let Some(outputs) = eval_ctx.resource(
            A_RECORD_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let mut merged = inputs;
        for (k, v) in outputs.iter() {
            merged.insert(k.to_owned(), v.clone());
        }
        Ok(crate::TrackedValue::new(crate::Value::Record(merged)).with_dependency(resource_id))
    })
}
