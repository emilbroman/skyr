use chrono::{DateTime, Local, Utc};
use clap::ValueEnum;
use serde::Serialize;
use tabular::{Row, Table};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum OutputFormat {
    Json,
    #[default]
    Text,
}

pub fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn table(format: &'static str) -> Table {
    Table::new(format)
}

pub fn row(columns: Vec<String>) -> Row {
    let mut row = Row::new();
    for value in columns {
        row.add_cell(value);
    }
    row
}

pub fn shorten_commit_hash(hash: &str) -> String {
    hash.chars().take(16).collect::<String>()
}

pub fn format_created_at(created_at: &str) -> String {
    let parsed_utc = match DateTime::parse_from_rfc3339(created_at) {
        Ok(parsed) => parsed.with_timezone(&Utc),
        Err(_) => return created_at.to_owned(),
    };
    let parsed_local = parsed_utc.with_timezone(&Local);

    let now = Local::now();
    let diff = now.signed_duration_since(parsed_local);

    if diff.num_seconds() >= 0 && diff.num_seconds() < 60 {
        return format!("{}s ago", diff.num_seconds());
    }

    if diff.num_seconds() >= 60 && diff.num_minutes() < 60 {
        return format!("{}m ago", diff.num_minutes());
    }

    if diff.num_hours() < 24 {
        return parsed_local.format("%H:%M:%S").to_string();
    }

    parsed_local.format("%Y-%m-%d %H:%M").to_string()
}
