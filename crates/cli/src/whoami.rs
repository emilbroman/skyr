use anyhow::{Context, anyhow};
use clap::Args;
use graphql_client::GraphQLQuery;
use serde::Serialize;

use crate::{auth, output::OutputFormat};

#[derive(Args, Debug)]
pub struct WhoamiArgs {
    #[arg(long, default_value = "localhost:8080")]
    api_url: String,
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/me.graphql",
    response_derives = "Debug, serde::Serialize"
)]
struct Me;

pub async fn run_whoami(args: WhoamiArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    let body = Me::build_query(me::Variables {});
    let response = client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to send me query")?;
    let response: graphql_client::Response<me::ResponseData> = response
        .json()
        .await
        .context("failed to decode me response")?;

    if let Some(errors) = response.errors {
        return Err(anyhow!(
            "whoami failed: {}",
            errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let data = response.data.context("me response did not include data")?;

    #[derive(Serialize)]
    struct WhoamiOutput {
        username: String,
        email: String,
        fullname: Option<String>,
    }

    let output = WhoamiOutput {
        username: data.me.username,
        email: data.me.email,
        fullname: data.me.fullname,
    };

    match format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}");
            table.add_row(crate::output::row(vec![
                String::from("FIELD"),
                String::from("VALUE"),
            ]));
            table.add_row(crate::output::row(vec![
                String::from("username"),
                output.username,
            ]));
            table.add_row(crate::output::row(vec![
                String::from("email"),
                output.email,
            ]));
            table.add_row(crate::output::row(vec![
                String::from("fullname"),
                output.fullname.unwrap_or_default(),
            ]));
            print!("{table}");
        }
    }

    Ok(())
}
