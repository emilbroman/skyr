use sha2::{Digest, Sha256};

const FILE_RESOURCE_TYPE: &str = "Std/Artifact.File";
const DEFAULT_MEDIA_TYPE: &str = "application/octet-stream";

/// Compute a content-addressed resource name from all inputs.
///
/// Hash is computed over canonicalized inputs (alphabetical key order):
/// `contents`, `mediaType` (with default resolved), `name`.
///
/// The hashed name is derived by splitting the user-provided `name` on the
/// last `.` and inserting `-{hash_hex[..32]}` between stem and extension.
fn content_addressed_name(name: &str, contents: &str, media_type: &str) -> String {
    let canonical = serde_json::json!({
        "contents": contents,
        "mediaType": media_type,
        "name": name,
    });
    let json_str = serde_json::to_string(&canonical).unwrap();
    let hash = Sha256::digest(json_str.as_bytes());
    let hash_hex = hex::encode(hash);
    let truncated = &hash_hex[..32];

    if let Some(dot_pos) = name.rfind('.') {
        let (stem, ext) = name.split_at(dot_pos);
        format!("{stem}-{truncated}{ext}")
    } else {
        format!("{name}-{truncated}")
    }
}

pub fn register_extern(eval: &mut impl super::ExternRegistry) {
    eval.add_extern_fn(FILE_RESOURCE_TYPE, |args, eval_ctx| {
        use crate::ValueAssertions;

        let (config, argument_dependencies) = match super::extract_config_arg(args)? {
            Ok(pair) => pair,
            Err(pending) => return Ok(pending),
        };

        let name = config.get("name").assert_str_ref()?;
        let media_type = match config.get("mediaType") {
            crate::Value::Nil => None,
            other => Some(other.assert_str_ref()?),
        };
        let contents = config.get("contents").assert_str_ref()?;
        let namespace = eval_ctx.namespace();

        // Resolve the default media type for hashing
        let resolved_media_type = media_type.unwrap_or(DEFAULT_MEDIA_TYPE);

        // Compute the content-addressed resource name
        let hashed_name = content_addressed_name(name, contents, resolved_media_type);

        let resource_id = ids::ResourceId {
            region: eval_ctx.region().clone(),
            typ: FILE_RESOURCE_TYPE.to_owned(),
            name: hashed_name.clone(),
        };

        let mut inputs = crate::Record::default();
        inputs.insert(String::from("name"), crate::Value::Str(hashed_name.clone()));
        inputs.insert(
            String::from("mediaType"),
            crate::Value::Str(resolved_media_type.to_owned()),
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
            &hashed_name,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            return Ok(crate::TrackedValue::pending().with_dependency(resource_id));
        };

        let mut out = crate::Record::default();
        let url = outputs.get("url").assert_str_ref()?;
        out.insert(String::from("url"), crate::Value::Str(url.to_owned()));

        Ok(crate::TrackedValue::new(crate::Value::Record(out)).with_dependency(resource_id))
    })
}
