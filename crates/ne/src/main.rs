use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use ne::{
    batcher::{Batcher, BatcherConfig},
    dedup::{DedupConfig, DedupStore},
    process_delivery,
    sender::{SmtpConfig, SmtpSender, SmtpTls},
};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;
use tokio::task::JoinSet;

#[derive(Parser)]
enum Program {
    /// Run the Notification Engine daemon: consume NQ and dispatch e-mails.
    Daemon {
        // ---- region & service-address template ---------------------------
        /// Skyr region this NE serves (e.g. `stockholm`). Validated as
        /// `[a-z]+`. Used to resolve peer service addresses (NQ, UDB).
        #[clap(long = "region")]
        region: String,

        /// Template used to construct region-scoped Skyr peer service
        /// addresses. Substitutes `{service}` (required) and `{region}`
        /// (optional). Defaults to `{service}.{region}.int.skyr.cloud` —
        /// override per stack (e.g. `{service}.<namespace>.svc.cluster.local`
        /// for a single-region Kubernetes deployment).
        #[clap(long = "service-address-template", default_value_t = ids::ServiceAddressTemplate::default_template())]
        service_address_template: ids::ServiceAddressTemplate,

        // ---- queue ---------------------------------------------------------
        /// Override the full AMQP URI instead of resolving the NQ broker
        /// from `--region` and `--service-address-template`. Useful for
        /// managed RabbitMQ deployments with TLS, custom vhosts, or
        /// credentials.
        #[clap(long = "nq-uri")]
        nq_uri: Option<String>,

        /// AMQP basic.qos prefetch count for each NE worker.
        #[clap(long = "prefetch", default_value_t = 4)]
        prefetch: u16,

        /// Optional dead-letter exchange to attach to the NQ queue. Operations
        /// declares the exchange and any DLQ separately.
        #[clap(long = "nq-dlx")]
        nq_dlx: Option<String>,

        /// Optional dead-letter routing key. Only meaningful with `--nq-dlx`.
        #[clap(long = "nq-dlx-routing-key")]
        nq_dlx_routing_key: Option<String>,

        /// Number of concurrent worker tasks. Each pulls from the same NQ
        /// queue under competing-consumer semantics. Defaults to 1.
        #[clap(long = "worker-count", default_value_t = 1)]
        worker_count: u16,

        // ---- dedup ---------------------------------------------------------
        /// Hostname of the Redis used to keep idempotency-key claims so
        /// at-least-once redeliveries do not produce duplicate e-mails.
        #[clap(long = "dedup-hostname", default_value = "localhost")]
        dedup_hostname: String,

        /// TTL (seconds) on each dedup claim. Should comfortably exceed the
        /// longest plausible queue dwell time.
        #[clap(long = "dedup-ttl-seconds", default_value_t = 7 * 24 * 60 * 60)]
        dedup_ttl_seconds: u64,

        // ---- smtp ----------------------------------------------------------
        /// SMTP server hostname.
        #[clap(long = "smtp-host")]
        smtp_host: String,

        /// SMTP server port. Defaults to 587 (submission / STARTTLS).
        #[clap(long = "smtp-port", default_value_t = 587)]
        smtp_port: u16,

        /// SMTP transport security mode: `starttls` (default), `tls`, or `none`.
        #[clap(long = "smtp-tls", default_value = "starttls")]
        smtp_tls: SmtpTls,

        /// SMTP AUTH username. If unset, the connection is unauthenticated.
        #[clap(long = "smtp-username", env = "NE_SMTP_USERNAME")]
        smtp_username: Option<String>,

        /// SMTP AUTH password. Read from `--smtp-password` or the
        /// `NE_SMTP_PASSWORD` environment variable.
        #[clap(
            long = "smtp-password",
            env = "NE_SMTP_PASSWORD",
            hide_env_values = true
        )]
        smtp_password: Option<String>,

        /// Sender address used in the SMTP envelope and the `From:` header.
        /// Must parse as a valid mailbox, e.g. `Skyr <skyr@example.com>` or
        /// `noreply@example.com`.
        #[clap(long = "smtp-from")]
        smtp_from: String,

        /// SMTP connection timeout in seconds.
        #[clap(long = "smtp-timeout-seconds", default_value_t = 30)]
        smtp_timeout_seconds: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match Program::parse() {
        Program::Daemon {
            region,
            service_address_template,
            nq_uri,
            prefetch,
            nq_dlx,
            nq_dlx_routing_key,
            worker_count,
            dedup_hostname,
            dedup_ttl_seconds,
            smtp_host,
            smtp_port,
            smtp_tls,
            smtp_username,
            smtp_password,
            smtp_from,
            smtp_timeout_seconds,
        } => {
            if worker_count == 0 {
                anyhow::bail!("--worker-count must be at least 1");
            }

            let region: ids::RegionId = region
                .parse()
                .map_err(|e: ids::ParseIdError| anyhow::anyhow!("invalid --region: {e}"))?;
            let template = service_address_template;

            let uri = nq_uri
                .unwrap_or_else(|| format!("amqp://{}:5672/%2f", template.format("nq", &region)));

            tracing::info!(
                worker_count,
                %region,
                smtp_host = %smtp_host,
                smtp_port,
                tls = ?smtp_tls,
                "starting notification engine daemon",
            );

            let dedup = DedupStore::connect(&DedupConfig {
                hostname: dedup_hostname,
                ttl_seconds: dedup_ttl_seconds,
            })
            .await?;

            let udb_client = udb::ClientBuilder::new()
                .known_node(template.format("udb", &region))
                .build()
                .await?;

            // Treat an empty username/password — produced when the
            // NE_SMTP_USERNAME / NE_SMTP_PASSWORD env vars are set to ""
            // (e.g., by the k8s SMTP-relay-fallback path) — as "no
            // SASL auth" rather than as an empty-string credential.
            let smtp_username = smtp_username.filter(|s| !s.is_empty());
            let smtp_password = smtp_password.filter(|s| !s.is_empty());

            let smtp_config = SmtpConfig {
                host: smtp_host,
                port: smtp_port,
                tls: smtp_tls,
                username: smtp_username,
                password: smtp_password,
                from: smtp_from,
                timeout: Duration::from_secs(smtp_timeout_seconds),
            };
            let sender = Arc::new(SmtpSender::build(&smtp_config)?);

            // Per-recipient batcher: coalesces notification e-mails so a
            // sustained surge for one recipient produces at most one
            // e-mail per batch window. The owning `Batcher` lives on the
            // main task; workers receive cheap `BatcherHandle` clones.
            let batcher = Batcher::start(BatcherConfig::default(), sender);

            // Shutdown signalling: workers listen on this; the OS signal
            // task fires it on SIGTERM / SIGINT.
            let shutdown = Arc::new(Notify::new());
            spawn_signal_listener(shutdown.clone());

            let mut tasks: JoinSet<anyhow::Result<()>> = JoinSet::new();
            for worker_index in 0..worker_count {
                let dedup = dedup.clone();
                let udb_client = udb_client.clone();
                let batcher_handle = batcher.handle();
                let shutdown = shutdown.clone();
                let uri = uri.clone();
                let dlx = nq_dlx.clone();
                let dlx_rk = nq_dlx_routing_key.clone();

                tasks.spawn(async move {
                    let mut builder = nq::ClientBuilder::new().uri(uri).prefetch(prefetch);
                    if let Some(dlx) = dlx {
                        builder = builder.dead_letter_exchange(dlx);
                    }
                    if let Some(dlx_rk) = dlx_rk {
                        builder = builder.dead_letter_routing_key(dlx_rk);
                    }
                    let mut consumer = builder.build_consumer().await?;

                    tracing::info!(worker_index, "ne worker ready");
                    loop {
                        tokio::select! {
                            next = consumer.next() => match next {
                                Ok(Some(delivery)) => {
                                    let outcome = process_delivery(
                                        delivery,
                                        &dedup,
                                        &udb_client,
                                        &batcher_handle,
                                    )
                                    .await;
                                    match outcome {
                                        Ok(o) => tracing::debug!(?o, "processed delivery"),
                                        Err(error) => {
                                            tracing::warn!(error = %error, "ack/nack failed");
                                        }
                                    }
                                }
                                Ok(None) => {
                                    tracing::warn!(worker_index, "nq consumer stream closed");
                                    return Ok(());
                                }
                                Err(error) => {
                                    tracing::error!(
                                        worker_index,
                                        error = %error,
                                        "nq consumer error",
                                    );
                                    return Err(error.into());
                                }
                            },
                            _ = shutdown.notified() => {
                                tracing::info!(worker_index, "ne worker shutting down");
                                return Ok(());
                            }
                        }
                    }
                });
            }

            // Wait for either an OS signal (which the listener task
            // converts into a `shutdown.notify_waiters()`) or any worker
            // exiting on its own. In the latter case we treat the exit
            // as a shutdown trigger and tear the rest of the workers
            // down gracefully so the batcher can flush.
            tokio::select! {
                _ = shutdown.notified() => {
                    tracing::info!("shutdown notified; draining workers");
                }
                Some(result) = tasks.join_next() => {
                    match result {
                        Ok(Ok(())) => tracing::warn!("a worker exited; draining the rest"),
                        Ok(Err(error)) => tracing::error!(error = %error, "worker error; draining the rest"),
                        Err(error) => tracing::error!(error = %error, "worker panicked; draining the rest"),
                    }
                }
            }

            // Tell every worker still running that it's time to stop.
            // Workers acknowledge by exiting their `select!` loop and
            // dropping their `BatcherHandle` clones; that lets the
            // subsequent `batcher.shutdown()` complete cleanly.
            shutdown.notify_waiters();

            let mut first_error: Option<anyhow::Error> = None;
            while let Some(result) = tasks.join_next().await {
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "worker task panic during shutdown");
                    }
                }
            }

            // All BatcherHandle clones are dropped now (the workers held
            // them and have exited). Drain every in-flight batch and
            // wait for the flusher task to finish its SMTP work.
            tracing::info!("draining notification batcher before exit");
            batcher.shutdown().await;

            match first_error {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }
}

/// Spawns a task that listens for SIGTERM and SIGINT and notifies all
/// waiters on `shutdown` when either fires. SIGTERM is what
/// orchestrators (systemd, Kubernetes) use for graceful termination;
/// SIGINT is the developer-facing Ctrl+C.
fn spawn_signal_listener(shutdown: Arc<Notify>) {
    tokio::spawn(async move {
        let term = signal(SignalKind::terminate());
        let int = signal(SignalKind::interrupt());
        let (mut term, mut int) = match (term, int) {
            (Ok(t), Ok(i)) => (t, i),
            (Err(error), _) | (_, Err(error)) => {
                tracing::error!(error = %error, "failed to install OS signal handlers");
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => tracing::info!("received SIGTERM"),
            _ = int.recv() => tracing::info!("received SIGINT"),
        }
        shutdown.notify_waiters();
    });
}
