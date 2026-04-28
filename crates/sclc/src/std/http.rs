const GET_RESOURCE_TYPE: &str = "Std/HTTP.Get";

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn(GET_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, argument_dependencies) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };

        let url = config.get("url").assert_str_ref()?;

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("url"), crate::Value::Str(url.to_owned()));

        let resource_id = ids::ResourceId {
            typ: GET_RESOURCE_TYPE.to_owned(),
            name: url.to_owned(),
        };

        let Some(outputs) = eval_ctx.resource(
            GET_RESOURCE_TYPE,
            url,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let mut merged = inputs;
        for (name, value) in outputs.iter() {
            merged.insert(name.to_owned(), value.clone());
        }
        Ok(crate::TrackedValue::new(crate::Value::Record(merged)).with_dependency(resource_id))
    })
}
