use clap::Parser;
use std::path::PathBuf;

mod auth;
mod fs_source;
mod repl;
mod run;
mod signin;
mod signup;
mod whoami;

#[derive(Parser)]
enum Program {
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
        Program::Signin(args) => {
            signin::run_signin(args).await?;
        }
        Program::Signup(args) => {
            signup::run_signup(args).await?;
        }
        Program::Whoami(args) => {
            whoami::run_whoami(args).await?;
        }
    }

    Ok(())
}
