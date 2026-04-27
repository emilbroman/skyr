//! Direct GraphQL access via `skyr api query` and `skyr api mut`.
//!
//! The body passed by the user is just the selection-set portion of a
//! GraphQL operation (`{ ... }`). The CLI prepends
//! `query SkyrCli(<varDefs>)` (or `mutation SkyrCli(<varDefs>)`) by reading
//! variable definitions out of `--arg` flags. See `parse_arg_spec` for the
//! supported `--arg` syntax.

use std::io::Read;

use anyhow::{Context as _, anyhow, bail};
use clap::{Args, Subcommand};
use serde_json::json;

use crate::{auth, context::Context, output::OutputFormat};

#[derive(Args, Debug)]
pub struct ApiArgs {
    #[command(subcommand)]
    command: ApiCommand,
}

#[derive(Subcommand, Debug)]
enum ApiCommand {
    /// Send a GraphQL query (read-only).
    Query(ApiOpArgs),
    /// Send a GraphQL mutation (state-changing).
    Mut(ApiOpArgs),
}

#[derive(Args, Debug)]
struct ApiOpArgs {
    /// Selection-set body of the operation, e.g. `{ me { username } }`.
    /// Mutually exclusive with `--from-file`.
    body: Option<String>,
    /// Read the selection-set body from a file (use `-` for stdin).
    #[arg(long, value_name = "PATH", conflicts_with = "body")]
    from_file: Option<String>,
    /// Variable specs in the form `<name>(:<type>)?(=<value>)?`. May be
    /// repeated. See module docs for the resolution rules.
    #[arg(long = "arg", value_name = "SPEC")]
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct ArgSpec {
    name: String,
    graphql_type: String,
    value: serde_json::Value,
}

pub async fn run_api(args: ApiArgs, ctx: &Context) -> anyhow::Result<()> {
    let (op_keyword, op) = match args.command {
        ApiCommand::Query(op) => ("query", op),
        ApiCommand::Mut(op) => ("mutation", op),
    };
    let body = read_body(&op)?;
    let specs: Vec<ArgSpec> = op
        .args
        .iter()
        .map(|raw| parse_arg_spec(raw))
        .collect::<anyhow::Result<_>>()?;

    let query = build_query(op_keyword, body.trim(), &specs);
    let mut variables = serde_json::Map::with_capacity(specs.len());
    for spec in &specs {
        variables.insert(spec.name.clone(), spec.value.clone());
    }

    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, ctx.api_url()).await?;
    let endpoint = auth::graphql_endpoint(ctx.api_url());

    let payload = json!({
        "query": query,
        "variables": serde_json::Value::Object(variables),
        "operationName": "SkyrCli",
    });

    let response = client
        .post(&endpoint)
        .header(
            reqwest::header::AUTHORIZATION,
            auth::bearer_header_value(&token)?,
        )
        .json(&payload)
        .send()
        .await
        .context("failed to send api request")?;
    let response: graphql_client::Response<serde_json::Value> = response
        .json()
        .await
        .context("failed to decode api response")?;
    let data = auth::graphql_response_data(response, "api request")?;

    match ctx.format {
        OutputFormat::Json => println!("{}", serde_json::to_string(&data)?),
        OutputFormat::Text => println!("{}", serde_json::to_string_pretty(&data)?),
    }
    Ok(())
}

fn read_body(op: &ApiOpArgs) -> anyhow::Result<String> {
    match (&op.body, &op.from_file) {
        (Some(b), None) => Ok(b.clone()),
        (None, Some(path)) => {
            if path == "-" {
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .context("failed to read selection-set body from stdin")?;
                Ok(buf)
            } else {
                std::fs::read_to_string(path)
                    .with_context(|| format!("failed to read selection-set body from {path}"))
            }
        }
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts_with"),
        (None, None) => bail!("provide a selection-set body, or --from-file PATH"),
    }
}

fn build_query(op_keyword: &str, body: &str, specs: &[ArgSpec]) -> String {
    if specs.is_empty() {
        format!("{op_keyword} SkyrCli {body}")
    } else {
        let var_defs = specs
            .iter()
            .map(|s| format!("${}: {}", s.name, s.graphql_type))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{op_keyword} SkyrCli({var_defs}) {body}")
    }
}

fn parse_arg_spec(raw: &str) -> anyhow::Result<ArgSpec> {
    let (name, type_part, value_part) = split_spec(raw);
    if name.is_empty() {
        bail!("--arg spec `{raw}` is missing a variable name");
    }
    if !is_valid_var_name(name) {
        bail!(
            "--arg `{raw}`: variable name `{name}` must be alphanumeric or underscore, \
             starting with a letter or underscore"
        );
    }

    let json_value = match (type_part, value_part) {
        (_, None) => serde_json::Value::Bool(true),
        (Some(t), Some(v)) => {
            let stripped = t.trim_end_matches('!');
            if stripped == "String" {
                serde_json::Value::String(v.to_owned())
            } else {
                serde_json::from_str::<serde_json::Value>(v).with_context(|| {
                    format!("--arg `{name}` value `{v}` is not valid JSON for type `{t}`")
                })?
            }
        }
        (None, Some(v)) => serde_json::from_str::<serde_json::Value>(v)
            .unwrap_or_else(|_| serde_json::Value::String(v.to_owned())),
    };

    let raw_type = match type_part {
        Some(t) => t.to_owned(),
        None => derive_type(&json_value).ok_or_else(|| {
            anyhow!("--arg `{name}`: cannot infer type for `null`; pass `:<Type>` explicitly")
        })?,
    };

    let graphql_type = if matches!(json_value, serde_json::Value::Null) {
        raw_type.trim_end_matches('!').to_owned()
    } else if raw_type.ends_with('!') {
        raw_type
    } else {
        format!("{raw_type}!")
    };

    Ok(ArgSpec {
        name: name.to_owned(),
        graphql_type,
        value: json_value,
    })
}

