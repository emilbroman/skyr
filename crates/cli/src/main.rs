use clap::Parser;
use std::path::PathBuf;

mod fs_source;
mod repl;
mod run;

#[derive(Parser)]
enum Program {
    Repl,
    Run {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "Local")]
        package: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Program::parse() {
        Program::Repl => {
            repl::run_repl().await?;
        }
        Program::Run { root, package } => {
            run::run_program(root, package).await?;
        }
    }

    Ok(())
}
