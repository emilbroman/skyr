use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;

use crate::auth;

#[derive(Args, Debug)]
pub struct RepoArgs {
    #[command(subcommand)]
    command: RepoCommand,
    #[arg(long, default_value = "localhost:8080")]
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

pub async fn run_repo(args: RepoArgs) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    match args.command {
        RepoCommand::List => {
            list_repositories(&client, &endpoint, &token).await?;
        }
        RepoCommand::Create { repository } => {
            create_repository(&client, &endpoint, &token, &repository).await?;
        }
    }

    Ok(())
}

async fn create_repository(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
) -> anyhow::Result<()> {
    let (organization, repository) = parse_repository_path(repository)?;
    let body = CreateRepository::build_query(create_repository::Variables {
        organization: organization.to_owned(),
        repository: repository.to_owned(),
    });
    let response = client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to send create repository mutation")?;
    let response: graphql_client::Response<create_repository::ResponseData> = response
        .json()
        .await
        .context("failed to decode create repository response")?;

    if let Some(errors) = response.errors {
        return Err(anyhow!(
            "repository create failed: {}",
            errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let data = response
        .data
        .context("create repository response did not include data")?;
    println!("{}", data.create_repository.name);
    Ok(())
}

async fn list_repositories(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
) -> anyhow::Result<()> {
    let body = ListRepositories::build_query(list_repositories::Variables {});
    let response = client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to send repositories query")?;
    let response: graphql_client::Response<list_repositories::ResponseData> = response
        .json()
        .await
        .context("failed to decode repositories response")?;

    if let Some(errors) = response.errors {
        return Err(anyhow!(
            "repository list failed: {}",
            errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let data = response
        .data
        .context("repositories response did not include data")?;
    for repo in data.repositories {
        println!("{}", repo.name);
    }
    Ok(())
}

pub(crate) fn parse_repository_path(repository: &str) -> anyhow::Result<(&str, &str)> {
    let (organization, repository_name) = repository
        .split_once('/')
        .ok_or_else(|| anyhow!("repository must be in <organization>/<repository> format"))?;
    if organization.is_empty() || repository_name.is_empty() || repository_name.contains('/') {
        return Err(anyhow!(
            "repository must be in <organization>/<repository> format"
        ));
    }
    Ok((organization, repository_name))
}
