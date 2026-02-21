use clap::Args;

use crate::auth;

#[derive(Args, Debug)]
pub struct SigninArgs {
    #[arg(long)]
    username: String,
    #[arg(long, default_value = "~/.ssh/id_ed25519")]
    key: String,
    #[arg(long, default_value = "localhost:8080")]
    api_url: String,
}

pub async fn run_signin(args: SigninArgs) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let endpoint = auth::graphql_endpoint(&args.api_url);
    let key_path = auth::expand_tilde(&args.key)?;

    let token = auth::signin_with_key(&client, &endpoint, &args.username, &key_path).await?;
    auth::persist_auth_state(&args.username, &key_path, &token).await?;

    println!("signed in as {}", args.username);
    Ok(())
}