/// Walk the spec once, splitting on the first `:` (separates name and type)
/// and the first `=` (separates type/name and value).
fn split_spec(raw: &str) -> (&str, Option<&str>, Option<&str>) {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b':' && bytes[i] != b'=' {
        i += 1;
    }
    let name = &raw[..i];
    if i == raw.len() {
        return (name, None, None);
    }
    if bytes[i] == b'=' {
        return (name, None, Some(&raw[i + 1..]));
    }
    // bytes[i] == b':'
    let after = &raw[i + 1..];
    let after_bytes = after.as_bytes();
    let mut j = 0;
    while j < after_bytes.len() && after_bytes[j] != b'=' {
        j += 1;
    }
    let type_part = &after[..j];
    if j == after.len() {
        (name, Some(type_part), None)
    } else {
        (name, Some(type_part), Some(&after[j + 1..]))
    }
}

fn derive_type(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(_) => Some("Boolean".into()),
        serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => Some("Int".into()),
        serde_json::Value::Number(_) => Some("Float".into()),
        serde_json::Value::String(_) => Some("String".into()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Some("JSON".into()),
    }
}

fn is_valid_var_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(raw: &str) -> ArgSpec {
        parse_arg_spec(raw).unwrap_or_else(|e| panic!("expected `{raw}` to parse: {e}"))
    }

    #[test]
    fn type_omitted_value_omitted() {
        let s = spec("enabled");
        assert_eq!(s.graphql_type, "Boolean!");
        assert_eq!(s.value, json!(true));
    }

    #[test]
    fn type_omitted_value_string() {
        let s = spec("name=alice");
        assert_eq!(s.graphql_type, "String!");
        assert_eq!(s.value, json!("alice"));
    }

    #[test]
    fn type_omitted_value_int() {
        let s = spec("count=42");
        assert_eq!(s.graphql_type, "Int!");
        assert_eq!(s.value, json!(42));
    }

    #[test]
    fn type_omitted_value_float() {
        let s = spec("ratio=1.5");
        assert_eq!(s.graphql_type, "Float!");
        assert_eq!(s.value, json!(1.5));
    }

    #[test]
    fn type_omitted_value_object() {
        let s = spec(r#"filter={"k":1}"#);
        assert_eq!(s.graphql_type, "JSON!");
        assert_eq!(s.value, json!({"k": 1}));
    }

    #[test]
    fn type_omitted_value_array() {
        let s = spec(r#"tags=["a","b"]"#);
        assert_eq!(s.graphql_type, "JSON!");
        assert_eq!(s.value, json!(["a", "b"]));
    }

    #[test]
    fn type_omitted_value_null_errors() {
        let err = parse_arg_spec("missing=null").unwrap_err();
        assert!(err.to_string().contains("cannot infer type for `null`"));
    }

    #[test]
    fn type_explicit_input_object() {
        let s = spec(r#"input:UserInput={"name":"x"}"#);
        assert_eq!(s.graphql_type, "UserInput!");
        assert_eq!(s.value, json!({"name": "x"}));
    }

    #[test]
    fn type_explicit_scalar_value_omitted() {
        let s = spec("tag:Tag");
        assert_eq!(s.graphql_type, "Tag!");
        assert_eq!(s.value, json!(true));
    }

    #[test]
    fn type_explicit_string_coerces() {
        let s = spec("name:String=42");
        assert_eq!(s.graphql_type, "String!");
        assert_eq!(s.value, json!("42"));
    }

    #[test]
    fn type_explicit_string_bang_coerces() {
        let s = spec("name:String!=42");
        assert_eq!(s.graphql_type, "String!");
        assert_eq!(s.value, json!("42"));
    }

    #[test]
    fn type_explicit_int_no_double_bang() {
        let s = spec("count:Int!=5");
        assert_eq!(s.graphql_type, "Int!");
        assert_eq!(s.value, json!(5));
    }

    #[test]
    fn null_value_drops_bang() {
        let s = spec("parent:User=null");
        assert_eq!(s.graphql_type, "User");
        assert_eq!(s.value, json!(null));
    }

    #[test]
    fn null_value_drops_bang_even_when_user_wrote_one() {
        let s = spec("parent:User!=null");
        assert_eq!(s.graphql_type, "User");
        assert_eq!(s.value, json!(null));
    }

    #[test]
    fn rejects_invalid_name() {
        assert!(parse_arg_spec("=value").is_err());
        assert!(parse_arg_spec("1bad=value").is_err());
        assert!(parse_arg_spec("ba-d=value").is_err());
    }

    #[test]
    fn rejects_invalid_json_with_explicit_type() {
        let err = parse_arg_spec("count:Int=notanumber").unwrap_err();
        assert!(err.to_string().contains("not valid JSON"));
    }

    #[test]
    fn build_query_no_args() {
        let q = build_query("query", "{ me { username } }", &[]);
        assert_eq!(q, "query SkyrCli { me { username } }");
    }

    #[test]
    fn build_query_with_args() {
        let specs = vec![
            ArgSpec {
                name: "name".into(),
                graphql_type: "String!".into(),
                value: json!("alice"),
            },
            ArgSpec {
                name: "count".into(),
                graphql_type: "Int!".into(),
                value: json!(3),
            },
        ];
        let q = build_query(
            "mutation",
            "{ doThing(name: $name, count: $count) }",
            &specs,
        );
        assert_eq!(
            q,
            "mutation SkyrCli($name: String!, $count: Int!) \
             { doThing(name: $name, count: $count) }"
        );
    }
}
