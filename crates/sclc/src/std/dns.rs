const A_RECORD_RESOURCE_TYPE: &str = "Std/DNS.ARecord";
const AAAA_RECORD_RESOURCE_TYPE: &str = "Std/DNS.AAAARecord";
const CNAME_RECORD_RESOURCE_TYPE: &str = "Std/DNS.CNAMERecord";
const TXT_RECORD_RESOURCE_TYPE: &str = "Std/DNS.TXTRecord";
const MX_RECORD_RESOURCE_TYPE: &str = "Std/DNS.MXRecord";
const SRV_RECORD_RESOURCE_TYPE: &str = "Std/DNS.SRVRecord";

fn register_record_extern(registry: &mut impl super::ExternRegistry, resource_type: &'static str) {
    registry.add_extern_fn(resource_type, move |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, deps) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };

        let name = config.get("name").assert_str_ref()?;

        let resource_id = ids::ResourceId {
            region: eval_ctx.region().clone(),
            typ: resource_type.to_string(),
            name: name.to_owned(),
        };

        let mut inputs = crate::Record::default();
        for (k, v) in config.iter() {
            inputs.insert(k.to_string(), v.clone());
        }

        let Some(outputs) = eval_ctx.resource(resource_type, name, &inputs, deps.clone())? else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let mut merged = inputs;
        for (k, v) in outputs.iter() {
            merged.insert(k.to_owned(), v.clone());
        }
        Ok(crate::TrackedValue::new(crate::Value::Record(merged)).with_dependency(resource_id))
    })
}

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    register_record_extern(eval, A_RECORD_RESOURCE_TYPE);
    register_record_extern(eval, AAAA_RECORD_RESOURCE_TYPE);
    register_record_extern(eval, CNAME_RECORD_RESOURCE_TYPE);
    register_record_extern(eval, TXT_RECORD_RESOURCE_TYPE);
    register_record_extern(eval, MX_RECORD_RESOURCE_TYPE);
    register_record_extern(eval, SRV_RECORD_RESOURCE_TYPE);
}
