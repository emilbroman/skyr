use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;

use crate::{auth, output::OutputFormat, repo};

#[allow(non_camel_case_types)]
type JSON = serde_json::Value;

#[derive(Args, Debug)]
pub struct DeploymentsArgs {
    #[command(subcommand)]
    command: DeploymentsCommand,
    #[arg(long, default_value = "localhost:8080")]
    api_url: String,
}

#[derive(Subcommand, Debug)]
enum DeploymentsCommand {
    List { repository: String },
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/list_repository_deployments.graphql",
    response_derives = "Debug"
)]
struct ListRepositoryDeployments;

pub async fn run_deployments(args: DeploymentsArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    match args.command {
        DeploymentsCommand::List { repository } => {
            list_deployments(&client, &endpoint, &token, &repository, format).await?;
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct DeploymentOutput {
    r#ref: String,
    commit: String,
    created_at: String,
    state: String,
    resources: Vec<ResourceOutput>,
}

#[derive(Serialize)]
struct ResourceOutput {
    r#type: String,
    id: String,
    inputs: Option<serde_json::Value>,
    outputs: Option<serde_json::Value>,
    dependencies: Vec<ResourceDependencyOutput>,
}

#[derive(Serialize)]
struct ResourceDependencyOutput {
    r#type: String,
    id: String,
}

async fn list_deployments(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let (_, repository_name) = repo::parse_repository_path(repository)?;

    let body = ListRepositoryDeployments::build_query(list_repository_deployments::Variables {});
    let response = client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to send deployments query")?;
    let response: graphql_client::Response<list_repository_deployments::ResponseData> = response
        .json()
        .await
        .context("failed to decode deployments response")?;

    if let Some(errors) = response.errors {
        return Err(anyhow!(
            "deployment list failed: {}",
            errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let data = response
        .data
        .context("deployments response did not include data")?;
    let repository = data
        .repositories
        .into_iter()
        .find(|repo| repo.name == repository_name)
        .ok_or_else(|| anyhow!("repository '{}' not found", repository))?;

    let deployments = repository
        .deployments
        .into_iter()
        .map(|deployment| DeploymentOutput {
            r#ref: deployment.ref_.to_owned(),
            commit: deployment.commit.to_owned(),
            created_at: deployment.created_at.to_owned(),
            state: format!("{:?}", deployment.state),
            resources: deployment
                .resources
                .into_iter()
                .map(|resource| ResourceOutput {
                    r#type: resource.type_.to_owned(),
                    id: resource.id,
                    inputs: resource.inputs,
                    outputs: resource.outputs,
                    dependencies: resource
                        .dependencies
                        .into_iter()
                        .map(|dependency| ResourceDependencyOutput {
                            r#type: dependency.type_,
                            id: dependency.id,
                        })
                        .collect::<Vec<_>>(),
                })
                .collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>();

    match format {
        OutputFormat::Json => crate::output::print_json(&deployments)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}  {:<}  {:<}  {:>}");
            table.add_row(crate::output::row(vec![
                String::from("REF"),
                String::from("COMMIT"),
                String::from("CREATED"),
                String::from("STATE"),
                String::from("RESOURCES"),
            ]));
            for deployment in deployments {
                table.add_row(crate::output::row(vec![
                    deployment.r#ref,
                    crate::output::shorten_commit_hash(&deployment.commit),
                    crate::output::format_created_at(&deployment.created_at),
                    deployment.state,
                    deployment.resources.len().to_string(),
                ]));
            }
            print!("{table}");
        }
    }

    Ok(())
}
