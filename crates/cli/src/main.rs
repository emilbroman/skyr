use clap::Parser;
use std::path::PathBuf;

mod auth;
mod deployment;
mod fs_source;
mod output;
mod repl;
mod repo;
mod run;
mod signin;
mod signup;
mod whoami;

#[derive(Parser)]
struct Program {
    #[arg(long, global = true, value_enum, default_value_t = output::OutputFormat::Text)]
    format: output::OutputFormat,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Repl,
    Run {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "Local")]
        package: String,
    },
    Signin(signin::SigninArgs),
    Signup(signup::SignupArgs),
    Whoami(whoami::WhoamiArgs),
    Repo(repo::RepoArgs),
    Deployments(deployment::DeploymentsArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let program = Program::parse();

    match program.command {
        Command::Repl => {
            repl::run_repl().await?;
        }
        Command::Run { root, package } => {
            run::run_program(root, package).await?;
        }
        Command::Signin(args) => {
            signin::run_signin(args, program.format).await?;
        }
        Command::Signup(args) => {
            signup::run_signup(args, program.format).await?;
        }
        Command::Whoami(args) => {
            whoami::run_whoami(args, program.format).await?;
        }
        Command::Repo(args) => {
            repo::run_repo(args, program.format).await?;
        }
        Command::Deployments(args) => {
            deployment::run_deployments(args, program.format).await?;
        }
    }

    Ok(())
}
