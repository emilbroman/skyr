use lsp::LspTransport;

pub async fn run_lsp() -> anyhow::Result<()> {
    let mut server = lsp::LanguageServer::new();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut transport = LspTransport::new(stdin, stdout);

    loop {
        let msg = transport.read_message().await?;
        let responses = server.handle(msg).await;
        for response in responses {
            transport.write_message(response).await?;
        }
    }
}
