use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;

use crate::{auth, context::Context, output::OutputFormat};

#[derive(Args, Debug)]
pub struct OrgArgs {
    #[command(subcommand)]
    command: OrgCommand,
}

#[derive(Subcommand, Debug)]
enum OrgCommand {
    /// List organizations the current user is a member of.
    List,
    /// Create a new organization with the given name.
    Create {
        name: String,
        /// Skyr region the organization should belong to. Defaults to
        /// the creator's region when omitted.
        #[arg(long)]
        region: Option<String>,
    },
    /// Add a user to an organization.
    AddMember {
        organization: String,
        username: String,
    },
    /// Leave an organization.
    Leave { organization: String },
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

pub async fn run_org(args: OrgArgs, ctx: &Context) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, ctx.api_url()).await?;
    let endpoint = auth::graphql_endpoint(ctx.api_url());

    match args.command {
        OrgCommand::List => list_organizations(&client, &endpoint, &token, ctx.format).await,
        OrgCommand::Create { name, region } => {
            create_organization(
                &client,
                &endpoint,
                &token,
                &name,
                region.as_deref(),
                ctx.format,
            )
            .await
        }
        OrgCommand::AddMember {
            organization,
            username,
        } => {
            add_organization_member(
                &client,
                &endpoint,
                &token,
                &organization,
                &username,
                ctx.format,
            )
            .await
        }
        OrgCommand::Leave { organization } => {
            leave_organization(&client, &endpoint, &token, &organization, ctx.format).await
        }
    }
}

async fn list_organizations(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let data = auth::graphql_query::<ListOrganizations>(
        client,
        endpoint,
        token,
        list_organizations::Variables {},
        "organization list",
    )
    .await?;

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
    region: Option<&str>,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let data = auth::graphql_query::<CreateOrganization>(
        client,
        endpoint,
        token,
        create_organization::Variables {
            name: name.to_owned(),
            region: region.map(str::to_owned),
        },
        "organization create",
    )
    .await?;

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
    let data = auth::graphql_query::<AddOrganizationMember>(
        client,
        endpoint,
        token,
        add_organization_member::Variables {
            organization: organization.to_owned(),
            username: username.to_owned(),
        },
        "organization add-member",
    )
    .await?;

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
    auth::graphql_query::<LeaveOrganization>(
        client,
        endpoint,
        token,
        leave_organization::Variables {
            organization: organization.to_owned(),
        },
        "organization leave",
    )
    .await?;

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
