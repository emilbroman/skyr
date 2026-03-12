use lsp::LspTransport;

pub async fn run_lsp() -> anyhow::Result<()> {
    eprintln!("scl language server starting");

    let mut server = lsp::LanguageServer::new();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut transport = LspTransport::new(stdin, stdout);

    loop {
        let msg = match transport.read_message().await {
            Ok(msg) => msg,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                eprintln!("client disconnected");
                return Ok(());
            }
            Err(err) => {
                eprintln!("failed to read message: {err}");
                return Err(err.into());
            }
        };

        let responses = server.handle(msg).await;
        for response in responses {
            if let Err(err) = transport.write_message(response).await {
                eprintln!("failed to write message: {err}");
                return Err(err.into());
            }
        }
    }
}
