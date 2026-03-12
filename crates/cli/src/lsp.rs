use std::io::{BufRead, Write};
use std::path::PathBuf;

use lsp::{IncomingMessage, LanguageServer, OutgoingMessage};
use sclc::ModuleId;

use crate::fs_source::FsSource;

pub async fn run_lsp() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdin = stdin.lock();
    let mut stdout = stdout.lock();

    let mut server = LanguageServer::new(|| FsSource {
        root: PathBuf::from("."),
        package_id: ModuleId::from(["Local", "Local"]),
    });

    loop {
        let msg = match read_message(&mut stdin)? {
            Some(msg) => msg,
            None => break,
        };

        let responses = server.handle(msg).await;

        for response in responses {
            write_message(&mut stdout, &response)?;
        }

        if server.should_exit() {
            break;
        }
    }

    std::process::exit(server.exit_code());
}

fn read_message(stdin: &mut impl BufRead) -> anyhow::Result<Option<IncomingMessage>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = stdin.read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(None);
        }

        let line = line.trim();
        if line.is_empty() {
            break;
        }

        if let Some(value) = line.strip_prefix("Content-Length: ") {
            content_length = Some(value.parse()?);
        }
    }

    let content_length = content_length.ok_or_else(|| anyhow::anyhow!("Missing Content-Length"))?;

    let mut content = vec![0u8; content_length];
    stdin.read_exact(&mut content)?;

    let json = String::from_utf8(content)?;
    let msg = IncomingMessage::parse(&json)?;
    Ok(Some(msg))
}

fn write_message(stdout: &mut impl Write, msg: &OutgoingMessage) -> anyhow::Result<()> {
    let json = serde_json::to_string(msg)?;
    write!(stdout, "Content-Length: {}\r\n\r\n{}", json.len(), json)?;
    stdout.flush()?;
    Ok(())
}
