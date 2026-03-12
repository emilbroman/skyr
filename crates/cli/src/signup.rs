use anyhow::Context;
use clap::Args;
use graphql_client::GraphQLQuery;
use serde::Serialize;

use crate::{auth, output::OutputFormat};

#[derive(Args, Debug)]
pub struct SignupArgs {
    #[arg(long)]
    username: String,
    #[arg(long)]
    email: String,
    #[arg(long, default_value = "~/.ssh/id_ed25519")]
    key: String,
    #[arg(long, default_value = "localhost:8080")]
    api_url: String,
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/signup.graphql",
    response_derives = "Debug, serde::Serialize"
)]
struct Signup;

pub async fn run_signup(args: SignupArgs, format: OutputFormat) -> anyhow::Result<()> {
    let endpoint = auth::graphql_endpoint(&args.api_url);
    let client = reqwest::Client::new();

    let key_path = auth::expand_tilde(&args.key)?;
    let proof = auth::build_auth_proof(&client, &endpoint, &args.username, &key_path).await?;

    let body = Signup::build_query(signup::Variables {
        username: args.username.clone(),
        email: args.email.clone(),
        pubkey: proof.pubkey,
        signature: proof.signature,
    });

    let response = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .context("failed to send signup mutation")?;

    let response: graphql_client::Response<signup::ResponseData> = response
        .json()
        .await
        .context("failed to decode graphql response")?;
    let data = auth::graphql_response_data(response, "signup")?;
    auth::persist_auth_state(&data.signup.user.username, &key_path, &data.signup.token).await?;

    #[derive(Serialize)]
    struct SignupUserOutput {
        username: String,
        email: String,
        fullname: Option<String>,
    }

    #[derive(Serialize)]
    struct SignupOutput {
        user: SignupUserOutput,
        token: String,
    }

    let output = SignupOutput {
        user: SignupUserOutput {
            username: data.signup.user.username,
            email: data.signup.user.email,
            fullname: data.signup.user.fullname,
        },
        token: data.signup.token,
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
                output.user.username,
            ]));
            table.add_row(crate::output::row(vec![
                String::from("email"),
                output.user.email,
            ]));
            table.add_row(crate::output::row(vec![
                String::from("fullname"),
                output.user.fullname.unwrap_or_default(),
            ]));
            table.add_row(crate::output::row(vec![
                String::from("token"),
                output.token,
            ]));
            print!("{table}");
        }
    }

    Ok(())
}
