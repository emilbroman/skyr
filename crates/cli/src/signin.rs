use clap::Args;
use serde::Serialize;

use crate::{auth, output::OutputFormat};

#[derive(Args, Debug)]
pub struct SigninArgs {
    #[arg(long)]
    username: String,
    #[arg(long, default_value = "~/.ssh/id_ed25519")]
    key: String,
    #[arg(long, default_value = "localhost:8080")]
    api_url: String,
}

pub async fn run_signin(args: SigninArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let endpoint = auth::graphql_endpoint(&args.api_url);
    let key_path = auth::expand_tilde(&args.key)?;

    let token = auth::signin_with_key(&client, &endpoint, &args.username, &key_path).await?;
    auth::persist_auth_state(&args.username, &key_path, &token).await?;

    #[derive(Serialize)]
    struct SigninOutput {
        username: String,
        token: String,
    }

    let output = SigninOutput {
        username: args.username,
        token,
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
                String::from("token"),
                output.token,
            ]));
            print!("{table}");
        }
    }

    Ok(())
}
