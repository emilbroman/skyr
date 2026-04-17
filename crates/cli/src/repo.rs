use anyhow::anyhow;
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;

use crate::{auth, output::OutputFormat};

#[derive(Args, Debug)]
pub struct RepoArgs {
    #[command(subcommand)]
    command: RepoCommand,
    #[arg(long, default_value = "https://skyr.cloud")]
    api_url: String,
}

#[derive(Subcommand, Debug)]
enum RepoCommand {
    List,
    Create { repository: String },
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

pub async fn run_repo(args: RepoArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    match args.command {
        RepoCommand::List => {
            list_repositories(&client, &endpoint, &token, format).await?;
        }
        RepoCommand::Create { repository } => {
            create_repository(&client, &endpoint, &token, &repository, format).await?;
        }
    }

    Ok(())
}

async fn create_repository(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let (organization, repository) = parse_repository_path(repository)?;
    let data = auth::graphql_query::<CreateRepository>(
        client,
        endpoint,
        token,
        create_repository::Variables {
            organization: organization.to_owned(),
            repository: repository.to_owned(),
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
    format: OutputFormat,
) -> anyhow::Result<()> {
    let data = auth::graphql_query::<ListRepositories>(
        client,
        endpoint,
        token,
        list_repositories::Variables {},
        "repository list",
    )
    .await?;

    #[derive(Serialize)]
    struct RepositoryOutput {
        name: String,
    }

    let repositories = data
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

/// Validate that a segment (organization or repository name) contains only
/// allowed characters: alphanumeric, hyphens, and underscores.
fn is_valid_path_segment(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub(crate) fn parse_repository_path(repository: &str) -> anyhow::Result<(&str, &str)> {
    let (organization, repository_name) = repository
        .split_once('/')
        .ok_or_else(|| anyhow!("repository must be in <organization>/<repository> format"))?;
    if !is_valid_path_segment(organization) || !is_valid_path_segment(repository_name) {
        return Err(anyhow!(
            "repository path segments must be non-empty and contain only \
             alphanumeric characters, hyphens, or underscores"
        ));
    }
    Ok((organization, repository_name))
}
