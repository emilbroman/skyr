//! Container log streaming from CRI log files to LDB.
//!
//! CRI log format: `<rfc3339_timestamp> <stdout|stderr> <P|F> <message>`
//! - `P` = partial line (buffered until `F`)
//! - `F` = full line (or end of partial sequence)

use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

/// Stream container logs from a CRI log file to an LDB namespace publisher.
///
/// This function:
/// 1. Waits for the log file to appear (up to 30s)
/// 2. Tails the file incrementally
/// 3. Parses the CRI log format
/// 4. Detects log severity on a best-effort basis
/// 5. Publishes each log line to LDB, prefixed with `[{container_index}] `
///
/// All errors are logged via tracing and never cause a panic.
pub(crate) async fn stream_container_logs(
    log_path: PathBuf,
    publisher: ldb::NamespacePublisher,
    cancel: CancellationToken,
    container_index: usize,
) {
    // Wait for log file to appear
    let file = match wait_for_file(&log_path, &cancel).await {
        Some(f) => f,
        None => return,
    };

    let mut reader = BufReader::new(file);
    let mut line_buf = String::new();
    let mut partial_buf = String::new();

    loop {
        line_buf.clear();

        tokio::select! {
            _ = cancel.cancelled() => {
                // Flush any remaining partial buffer
                if !partial_buf.is_empty() {
                    let severity = detect_severity(&partial_buf, false);
                    let msg = format!("[{container_index}] {}", std::mem::take(&mut partial_buf));
                    publish(&publisher, severity, msg).await;
                }
                return;
            }
            result = reader.read_line(&mut line_buf) => {
                match result {
                    Ok(0) => {
                        // EOF — wait a bit and try again (file may still be written to)
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        continue;
                    }
                    Ok(_) => {
                        let line = line_buf.trim_end_matches('\n');
                        match parse_cri_log_line(line) {
                            Some((stream, tag, message)) => {
                                let is_stderr = stream == "stderr";
                                match tag {
                                    "P" => {
                                        partial_buf.push_str(message);
                                    }
                                    _ => {
                                        let full_message = if partial_buf.is_empty() {
                                            message.to_string()
                                        } else {
                                            partial_buf.push_str(message);
                                            std::mem::take(&mut partial_buf)
                                        };
                                        let severity = detect_severity(&full_message, is_stderr);
                                        let prefixed = format!("[{container_index}] {full_message}");
                                        publish(&publisher, severity, prefixed).await;
                                    }
                                }
                            }
                            None => {
                                // Non-CRI format line — publish as-is
                                if !line.is_empty() {
                                    let severity = detect_severity(line, false);
                                    let prefixed = format!("[{container_index}] {line}");
                                    publish(&publisher, severity, prefixed).await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %log_path.display(),
                            error = %e,
                            "error reading container log file"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
        }
    }
}

/// Wait for the log file to appear, retrying up to 30 seconds.
async fn wait_for_file(
    path: &std::path::Path,
    cancel: &CancellationToken,
) -> Option<tokio::fs::File> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);

    loop {
        match tokio::fs::File::open(path).await {
            Ok(file) => return Some(file),
            Err(_) => {
                if tokio::time::Instant::now() >= deadline {
                    tracing::warn!(
                        path = %path.display(),
                        "timed out waiting for container log file to appear"
                    );
                    return None;
                }

                tokio::select! {
                    _ = cancel.cancelled() => return None,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                }
            }
        }
    }
}

/// Parse a CRI log line into (stream, tag, message).
///
/// Format: `<timestamp> <stdout|stderr> <P|F> <message>`
fn parse_cri_log_line(line: &str) -> Option<(&str, &str, &str)> {
    // Skip timestamp (first space-delimited token)
    let rest = line.split_once(' ')?.1;
    // Extract stream
    let (stream, rest) = rest.split_once(' ')?;
    if stream != "stdout" && stream != "stderr" {
        return None;
    }
    // Extract tag
    let (tag, message) = rest.split_once(' ').unwrap_or((rest, ""));
    if tag != "P" && tag != "F" {
        return None;
    }
    Some((stream, tag, message))
}

/// Detect log severity on a best-effort basis.
fn detect_severity(message: &str, is_stderr: bool) -> ldb::Severity {
    // Check for structured logging patterns first
    let upper = message.to_uppercase();

    // Check for error-level indicators
    if upper.contains("ERROR")
        || upper.contains("ERR ")
        || upper.contains("FATAL")
        || upper.contains("PANIC")
        || contains_structured_level(message, "error")
        || contains_structured_level(message, "fatal")
        || contains_structured_level(message, "panic")
    {
        return ldb::Severity::Error;
    }

    // Check for warning-level indicators
    if upper.contains("WARN")
        || upper.contains("WARNING")
        || contains_structured_level(message, "warn")
        || contains_structured_level(message, "warning")
    {
        return ldb::Severity::Warning;
    }

    // stderr without explicit level gets Warning
    if is_stderr {
        return ldb::Severity::Warning;
    }

    ldb::Severity::Info
}

/// Check for structured logging level patterns like `level=error` or `"level":"error"`.
fn contains_structured_level(message: &str, level: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains(&format!("level={level}"))
        || lower.contains(&format!("\"level\":\"{level}\""))
        || lower.contains(&format!("level=\"{level}\""))
}

async fn publish(publisher: &ldb::NamespacePublisher, severity: ldb::Severity, message: String) {
    if let Err(e) = publisher.log(severity, message).await {
        tracing::warn!(error = %e, "failed to publish container log to LDB");
    }
}
