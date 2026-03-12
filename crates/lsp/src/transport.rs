use std::io;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::server::{IncomingMessage, OutgoingMessage};

/// JSON-RPC over stdio transport with LSP Content-Length framing.
pub struct LspTransport<R, W> {
    reader: BufReader<R>,
    writer: W,
}

impl<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin> LspTransport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }

    /// Read the next JSON-RPC message from the transport.
    pub async fn read_message(&mut self) -> io::Result<IncomingMessage> {
        let content_length = self.read_headers().await?;
        let mut body = vec![0u8; content_length];
        self.reader.read_exact(&mut body).await?;

        let msg: IncomingMessage = serde_json::from_slice(&body).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid JSON-RPC message: {err}"),
            )
        })?;

        Ok(msg)
    }

    /// Write a JSON-RPC message to the transport.
    pub async fn write_message(&mut self, msg: OutgoingMessage) -> io::Result<()> {
        let body = serde_json::to_string(&msg).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to serialize message: {err}"),
            )
        })?;

        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer.write_all(header.as_bytes()).await?;
        self.writer.write_all(body.as_bytes()).await?;
        self.writer.flush().await?;

        Ok(())
    }

    async fn read_headers(&mut self) -> io::Result<usize> {
        let mut content_length: Option<usize> = None;

        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).await?;

            if line.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Connection closed",
                ));
            }

            let line = line.trim();
            if line.is_empty() {
                // End of headers
                break;
            }

            if let Some(value) = line.strip_prefix("Content-Length: ") {
                content_length = Some(value.trim().parse().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Invalid Content-Length: {value}"),
                    )
                })?);
            }
            // Ignore other headers (e.g., Content-Type)
        }

        content_length.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Missing Content-Length header")
        })
    }
}
