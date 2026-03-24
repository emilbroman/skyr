use std::io;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::server::{IncomingMessage, OutgoingMessage};

/// Maximum allowed Content-Length value (64 MiB).
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Maximum length of a single header line (8 KiB).
const MAX_HEADER_LINE_LENGTH: usize = 8 * 1024;

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

            if line.len() > MAX_HEADER_LINE_LENGTH {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Header line too long ({} bytes, max {})",
                        line.len(),
                        MAX_HEADER_LINE_LENGTH
                    ),
                ));
            }

            let line = line.trim();
            if line.is_empty() {
                // End of headers
                break;
            }

            // Case-insensitive header matching per HTTP/LSP spec
            if let Some((key, value)) = line.split_once(':')
                && key.trim().eq_ignore_ascii_case("content-length")
            {
                let length: usize = value.trim().parse().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Invalid Content-Length: {}", value.trim()),
                    )
                })?;
                if length > MAX_MESSAGE_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Content-Length {length} exceeds maximum allowed size \
                             ({MAX_MESSAGE_SIZE})"
                        ),
                    ));
                }
                content_length = Some(length);
            }
            // Ignore other headers (e.g., Content-Type)
        }

        content_length.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Missing Content-Length header")
        })
    }
}
