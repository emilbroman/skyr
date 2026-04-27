use clap::Parser;
use std::path::PathBuf;

mod api;
mod auth;
mod cache;
mod context;
mod deployment;
mod deps;
mod fmt;
mod git_client;
mod git_context;
mod lsp;
mod org;
mod output;
mod port_forward;
mod repl;
mod repo;
mod resolver;
mod resource;
mod run;
mod ws;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Program {
    /// Output format for command results.
    #[arg(long, global = true, value_enum, default_value_t = output::OutputFormat::Text)]
    format: output::OutputFormat,
    /// Skyr API base URL.
    #[arg(
        long,
        global = true,
        env = "SKYR_API_URL",
        default_value = "https://skyr.cloud"
    )]
    api_url: String,
    /// Override the organization (default: parsed from `origin` git remote).
    #[arg(long, global = true, env = "SKYR_ORG")]
    org: Option<String>,
    /// Override the repository (default: parsed from `origin` git remote).
    #[arg(long, global = true, env = "SKYR_REPO")]
    repo: Option<String>,
    /// Override the environment (default: current git branch).
    #[arg(long, global = true, env = "SKYR_ENV")]
    env: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Lsp {
        /// SSH git server address for fetching dependencies (host:port)
        #[arg(long, default_value = "skyr.cloud:22")]
        git_server: String,
    },
    Repl {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "Local")]
        package: String,
        /// SSH git server address for fetching dependencies (host:port)
        #[arg(long, default_value = "skyr.cloud:22")]
        git_server: String,
    },
    Run {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "Local")]
        package: String,
        /// SSH git server address for fetching dependencies (host:port)
        #[arg(long, default_value = "skyr.cloud:22")]
        git_server: String,
    },
    /// Sign in/up, sign out, or print the current user.
    Auth(auth::AuthArgs),
    /// Direct GraphQL query/mutation access.
    Api(api::ApiArgs),
    Org(org::OrgArgs),
    Repo(repo::RepoArgs),
    Deployments(deployment::DeploymentsArgs),
    Deps(deps::DepsArgs),
    Resources(resource::ResourcesArgs),
    PortForward(port_forward::PortForwardArgs),
    Fmt(fmt::FmtArgs),
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
    let ctx = context::Context::new(
        program.api_url,
        program.format,
        program.org,
        program.repo,
        program.env,
    );

    match program.command {
        Command::Lsp { git_server } => {
            lsp::run_lsp(git_server).await?;
        }
        Command::Repl {
            root,
            package,
            git_server,
        } => {
            repl::run_repl(root, package, git_server).await?;
        }
        Command::Run {
            root,
            package,
            git_server,
        } => {
            run::run_program(root, package, git_server).await?;
        }
        Command::Auth(args) => {
            auth::run_auth(args, &ctx).await?;
        }
        Command::Api(args) => {
            api::run_api(args, &ctx).await?;
        }
        Command::Org(args) => {
            org::run_org(args, &ctx).await?;
        }
        Command::Repo(args) => {
            repo::run_repo(args, &ctx).await?;
        }
        Command::Deployments(args) => {
            deployment::run_deployments(args, &ctx).await?;
        }
        Command::Deps(args) => {
            deps::run_deps(args, ctx.format).await?;
        }
        Command::Resources(args) => {
            resource::run_resources(args, &ctx).await?;
        }
        Command::PortForward(args) => {
            port_forward::run_port_forward(args).await?;
        }
        Command::Fmt(args) => {
            fmt::run_fmt(args)?;
        }
    }

    Ok(())
}
