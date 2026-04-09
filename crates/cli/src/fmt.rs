use std::path::PathBuf;

use clap::Args;

#[derive(Args)]
pub struct FmtArgs {
    /// The file to format
    pub file: PathBuf,

    /// Write the formatted output back to the file instead of printing to stdout
    #[arg(long)]
    pub write: bool,
}

pub fn run_fmt(args: FmtArgs) -> anyhow::Result<()> {
    let source = std::fs::read_to_string(&args.file)?;

    let stem = args
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent_name = args
        .file
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Local".to_string());
    let module_id = sclc::ModuleId::new(sclc::PackageId::from([parent_name]), vec![stem]);

    let diagnosed = sclc::parse_file_mod(&source, &module_id);

    if diagnosed.diags().has_errors() {
        for diag in diagnosed.diags().iter() {
            eprintln!("{diag}");
        }
        anyhow::bail!("cannot format file with syntax errors");
    }

    let file_mod = diagnosed.into_inner();

    let formatted = sclc::Formatter::format(&source, &file_mod);

    if args.write {
        std::fs::write(&args.file, &formatted)?;
    } else {
        print!("{formatted}");
    }

    Ok(())
}
