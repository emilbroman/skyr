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

pub(crate) fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub(crate) fn table(format: &'static str) -> Table {
    Table::new(format)
}

pub(crate) fn row(columns: Vec<String>) -> Row {
    let mut row = Row::new();
    for value in columns {
        row.add_cell(value);
    }
    row
}

pub(crate) fn spawn_effect_printer(
    mut effects_rx: tokio::sync::mpsc::UnboundedReceiver<sclc::Effect>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        while let Some(effect) = effects_rx.recv().await {
            match effect {
                sclc::Effect::CreateResource { id, inputs, .. } => {
                    println!("CREATE {id} {inputs:?}");
                }
                sclc::Effect::UpdateResource { id, inputs, .. } => {
                    println!("UPDATE {id} {inputs:?}");
                }
                sclc::Effect::TouchResource { id, inputs, .. } => {
                    println!("TOUCH {id} {inputs:?}");
                }
            }
        }
    })
}

pub(crate) fn report_diagnostics<T>(diagnosed: sclc::Diagnosed<T>) -> Option<T> {
    for diag in diagnosed.diags().iter() {
        let (module_id, span) = diag.locate();
        println!("[{:?}] {module_id}:{span}: {diag}", diag.level());
    }

    if diagnosed.diags().has_errors() {
        return None;
    }

    Some(diagnosed.into_inner())
}

#[derive(Clone, Serialize)]
pub(crate) struct LogOutput {
    pub(crate) severity: String,
    pub(crate) timestamp: String,
    pub(crate) message: String,
}

/// Print groups of logs in text format, where each group has a label header.
pub(crate) fn print_logs_text(groups: &[(&str, &[LogOutput])]) {
    for (i, (label, logs)) in groups.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("==> {label} <==");
        for log in *logs {
            println!("[{}] [{}] {}", log.timestamp, log.severity, log.message);
        }
    }
}

pub(crate) fn shorten_commit_hash(hash: &str) -> String {
    hash.chars().take(16).collect::<String>()
}

pub(crate) fn format_created_at(created_at: &str) -> String {
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
