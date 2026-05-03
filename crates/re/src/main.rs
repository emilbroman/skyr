//! Reporting Engine (RE) daemon.
//!
//! See `crates/re/README.md` and `STATUS_REPORTING.md` for the architectural
//! design. The binary spins up one or more worker tasks, each owning a
//! contiguous range of RQ shards (mirroring the RTQ/RTE topology). Each
//! worker runs:
//!
//! - A consumer loop draining the RQ delivery stream into the per-report
//!   pipeline.
//! - A watchdog task scanning the worker-local heartbeat cache and opening
//!   synthetic SystemError incidents for entities whose reports have gone
//!   stale.

use std::sync::Arc;

use clap::Parser;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::Instrument;

mod config;
mod entity;
mod pipeline;
mod thresholds;
mod watchdog;

use crate::config::WorkerConfig;
use crate::pipeline::{HeartbeatCache, PipelineContext};
use crate::thresholds::ThresholdTracker;

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "worker-index", default_value_t = 0)]
        worker_index: u16,

        #[clap(long = "worker-count", default_value_t = 1)]
        worker_count: u16,

        #[clap(long = "local-workers", default_value_t = 1)]
        local_workers: u16,

        /// Skyr region this RE serves (e.g. `stockholm`). Validated as
        /// `[a-z]+`. Used to resolve peer service addresses.
        #[clap(long = "region")]
        region: String,

        /// Template used to construct region-scoped Skyr peer service
        /// addresses. Substitutes `{service}` (required) and `{region}`
        /// (optional). Defaults to `{service}.{region}.int.skyr.cloud` —
        /// override per stack (e.g. `{service}.<namespace>.svc.cluster.local`
        /// for a single-region Kubernetes deployment).
        #[clap(long = "service-address-template", default_value_t = ids::ServiceAddressTemplate::default_template())]
        service_address_template: ids::ServiceAddressTemplate,
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
            worker_index,
            worker_count,
            local_workers,
            region,
            service_address_template,
        } => {
            let region: ids::RegionId = region
                .parse()
                .map_err(|e: ids::ParseIdError| anyhow::anyhow!("invalid --region: {e}"))?;
            let template = service_address_template;

            tracing::info!(%region, "starting reporting engine daemon");

            if local_workers == 0 {
                anyhow::bail!("--local-workers must be at least 1");
            }
            if worker_count == 0 {
                anyhow::bail!("--worker-count must be at least 1");
            }
            if worker_index >= worker_count {
                anyhow::bail!("--worker-index must be less than --worker-count");
            }
            if worker_index.saturating_add(local_workers) > worker_count {
                anyhow::bail!("--worker-index + --local-workers must be <= --worker-count");
            }

            let cfg = Arc::new(WorkerConfig::from_env());

            let rq_uri = format!("amqp://{}:5672/%2f", template.format("rq", &region));
            let nq_uri = format!("amqp://{}:5672/%2f", template.format("nq", &region));

            let sdb_client = sdb::ClientBuilder::new()
                .known_node(template.format("sdb", &region))
                .build()
                .await?;
            // Home-region RDB. RE writes the `resource_regions` routing
            // index here; it never reads or writes resource state itself.
            let rdb_client = rdb::ClientBuilder::new()
                .known_node(template.format("rdb", &region))
                .region(region.clone())
                .build()
                .await?;
            let nq_publisher = nq::ClientBuilder::new()
                .uri(nq_uri)
                .build_publisher()
                .await?;

            let mut handles: Vec<JoinHandle<()>> = Vec::new();

            for offset in 0..local_workers {
                let index = worker_index + offset;
                let worker_cfg = rq::WorkerConfig {
                    worker_index: index,
                    worker_count,
                };

                let consumer = rq::ClientBuilder::new()
                    .uri(rq_uri.clone())
                    .build_consumer(worker_cfg)
                    .await?;

                let span =
                    tracing::info_span!("worker", worker = %format!("{}/{}", index, worker_count));
                tracing::info!(
                    parent: &span,
                    shards = ?consumer.owned_shards(),
                    "started rq consumer",
                );

                let ctx = PipelineContext {
                    sdb: sdb_client.clone(),
                    nq: nq_publisher.clone(),
                    rdb: rdb_client.clone(),
                    thresholds: Arc::new(cfg.thresholds.clone()),
                    tracker: Arc::new(Mutex::new(ThresholdTracker::new())),
                    heartbeats: Arc::new(Mutex::new(HeartbeatCache::new())),
                };

                let watchdog_handle = watchdog::spawn(ctx.clone(), cfg.clone());
                handles.push(watchdog_handle);

                handles.push(tokio::spawn(worker_loop(consumer, ctx).instrument(span)));
            }

            for handle in handles {
                let _ = handle.await;
            }

            Ok(())
        }
    }
}

async fn worker_loop(mut consumer: rq::Consumer, ctx: PipelineContext) {
    loop {
        let keep_running = match worker_loop_iteration(&mut consumer, &ctx).await {
            Ok(keep_running) => keep_running,
            Err(error) => {
                tracing::error!(error = %error, "worker loop iteration failed");
                true
            }
        };

        if !keep_running {
            return;
        }
    }
}

async fn worker_loop_iteration(
    consumer: &mut rq::Consumer,
    ctx: &PipelineContext,
) -> anyhow::Result<bool> {
    let Some(delivery) = consumer.next().await? else {
        tracing::warn!("rq consumer stream closed");
        return Ok(false);
    };

    let report = &delivery.report;
    let entity_qid = report.entity_qid.as_string();
    tracing::debug!(
        entity_qid = %entity_qid,
        outcome = if report.outcome.is_success() { "success" } else { "failure" },
        redelivered = delivery.redelivered(),
        "received rq report",
    );

    match pipeline::process_report(ctx, report).await {
        Ok(()) => {
            delivery.ack().await?;
            Ok(true)
        }
        Err(error) => {
            tracing::warn!(
                entity_qid = %entity_qid,
                error = %error,
                "pipeline processing failed; nack-without-requeue",
            );
            // We always nack-without-requeue. SDB and NQ failures are
            // typically infra-side and warrant DLX-and-investigate rather
            // than hot-loop redelivery; per-report idempotency on the SDB
            // side ensures the next delivery (whether redelivered later or
            // not) reaches a coherent state.
            delivery.nack(false).await?;
            Ok(true)
        }
    }
}
