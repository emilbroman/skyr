use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;

use crate::{auth, output::OutputFormat, repo};

#[allow(clippy::upper_case_acronyms)]
type JSON = serde_json::Value;

#[derive(Args, Debug)]
pub struct ResourcesArgs {
    #[command(subcommand)]
    command: ResourcesCommand,
    #[arg(long, default_value = "https://skyr.cloud")]
    api_url: String,
}

#[derive(Subcommand, Debug)]
enum ResourcesCommand {
    List { repository: String },
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/list_repository_resources.graphql",
    response_derives = "Debug"
)]
struct ListRepositoryResources;

pub async fn run_resources(args: ResourcesArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    match args.command {
        ResourcesCommand::List { repository } => {
            list_resources(&client, &endpoint, &token, &repository, format).await?;
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct ResourceListOutput {
    r#type: String,
    name: String,
    owner: String,
    inputs: Option<serde_json::Value>,
    outputs: Option<serde_json::Value>,
}

async fn list_resources(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let (_, repository_name) = repo::parse_repository_path(repository)?;

    let body = ListRepositoryResources::build_query(list_repository_resources::Variables {});
    let response = client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to send resources query")?;
    let response: graphql_client::Response<list_repository_resources::ResponseData> = response
        .json()
        .await
        .context("failed to decode resources response")?;
    let data = auth::graphql_response_data(response, "resource list")?;
    let repository = data
        .repositories
        .into_iter()
        .find(|repo| repo.name == repository_name)
        .ok_or_else(|| anyhow!("repository '{repository_name}' not found"))?;

    let resources: Vec<ResourceListOutput> = repository
        .environments
        .into_iter()
        .flat_map(|env| {
            env.resources.into_iter().map(|resource| {
                let owner = resource
                    .owner
                    .map(|o| o.id.chars().take(8).collect::<String>())
                    .unwrap_or_default();
                ResourceListOutput {
                    r#type: resource.type_,
                    name: resource.name,
                    owner,
                    inputs: resource.inputs,
                    outputs: resource.outputs,
                }
            })
        })
        .collect();

    match format {
        OutputFormat::Json => crate::output::print_json(&resources)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}  {:<}  {:<}  {:<}");
            table.add_row(crate::output::row(vec![
                String::from("TYPE"),
                String::from("NAME"),
                String::from("OWNER"),
                String::from("INPUTS"),
                String::from("OUTPUTS"),
            ]));
            for resource in resources {
                let inputs = resource
                    .inputs
                    .map(|v| serde_json::to_string(&v).unwrap_or_default())
                    .unwrap_or_default();
                let outputs = resource
                    .outputs
                    .map(|v| serde_json::to_string(&v).unwrap_or_default())
                    .unwrap_or_default();
                table.add_row(crate::output::row(vec![
                    resource.r#type,
                    resource.name,
                    resource.owner,
                    inputs,
                    outputs,
                ]));
            }
            print!("{table}");
        }
    }

    Ok(())
}
