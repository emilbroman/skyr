const ED25519_RESOURCE_TYPE: &str = "Std/Crypto.ED25519PrivateKey";
const ECDSA_RESOURCE_TYPE: &str = "Std/Crypto.ECDSAPrivateKey";
const RSA_RESOURCE_TYPE: &str = "Std/Crypto.RSAPrivateKey";
const CSR_RESOURCE_TYPE: &str = "Std/Crypto.CertificationRequest";
const CERT_SIG_RESOURCE_TYPE: &str = "Std/Crypto.CertificateSignature";

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

        let resource_id = ids::ResourceId {
            typ: ED25519_RESOURCE_TYPE.to_owned(),
            name: name.to_owned(),
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

        let resource_id = ids::ResourceId {
            typ: ECDSA_RESOURCE_TYPE.to_owned(),
            name: name.to_owned(),
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

    eval.add_extern_fn(CSR_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;
        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        let config = config_arg.value.assert_record()?;
        let private_key_pem = config.get("privateKeyPem").assert_str_ref()?;
        let subject = config.get("subject").assert_record_ref()?;

        let common_name = subject.get("commonName").assert_str_ref()?;

        let mut subject_record = crate::Record::default();
        subject_record.insert(
            String::from("commonName"),
            crate::Value::Str(common_name.to_owned()),
        );
        for field in [
            "organization",
            "organizationalUnit",
            "country",
            "state",
            "locality",
        ] {
            subject_record.insert(String::from(field), subject.get(field).clone());
        }

        let mut inputs = crate::Record::default();
        inputs.insert(
            String::from("privateKeyPem"),
            crate::Value::Str(private_key_pem.to_owned()),
        );
        inputs.insert(
            String::from("subject"),
            crate::Value::Record(subject_record),
        );
        inputs.insert(
            String::from("subjectAlternativeNames"),
            config.get("subjectAlternativeNames").clone(),
        );
        inputs.insert(
            String::from("keyUsage"),
            config.get("keyUsage").clone(),
        );
        inputs.insert(
            String::from("extendedKeyUsage"),
            config.get("extendedKeyUsage").clone(),
        );

        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&format!("{:?}", inputs), &mut hasher);
        let resource_name = format!("{:x}", std::hash::Hasher::finish(&hasher));

        let resource_id = ids::ResourceId {
            typ: CSR_RESOURCE_TYPE.to_owned(),
            name: resource_name.clone(),
        };

        let Some(outputs) = eval_ctx.resource(
            CSR_RESOURCE_TYPE,
            &resource_name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            argument_dependencies.insert(resource_id);
            return Ok(crate::TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        let pem = outputs.get("pem").assert_str_ref()?;
        let mut out = crate::Record::default();
        out.insert(String::from("pem"), crate::Value::Str(pem.to_owned()));

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

        let resource_id = ids::ResourceId {
            typ: RSA_RESOURCE_TYPE.to_owned(),
            name: name.to_owned(),
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

    eval.add_extern_fn(CERT_SIG_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;
        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        let config = config_arg.value.assert_record()?;

        let mut inputs = crate::Record::default();
        inputs.insert(
            String::from("csrPem"),
            crate::Value::Str(config.get("csrPem").assert_str_ref()?.to_owned()),
        );
        inputs.insert(
            String::from("privateKeyPem"),
            crate::Value::Str(config.get("privateKeyPem").assert_str_ref()?.to_owned()),
        );
        inputs.insert(
            String::from("caCertPem"),
            config.get("caCertPem").clone(),
        );
        inputs.insert(
            String::from("validity"),
            config.get("validity").clone(),
        );

        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&format!("{:?}", inputs), &mut hasher);
        let resource_name = format!("{:x}", std::hash::Hasher::finish(&hasher));

        let resource_id = ids::ResourceId {
            typ: CERT_SIG_RESOURCE_TYPE.to_owned(),
            name: resource_name.clone(),
        };

        let Some(outputs) = eval_ctx.resource(
            CERT_SIG_RESOURCE_TYPE,
            &resource_name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            argument_dependencies.insert(resource_id);
            return Ok(crate::TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        let pem = outputs.get("pem").assert_str_ref()?;
        let mut out = crate::Record::default();
        out.insert(String::from("pem"), crate::Value::Str(pem.to_owned()));

        argument_dependencies.insert(resource_id);
        Ok(crate::TrackedValue::new(crate::Value::Record(out))
            .with_dependencies(argument_dependencies))
    });
}
