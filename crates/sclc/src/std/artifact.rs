const FILE_RESOURCE_TYPE: &str = "Std/Artifact.File";

pub fn register_extern(eval: &mut crate::Eval) {
    eval.add_extern_fn(FILE_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        let config = config_arg.value.assert_record()?;

        let name = config.get("name").assert_str_ref()?;
        let media_type = match config.get("mediaType") {
            crate::Value::Nil => None,
            other => Some(other.assert_str_ref()?),
        };
        let contents = config.get("contents").assert_str_ref()?;
        let namespace = eval_ctx.namespace();

        let resource_id = crate::ResourceId {
            ty: FILE_RESOURCE_TYPE.to_owned(),
            id: name.to_owned(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("name"), crate::Value::Str(name.to_owned()));
        inputs.insert(
            String::from("mediaType"),
            media_type
                .map(|value| crate::Value::Str(value.to_owned()))
                .unwrap_or(crate::Value::Nil),
        );
        inputs.insert(
            String::from("namespace"),
            crate::Value::Str(namespace.to_owned()),
        );
        inputs.insert(
            String::from("contents"),
            crate::Value::Str(contents.to_owned()),
        );

        let Some(outputs) = eval_ctx.resource(
            FILE_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            argument_dependencies.insert(resource_id);
            return Ok(crate::TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        let mut out = crate::Record::default();
        let url = outputs.get("url").assert_str_ref()?;
        out.insert(String::from("url"), crate::Value::Str(url.to_owned()));
        argument_dependencies.insert(resource_id);

        Ok(crate::TrackedValue::new(crate::Value::Record(out))
            .with_dependencies(argument_dependencies))
    })
}
