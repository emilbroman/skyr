const GET_RESOURCE_TYPE: &str = "Std/HTTP.Get";

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn(GET_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, argument_dependencies) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
let region = super::extract_region_field(&config)?;

        let url = config.get("url").assert_str_ref()?;
        let headers = match config.get("headers") {
            crate::Value::Nil => crate::Value::Dict(crate::Dict::default()),
            other => other.clone(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("url"), crate::Value::Str(url.to_owned()));
        inputs.insert(String::from("headers"), headers);

        let resource_id = ids::ResourceId {
            region: region
                .clone()
                .unwrap_or_else(|| eval_ctx.region().clone()),
            typ: GET_RESOURCE_TYPE.to_owned(),
            name: url.to_owned(),
        };

        let Some(outputs) = eval_ctx.resource(
            region.clone(),
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
