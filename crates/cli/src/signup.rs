use anyhow::{Context, anyhow};
use clap::Args;
use graphql_client::GraphQLQuery;

use crate::auth;

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

pub async fn run_signup(args: SignupArgs) -> anyhow::Result<()> {
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

    if let Some(errors) = response.errors {
        return Err(anyhow!(
            "signup failed: {}",
            errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let data = response
        .data
        .context("signup response did not include data")?;
    auth::persist_auth_state(&data.signup.user.username, &key_path, &data.signup.token).await?;
    println!("{}", serde_json::to_string_pretty(&data.signup)?);

    Ok(())
}
