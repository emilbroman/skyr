use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Parser;
use sclc::{RecordType, TypeKind};

#[derive(serde::Serialize)]
struct ModuleTypes<'a> {
    value_exports: &'a RecordType,
    type_exports: &'a RecordType,
}

#[derive(Parser, Debug)]
#[command(name = "sclc-docgen", about = "Generate stdlib type documentation")]
struct Cli {
    /// Output file path
    #[arg(short = 'f', long = "output-file")]
    output_file: PathBuf,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();

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
    std::fs::write(&cli.output_file, json).expect("failed to write output file");
}
