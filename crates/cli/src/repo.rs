use anyhow::anyhow;
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;

use crate::{auth, context::Context, output::OutputFormat};

#[derive(Args, Debug)]
pub struct RepoArgs {
    #[command(subcommand)]
    command: RepoCommand,
}

#[derive(Subcommand, Debug)]
enum RepoCommand {
    /// List repositories visible to the signed-in user.
    List,
    /// Create a new repository in the current org (or `--org`).
    Create {
        /// Repository name (just the repo, not `org/repo`).
        name: String,
        /// Skyr region the repository should belong to. Defaults to the
        /// org's region when omitted.
        #[arg(long)]
        region: Option<String>,
    },
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/list_repositories.graphql"
)]
struct ListRepositories;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/create_repository.graphql"
)]
struct CreateRepository;

pub async fn run_repo(args: RepoArgs, ctx: &Context) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, ctx.api_url()).await?;
    let endpoint = auth::graphql_endpoint(ctx.api_url());

    match args.command {
        RepoCommand::List => {
            let org = ctx.org()?.to_owned();
            list_repositories(&client, &endpoint, &token, &org, ctx.format).await
        }
        RepoCommand::Create { name, region } => {
            let org = ctx.org()?;
            validate_segment(org).map_err(|e| anyhow!("--org `{org}`: {e}"))?;
            validate_segment(&name).map_err(|e| anyhow!("repository name `{name}`: {e}"))?;
            create_repository(
                &client,
                &endpoint,
                &token,
                org,
                &name,
                region.as_deref(),
                ctx.format,
            )
            .await
        }
    }
}

async fn create_repository(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    organization: &str,
    repository: &str,
    region: Option<&str>,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let data = auth::graphql_query::<CreateRepository>(
        client,
        endpoint,
        token,
        create_repository::Variables {
            organization: organization.to_owned(),
            repository: repository.to_owned(),
            region: region.map(str::to_owned),
        },
        "repository create",
    )
    .await?;

    #[derive(Serialize)]
    struct CreateRepositoryOutput {
        name: String,
    }

    let output = CreateRepositoryOutput {
        name: data.create_repository.name,
    };

    match format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => println!("{}", output.name),
    }

    Ok(())
}

async fn list_repositories(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    organization: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let data = auth::graphql_query::<ListRepositories>(
        client,
        endpoint,
        token,
        list_repositories::Variables {
            organization: organization.to_owned(),
        },
        "repository list",
    )
    .await?;

    #[derive(Serialize)]
    struct RepositoryOutput {
        name: String,
    }

    let repositories = data
        .organization
        .repositories
        .into_iter()
        .map(|repo| RepositoryOutput { name: repo.name })
        .collect::<Vec<_>>();

    match format {
        OutputFormat::Json => crate::output::print_json(&repositories)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}");
            table.add_row(crate::output::row(vec![String::from("NAME")]));
            for repo in repositories {
                table.add_row(crate::output::row(vec![repo.name]));
            }
            print!("{table}");
        }
    }
    Ok(())
}

fn validate_segment(s: &str) -> Result<(), &'static str> {
    if s.is_empty() {
        return Err("must not be empty");
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("must only contain alphanumeric characters, hyphens, or underscores");
    }
    Ok(())
}
