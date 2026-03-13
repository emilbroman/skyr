const ED25519_RESOURCE_TYPE: &str = "Std/Crypto.ED25519PrivateKey";
const ECDSA_RESOURCE_TYPE: &str = "Std/Crypto.ECDSAPrivateKey";
const RSA_RESOURCE_TYPE: &str = "Std/Crypto.RSAPrivateKey";

fn extract_key_outputs(
    outputs: &crate::Record,
) -> Result<crate::Record, crate::EvalError> {
    use crate::ValueAssertions;

    let pem = outputs.get("pem").assert_str_ref()?;
    let public_key_pem = outputs.get("publicKeyPem").assert_str_ref()?;

    let mut out = crate::Record::default();
    out.insert(String::from("pem"), crate::Value::Str(pem.to_owned()));
    out.insert(
        String::from("publicKeyPem"),
        crate::Value::Str(public_key_pem.to_owned()),
    );
    Ok(out)
}

pub fn register_extern(eval: &mut crate::Eval) {
    eval.add_extern_fn(ED25519_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        let config = config_arg.value.assert_record()?;
        let name = config.get("name").assert_str_ref()?;

        let resource_id = crate::ResourceId {
            ty: ED25519_RESOURCE_TYPE.to_owned(),
            id: name.to_owned(),
        };

        let inputs = crate::Record::default();

        let Some(outputs) = eval_ctx.resource(
            ED25519_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            argument_dependencies.insert(resource_id);
            return Ok(crate::TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        let out = extract_key_outputs(&outputs)?;
        argument_dependencies.insert(resource_id);
        Ok(crate::TrackedValue::new(crate::Value::Record(out))
            .with_dependencies(argument_dependencies))
    });

    eval.add_extern_fn(ECDSA_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        let config = config_arg.value.assert_record()?;
        let name = config.get("name").assert_str_ref()?;

        let curve = match config.get("curve") {
            crate::Value::Nil => "P-256",
            other => other.assert_str_ref()?,
        };

        let resource_id = crate::ResourceId {
            ty: ECDSA_RESOURCE_TYPE.to_owned(),
            id: name.to_owned(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("curve"), crate::Value::Str(curve.to_owned()));

        let Some(outputs) = eval_ctx.resource(
            ECDSA_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            argument_dependencies.insert(resource_id);
            return Ok(crate::TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        let out = extract_key_outputs(&outputs)?;
        argument_dependencies.insert(resource_id);
        Ok(crate::TrackedValue::new(crate::Value::Record(out))
            .with_dependencies(argument_dependencies))
    });

    eval.add_extern_fn(RSA_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        let config = config_arg.value.assert_record()?;
        let name = config.get("name").assert_str_ref()?;

        let size = match config.get("size") {
            crate::Value::Nil => 2048,
            other => *other.assert_int_ref()?,
        };

        let resource_id = crate::ResourceId {
            ty: RSA_RESOURCE_TYPE.to_owned(),
            id: name.to_owned(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("size"), crate::Value::Int(size));

        let Some(outputs) = eval_ctx.resource(
            RSA_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            argument_dependencies.insert(resource_id);
            return Ok(crate::TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        let out = extract_key_outputs(&outputs)?;
        argument_dependencies.insert(resource_id);
        Ok(crate::TrackedValue::new(crate::Value::Record(out))
            .with_dependencies(argument_dependencies))
    });
}
