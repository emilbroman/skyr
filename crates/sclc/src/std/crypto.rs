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

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn(ED25519_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, argument_dependencies) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let name = config.get("name").assert_str_ref()?;

        let resource_id = ids::ResourceId {
            region: eval_ctx.region().clone(),
            typ: ED25519_RESOURCE_TYPE.to_owned(),
            name: name.to_owned(),
        };

        let inputs = crate::Record::default();

        let Some(outputs) = eval_ctx.resource(
            None,
            ED25519_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let out = extract_key_outputs(&outputs)?;
        Ok(crate::TrackedValue::new(crate::Value::Record(out)).with_dependency(resource_id))
    });

    eval.add_extern_fn(ECDSA_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, argument_dependencies) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let name = config.get("name").assert_str_ref()?;

        let curve = match config.get("curve") {
            crate::Value::Nil => "P-256",
            other => other.assert_str_ref()?,
        };

        let resource_id = ids::ResourceId {
            region: eval_ctx.region().clone(),
            typ: ECDSA_RESOURCE_TYPE.to_owned(),
            name: name.to_owned(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("curve"), crate::Value::Str(curve.to_owned()));

        let Some(outputs) = eval_ctx.resource(
            None,
            ECDSA_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let out = extract_key_outputs(&outputs)?;
        Ok(crate::TrackedValue::new(crate::Value::Record(out)).with_dependency(resource_id))
    });

    eval.add_extern_fn(CSR_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, argument_dependencies) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
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
            region: eval_ctx.region().clone(),
            typ: CSR_RESOURCE_TYPE.to_owned(),
            name: resource_name.clone(),
        };

        let Some(outputs) = eval_ctx.resource(
            None,
            CSR_RESOURCE_TYPE,
            &resource_name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let pem = outputs.get("pem").assert_str_ref()?;
        let mut out = crate::Record::default();
        out.insert(String::from("pem"), crate::Value::Str(pem.to_owned()));

        Ok(crate::TrackedValue::new(crate::Value::Record(out)).with_dependency(resource_id))
    });

    eval.add_extern_fn(RSA_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, argument_dependencies) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };
        let name = config.get("name").assert_str_ref()?;

        let size = match config.get("size") {
            crate::Value::Nil => 2048,
            other => *other.assert_int_ref()?,
        };

        let resource_id = ids::ResourceId {
            region: eval_ctx.region().clone(),
            typ: RSA_RESOURCE_TYPE.to_owned(),
            name: name.to_owned(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("size"), crate::Value::Int(size));

        let Some(outputs) = eval_ctx.resource(
            None,
            RSA_RESOURCE_TYPE,
            name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let out = extract_key_outputs(&outputs)?;
        Ok(crate::TrackedValue::new(crate::Value::Record(out)).with_dependency(resource_id))
    });

    eval.add_extern_fn(CERT_SIG_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, argument_dependencies) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };

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
            region: eval_ctx.region().clone(),
            typ: CERT_SIG_RESOURCE_TYPE.to_owned(),
            name: resource_name.clone(),
        };

        let Some(outputs) = eval_ctx.resource(
            None,
            CERT_SIG_RESOURCE_TYPE,
            &resource_name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let pem = outputs.get("pem").assert_str_ref()?;
        let mut out = crate::Record::default();
        out.insert(String::from("pem"), crate::Value::Str(pem.to_owned()));

        Ok(crate::TrackedValue::new(crate::Value::Record(out)).with_dependency(resource_id))
    });

    eval.add_extern_fn("Std/Crypto.sha1", |args, _ctx| {
        use crate::ValueAssertions;
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }
        first.try_map(|value| {
            let input = value.assert_str()?;
            use sha1::Digest;
            let mut hasher = sha1::Sha1::new();
            hasher.update(input.as_bytes());
            let digest = hasher.finalize();
            Ok(crate::Value::Str(hex::encode(digest)))
        })
    });

    eval.add_extern_fn("Std/Crypto.sha256", |args, _ctx| {
        use crate::ValueAssertions;
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }
        first.try_map(|value| {
            let input = value.assert_str()?;
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(input.as_bytes());
            let digest = hasher.finalize();
            Ok(crate::Value::Str(hex::encode(digest)))
        })
    });

    eval.add_extern_fn("Std/Crypto.sha512", |args, _ctx| {
        use crate::ValueAssertions;
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }
        first.try_map(|value| {
            let input = value.assert_str()?;
            use sha2::Digest;
            let mut hasher = sha2::Sha512::new();
            hasher.update(input.as_bytes());
            let digest = hasher.finalize();
            Ok(crate::Value::Str(hex::encode(digest)))
        })
    });

    eval.add_extern_fn("Std/Crypto.md5", |args, _ctx| {
        use crate::ValueAssertions;
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        if first.value.has_pending() {
            return Ok(crate::TrackedValue::pending().with_dependencies(first.dependencies));
        }
        first.try_map(|value| {
            let input = value.assert_str()?;
            use md5::Digest;
            let mut hasher = md5::Md5::new();
            hasher.update(input.as_bytes());
            let digest = hasher.finalize();
            Ok(crate::Value::Str(hex::encode(digest)))
        })
    });

    eval.add_extern_fn("Std/Crypto.hmacSha256", |args, _ctx| {
        use crate::ValueAssertions;
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let second = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        if first.value.has_pending() || second.value.has_pending() {
            let deps: std::collections::BTreeSet<_> = first
                .dependencies
                .union(&second.dependencies)
                .cloned()
                .collect();
            return Ok(crate::TrackedValue::pending().with_dependencies(deps));
        }
        let deps: std::collections::BTreeSet<_> = first
            .dependencies
            .union(&second.dependencies)
            .cloned()
            .collect();
        let key = first.value.assert_str()?;
        let message = second.value.assert_str()?;
        use hmac::{Hmac, Mac};
        type HmacSha256 = Hmac<sha2::Sha256>;
        let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|err| {
            crate::EvalErrorKind::Custom(format!("Std/Crypto.hmacSha256: invalid key: {err}"))
        })?;
        mac.update(message.as_bytes());
        let digest = mac.finalize().into_bytes();
        Ok(crate::TrackedValue::new(crate::Value::Str(hex::encode(digest))).with_dependencies(deps))
    });

    eval.add_extern_fn("Std/Crypto.hmacSha512", |args, _ctx| {
        use crate::ValueAssertions;
        let mut args = args.into_iter();
        let first = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        let second = args
            .next()
            .unwrap_or_else(|| crate::TrackedValue::new(crate::Value::Nil));
        if first.value.has_pending() || second.value.has_pending() {
            let deps: std::collections::BTreeSet<_> = first
                .dependencies
                .union(&second.dependencies)
                .cloned()
                .collect();
            return Ok(crate::TrackedValue::pending().with_dependencies(deps));
        }
        let deps: std::collections::BTreeSet<_> = first
            .dependencies
            .union(&second.dependencies)
            .cloned()
            .collect();
        let key = first.value.assert_str()?;
        let message = second.value.assert_str()?;
        use hmac::{Hmac, Mac};
        type HmacSha512 = Hmac<sha2::Sha512>;
        let mut mac = HmacSha512::new_from_slice(key.as_bytes()).map_err(|err| {
            crate::EvalErrorKind::Custom(format!("Std/Crypto.hmacSha512: invalid key: {err}"))
        })?;
        mac.update(message.as_bytes());
        let digest = mac.finalize().into_bytes();
        Ok(crate::TrackedValue::new(crate::Value::Str(hex::encode(digest))).with_dependencies(deps))
    });
}
