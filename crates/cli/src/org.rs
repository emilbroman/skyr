use anyhow::Context;
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;

use crate::{auth, output::OutputFormat};

#[derive(Args, Debug)]
pub struct OrgArgs {
    #[command(subcommand)]
    command: OrgCommand,
    #[arg(long, default_value = "https://skyr.cloud")]
    api_url: String,
}

#[derive(Subcommand, Debug)]
enum OrgCommand {
    List,
    Create {
        name: String,
    },
    AddMember {
        organization: String,
        username: String,
    },
    Leave {
        organization: String,
    },
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/list_organizations.graphql"
)]
struct ListOrganizations;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/create_organization.graphql"
)]
struct CreateOrganization;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/add_organization_member.graphql"
)]
struct AddOrganizationMember;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/leave_organization.graphql"
)]
struct LeaveOrganization;

pub async fn run_org(args: OrgArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    match args.command {
        OrgCommand::List => {
            list_organizations(&client, &endpoint, &token, format).await?;
        }
        OrgCommand::Create { name } => {
            create_organization(&client, &endpoint, &token, &name, format).await?;
        }
        OrgCommand::AddMember {
            organization,
            username,
        } => {
            add_organization_member(&client, &endpoint, &token, &organization, &username, format)
                .await?;
        }
        OrgCommand::Leave { organization } => {
            leave_organization(&client, &endpoint, &token, &organization, format).await?;
        }
    }

    Ok(())
}

async fn list_organizations(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let body = ListOrganizations::build_query(list_organizations::Variables {});
    let response = client
        .post(endpoint)
        .header(
            reqwest::header::AUTHORIZATION,
            auth::bearer_header_value(token)?,
        )
        .json(&body)
        .send()
        .await
        .context("failed to send organizations query")?;
    let response: graphql_client::Response<list_organizations::ResponseData> = response
        .json()
        .await
        .context("failed to decode organizations response")?;
    let data = auth::graphql_response_data(response, "organization list")?;

    #[derive(Serialize)]
    struct OrgOutput {
        name: String,
        members: Vec<String>,
    }

    let orgs = data
        .organizations
        .into_iter()
        .map(|org| OrgOutput {
            name: org.name,
            members: org.members.into_iter().map(|m| m.username).collect(),
        })
        .collect::<Vec<_>>();

    match format {
        OutputFormat::Json => crate::output::print_json(&orgs)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}");
            table.add_row(crate::output::row(vec![
                String::from("NAME"),
                String::from("MEMBERS"),
            ]));
            for org in orgs {
                table.add_row(crate::output::row(vec![org.name, org.members.join(", ")]));
            }
            print!("{table}");
        }
    }
    Ok(())
}

async fn create_organization(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    name: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let body = CreateOrganization::build_query(create_organization::Variables {
        name: name.to_owned(),
    });
    let response = client
        .post(endpoint)
        .header(
            reqwest::header::AUTHORIZATION,
            auth::bearer_header_value(token)?,
        )
        .json(&body)
        .send()
        .await
        .context("failed to send create organization mutation")?;
    let response: graphql_client::Response<create_organization::ResponseData> = response
        .json()
        .await
        .context("failed to decode create organization response")?;
    let data = auth::graphql_response_data(response, "organization create")?;

    #[derive(Serialize)]
    struct CreateOrgOutput {
        name: String,
    }

    let output = CreateOrgOutput {
        name: data.create_organization.name,
    };

    match format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => println!("{}", output.name),
    }

    Ok(())
}

async fn add_organization_member(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    organization: &str,
    username: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let body = AddOrganizationMember::build_query(add_organization_member::Variables {
        organization: organization.to_owned(),
        username: username.to_owned(),
    });
    let response = client
        .post(endpoint)
        .header(
            reqwest::header::AUTHORIZATION,
            auth::bearer_header_value(token)?,
        )
        .json(&body)
        .send()
        .await
        .context("failed to send add organization member mutation")?;
    let response: graphql_client::Response<add_organization_member::ResponseData> = response
        .json()
        .await
        .context("failed to decode add organization member response")?;
    let data = auth::graphql_response_data(response, "organization add-member")?;

    #[derive(Serialize)]
    struct AddMemberOutput {
        name: String,
        members: Vec<String>,
    }

    let output = AddMemberOutput {
        name: data.add_organization_member.name,
        members: data
            .add_organization_member
            .members
            .into_iter()
            .map(|m| m.username)
            .collect(),
    };

    match format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => {
            println!("Added {} to {}", username, output.name);
            println!("Members: {}", output.members.join(", "));
        }
    }

    Ok(())
}

async fn leave_organization(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    organization: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let body = LeaveOrganization::build_query(leave_organization::Variables {
        organization: organization.to_owned(),
    });
    let response = client
        .post(endpoint)
        .header(
            reqwest::header::AUTHORIZATION,
            auth::bearer_header_value(token)?,
        )
        .json(&body)
        .send()
        .await
        .context("failed to send leave organization mutation")?;
    let response: graphql_client::Response<leave_organization::ResponseData> = response
        .json()
        .await
        .context("failed to decode leave organization response")?;
    auth::graphql_response_data(response, "organization leave")?;

    #[derive(Serialize)]
    struct LeaveOrgOutput {
        organization: String,
    }

    let output = LeaveOrgOutput {
        organization: organization.to_owned(),
    };

    match format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => println!("Left {}", organization),
    }

    Ok(())
}
