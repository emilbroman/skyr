//! SMTP sender — the only Skyr component that holds SMTP credentials.
//!
//! The transport is `lettre`'s async tokio + rustls SMTP client. We deliberately
//! keep the abstraction layer thin: the sender takes a parsed configuration at
//! startup and sends one [`Message`] per call.
//!
//! # TLS modes
//!
//! Three transport flavours are supported:
//!
//! - [`SmtpTls::StartTls`] — connect on the plaintext port, then upgrade to TLS
//!   via `STARTTLS`. The default; matches port 587 ("submission").
//! - [`SmtpTls::ImplicitTls`] — connect on a TLS-from-the-start port, typically
//!   465.
//! - [`SmtpTls::None`] — no TLS at all. Use only for local development.
//!
//! # Errors
//!
//! Lettre distinguishes permanent (5XX) and transient (4XX) SMTP errors via
//! [`lettre::transport::smtp::Error`]. We expose a [`SendOutcome`] indicating
//! which class the failure was in so the caller can decide whether to retry
//! (transient → release dedup claim and nack-with-requeue) or drop (permanent
//! → leave dedup claim, nack-without-requeue, let the broker DLX it).

use std::time::Duration;

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use thiserror::Error;

use crate::render::RenderedEmail;

/// TLS mode selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// Plain TCP, then upgrade via `STARTTLS`. Matches port 587.
    StartTls,
    /// Implicit TLS from the first byte. Matches port 465.
    ImplicitTls,
    /// No TLS. Local development only.
    None,
}

impl std::str::FromStr for SmtpTls {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "starttls" | "start-tls" => Ok(SmtpTls::StartTls),
            "tls" | "implicit" | "implicit-tls" => Ok(SmtpTls::ImplicitTls),
            "none" | "plain" | "plaintext" => Ok(SmtpTls::None),
            other => Err(format!(
                "invalid SMTP TLS mode `{other}`: expected one of starttls, tls, none",
            )),
        }
    }
}

/// Configuration for the SMTP sender.
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    /// Optional username for SMTP AUTH. If `None`, the connection is not
    /// authenticated.
    pub username: Option<String>,
    /// Optional password for SMTP AUTH. Only consulted when `username` is set.
    pub password: Option<String>,
    /// The address that appears in the `From:` header and the SMTP envelope.
    /// Must parse as a valid mailbox.
    pub from: String,
    /// Connection timeout for the underlying SMTP transport.
    pub timeout: Duration,
}

impl SmtpConfig {
    pub fn parsed_from(&self) -> Result<Mailbox, SendError> {
        self.from
            .parse::<Mailbox>()
            .map_err(|e| SendError::InvalidFromAddress {
                address: self.from.clone(),
                reason: e.to_string(),
            })
    }
}

#[derive(Debug, Error)]
pub enum SendError {
    #[error("invalid `from` address {address:?}: {reason}")]
    InvalidFromAddress { address: String, reason: String },

    #[error("invalid recipient address {address:?}: {reason}")]
    InvalidRecipientAddress { address: String, reason: String },

    #[error("failed to build email message: {0}")]
    Build(String),

    #[error("smtp transport setup failed: {0}")]
    TransportSetup(String),

    #[error("smtp send failed (transient): {0}")]
    Transient(String),

    #[error("smtp send failed (permanent): {0}")]
    Permanent(String),
}

impl SendError {
    /// Whether the underlying SMTP error class is transient (worth retrying)
    /// rather than permanent (a configuration or data problem).
    pub fn is_transient(&self) -> bool {
        matches!(self, SendError::Transient(_) | SendError::TransportSetup(_))
    }
}

/// SMTP sender. Holds an open transport ready to dispatch [`Message`]s.
pub struct SmtpSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpSender {
    /// Builds an SMTP transport from configuration. The transport is lazy —
    /// the underlying connection is established per-send by `lettre`.
    pub fn build(config: &SmtpConfig) -> Result<Self, SendError> {
        let from = config.parsed_from()?;

        let mut builder = match config.tls {
            SmtpTls::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
                .map_err(|e| SendError::TransportSetup(e.to_string()))?,
            SmtpTls::ImplicitTls => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
                .map_err(|e| SendError::TransportSetup(e.to_string()))?,
            SmtpTls::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host),
        };

        builder = builder.port(config.port).timeout(Some(config.timeout));

        if let Some(username) = &config.username {
            let password = config.password.clone().unwrap_or_default();
            builder = builder.credentials(Credentials::new(username.clone(), password));
        }

        Ok(Self {
            transport: builder.build(),
            from,
        })
    }

    /// Sends a rendered email to the given recipients. Returns `Ok(())` on
    /// success; errors are classified as permanent or transient via
    /// [`SendError::is_transient`].
    ///
    /// All recipients receive the same body in a single envelope. v1 has no
    /// per-recipient personalization.
    pub async fn send(
        &self,
        email: &RenderedEmail,
        recipients: &[String],
    ) -> Result<(), SendError> {
        if recipients.is_empty() {
            // Nothing to send. The caller is expected to short-circuit before
            // reaching here; we treat this as a no-op to make the function
            // robust.
            return Ok(());
        }

        let mut builder = Message::builder()
            .from(self.from.clone())
            .subject(email.subject.clone());

        for address in recipients {
            let mailbox: Mailbox =
                address
                    .parse()
                    .map_err(|e: lettre::address::AddressError| {
                        SendError::InvalidRecipientAddress {
                            address: address.clone(),
                            reason: e.to_string(),
                        }
                    })?;
            builder = builder.to(mailbox);
        }

        let message = builder
            .header(ContentType::TEXT_PLAIN)
            .body(email.body.clone())
            .map_err(|e| SendError::Build(e.to_string()))?;

        match self.transport.send(message).await {
            Ok(_) => Ok(()),
            Err(err) => Err(classify_smtp_error(err)),
        }
    }
}

fn classify_smtp_error(err: lettre::transport::smtp::Error) -> SendError {
    if err.is_permanent() {
        SendError::Permanent(err.to_string())
    } else {
        // Lettre marks I/O, TLS, and 4XX server errors as non-permanent. Treat
        // them all as transient; the caller will release the dedup claim and
        // nack with requeue.
        SendError::Transient(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_smtp_tls_modes() {
        assert_eq!("starttls".parse::<SmtpTls>().unwrap(), SmtpTls::StartTls);
        assert_eq!("start-tls".parse::<SmtpTls>().unwrap(), SmtpTls::StartTls);
        assert_eq!("STARTTLS".parse::<SmtpTls>().unwrap(), SmtpTls::StartTls);
        assert_eq!("tls".parse::<SmtpTls>().unwrap(), SmtpTls::ImplicitTls);
        assert_eq!("implicit".parse::<SmtpTls>().unwrap(), SmtpTls::ImplicitTls);
        assert_eq!("none".parse::<SmtpTls>().unwrap(), SmtpTls::None);
        assert!("nope".parse::<SmtpTls>().is_err());
    }

    #[test]
    fn from_address_must_parse() {
        let cfg = SmtpConfig {
            host: "smtp.example.com".into(),
            port: 587,
            tls: SmtpTls::StartTls,
            username: None,
            password: None,
            from: "not-an-email".into(),
            timeout: Duration::from_secs(30),
        };
        assert!(cfg.parsed_from().is_err());

        let mut cfg = cfg;
        cfg.from = "Skyr <skyr@example.com>".into();
        assert!(cfg.parsed_from().is_ok());
    }
}
