mod backoff;
mod finder;
mod util;
mod worker;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use clap::Parser;
use futures_util::StreamExt;
use tokio::{
    sync::oneshot,
    task,
    time::{Instant, sleep_until},
};
use tracing::Instrument;

use worker::Worker;

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "cdb-hostname", default_value = "localhost")]
        cdb_hostname: String,

        #[clap(long = "rdb-hostname", default_value = "localhost")]
        rdb_hostname: String,

        #[clap(long = "rtq-hostname", default_value = "localhost")]
        rtq_hostname: String,

        #[clap(long = "ldb-hostname", default_value = "localhost")]
        ldb_hostname: String,

        #[clap(long = "worker-index", default_value_t = 0)]
        worker_index: u16,

        #[clap(long = "worker-count", default_value_t = 1)]
        worker_count: u16,
    },
}

/// Determines whether the given deployment is owned by this worker.
///
/// Interprets the first 16 hex characters of the deployment (commit hash) ID
/// as a big-endian u64 and assigns it to a worker via modulo division.
/// When `worker_count` is 1 every deployment is owned.
fn deployment_owned_by_worker(
    deployment_id: &ids::DeploymentId,
    worker_index: u16,
    worker_count: u16,
) -> bool {
    if worker_count <= 1 {
        return true;
    }
    let hex_prefix = &deployment_id.as_str()[..16];
    let hash = u64::from_str_radix(hex_prefix, 16).unwrap_or(0);
    (hash % worker_count as u64) == worker_index as u64
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
            cdb_hostname,
            rdb_hostname,
            rtq_hostname,
            ldb_hostname,
            worker_index,
            worker_count,
        } => {
            if worker_count == 0 {
                anyhow::bail!("--worker-count must be at least 1");
            }
            if worker_index >= worker_count {
                anyhow::bail!("--worker-index must be less than --worker-count");
            }

            tracing::info!(
                worker_index,
                worker_count,
                "starting deployment engine daemon",
            );

            let cdb_client = cdb::ClientBuilder::new()
                .known_node(&cdb_hostname)
                .build()
                .await?;

            let rdb_client = rdb::ClientBuilder::new()
                .known_node(&rdb_hostname)
                .build()
                .await?;

            let rtq_publisher = rtq::ClientBuilder::new()
                .uri(format!("amqp://{}:5672/%2f", rtq_hostname))
                .build_publisher()
                .await?;
            let ldb_publisher = ldb::ClientBuilder::new()
                .brokers(format!("{}:9092", ldb_hostname))
                .build_publisher()
                .await?;

            let mut workers = BTreeMap::new();

            loop {
                let next_loop = Instant::now() + Duration::from_secs(20);

                if let Err(e) = process(
                    cdb_client.clone(),
                    rdb_client.clone(),
                    rtq_publisher.clone(),
                    ldb_publisher.clone(),
                    &mut workers,
                    worker_index,
                    worker_count,
                )
                .await
                {
                    tracing::error!("{e}")
                }

                tracing::debug!(
                    "will poll for new deployments again in {:.2}s",
                    (next_loop - Instant::now()).as_secs_f64()
                );
                sleep_until(next_loop).await;
            }
        }
    }
}

async fn process(
    client: cdb::Client,
    rdb_client: rdb::Client,
    rtq_publisher: rtq::Publisher,
    ldb_publisher: ldb::Publisher,
    workers: &mut BTreeMap<String, oneshot::Sender<()>>,
    worker_index: u16,
    worker_count: u16,
) -> anyhow::Result<()> {
    let all_deployments = client.active_deployments().await?.collect::<Vec<_>>().await;

    let deployments: Vec<_> = all_deployments
        .into_iter()
        .filter(|d| match d {
            Ok(d) => deployment_owned_by_worker(&d.deployment, worker_index, worker_count),
            Err(_) => true, // propagate errors
        })
        .collect();

    tracing::debug!(
        "found {} deployments for this worker (index={}, count={})",
        deployments.len(),
        worker_index,
        worker_count,
    );

    // Remove workers whose loop has exited (receiver dropped).
    workers.retain(|id, tx| {
        if tx.is_closed() {
            tracing::debug!(dep = %id, "worker exited; removing from pool");
            false
        } else {
            true
        }
    });

    let mut untouched = workers.keys().cloned().collect::<BTreeSet<_>>();
    for deployment in deployments {
        let deployment = deployment?;
        let deployment_qid = deployment.deployment_qid().to_string();
        if !untouched.remove(&deployment_qid) {
            tracing::debug!(dep = %deployment_qid, "new deployment to process");

            let (tx, rx) = oneshot::channel();
            // Resources are namespaced by environment QID.
            let environment_qid = deployment.environment_qid().to_string();

            let log_publisher = match ldb_publisher.namespace(deployment_qid.clone()).await {
                Ok(log_publisher) => log_publisher,
                Err(error) => {
                    tracing::error!(
                        dep = %deployment_qid,
                        error = %error,
                        "failed to create deployment log publisher topic",
                    );
                    continue;
                }
            };

            let env_qid = deployment.environment_qid();
            let worker = Worker {
                client: client.repo(deployment.repo.clone()).deployment(
                    deployment.environment.clone(),
                    deployment.deployment.clone(),
                    deployment.nonce,
                ),
                cdb_client: client.clone(),
                rdb_client: rdb_client.clone(),
                environment_qid: env_qid.clone(),
                namespace: rdb_client.namespace(environment_qid),
                rtq_publisher: rtq_publisher.clone(),
                log_publisher,
                last_failure_at: None,
                cached_compile: None,
            };

            let span = tracing::info_span!("worker", dep = %deployment_qid);
            task::spawn(worker.run_loop(rx).instrument(span));

            workers.insert(deployment_qid, tx);
        }
    }

    for id in untouched {
        tracing::debug!(dep = %id, "no longer watching");
        workers.remove(&id);
    }

    Ok(())
}
