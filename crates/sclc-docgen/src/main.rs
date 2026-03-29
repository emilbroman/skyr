use std::collections::BTreeMap;

use sclc::{RecordType, TypeKind};

#[derive(serde::Serialize)]
struct ModuleTypes<'a> {
    value_exports: &'a RecordType,
    type_exports: &'a RecordType,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stdlib = sclc::stdlib_types()
        .await
        .expect("failed to compile stdlib");

    let mut modules: BTreeMap<String, ModuleTypes> = BTreeMap::new();

    for (module_id, (value_type, type_level)) in &stdlib {
        if let TypeKind::Record(value_exports) = &value_type.kind {
            modules.insert(
                module_id.to_string(),
                ModuleTypes {
                    value_exports,
                    type_exports: type_level,
                },
            );
        }
    }

    let json = serde_json::to_string_pretty(&modules).expect("failed to serialize");
    println!("{json}");
}
