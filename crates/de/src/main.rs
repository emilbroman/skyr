use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use cdb::{DeploymentClient, DeploymentState};
use clap::Parser;
use futures_util::StreamExt;
use slog::{Drain, Logger, debug, error, info, o};
use tokio::{
    sync::oneshot::{self, error::TryRecvError},
    task,
    time::{Instant, sleep_until},
};

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "db-hostname", default_value = "localhost")]
        db_hostname: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();

    let log = slog::Logger::root(drain, o!());

    match Program::parse() {
        Program::Daemon { db_hostname } => {
            info!(log, "starting deployment engine daemon");

            let client = cdb::ClientBuilder::new()
                .known_node(db_hostname)
                .build()
                .await?;

            let mut workers = BTreeMap::new();

            loop {
                let next_loop = Instant::now() + Duration::from_secs(20);

                if let Err(e) = process(log.clone(), client.clone(), &mut workers).await {
                    error!(log, "{e}")
                }

                debug!(
                    log,
                    "will poll for new deployments again in {:.2}s",
                    (next_loop - Instant::now()).as_secs_f64()
                );
                sleep_until(next_loop).await;
            }
        }
    }
}

async fn process(
    log: Logger,
    client: cdb::Client,
    workers: &mut BTreeMap<String, oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let deployments = client.active_deployments().await?.collect::<Vec<_>>().await;

    debug!(log, "found {} deployments", deployments.len());

    let mut untouched = workers.keys().cloned().collect::<BTreeSet<_>>();
    for deployment in deployments {
        let deployment = deployment?;
        let id = deployment.fqid();
        if !untouched.remove(&id) {
            debug!(log, "new deployment to process: {}", deployment.fqid());

            let (tx, rx) = oneshot::channel();

            let worker = Worker {
                log: log.new(o!("dep" => deployment.fqid())),
                client: client.repo(deployment.repository).deployment(deployment.id),
            };

            task::spawn(worker.run_loop(rx));

            workers.insert(id, tx);
        }
    }

    for id in untouched {
        debug!(log, "no longer watching {}", id);
        workers.remove(&id);
    }

    Ok(())
}

struct Worker {
    log: Logger,
    client: DeploymentClient,
}

impl Worker {
    async fn run_loop(mut self, mut rx: oneshot::Receiver<()>) {
        loop {
            let next_loop = Instant::now() + Duration::from_secs(5);

            match rx.try_recv() {
                Ok(()) | Err(TryRecvError::Closed) => return,
                Err(TryRecvError::Empty) => {
                    if let Err(e) = self.work().await {
                        error!(self.log, "{e}");
                    }
                }
            }

            debug!(
                self.log,
                "will reconcile in {:.2}s",
                (next_loop - Instant::now()).as_secs_f64()
            );
            sleep_until(next_loop).await;
        }
    }

    async fn work(&mut self) -> anyhow::Result<()> {
        let deployment = self.client.get().await?;

        match deployment.state {
            DeploymentState::Down => {
                info!(
                    &self.log,
                    "deployment down, waiting to be decommissioned..."
                );
                Ok(())
            }

            DeploymentState::Desired => {
                // TODO: resource management

                info!(&self.log, "reconciling");

                let program = sclc::compile(self.client.clone()).await?;
                debug!(&self.log, "{program:?}");

                if let Some(superceded) = self.client.get_superceded().await? {
                    superceded.set(DeploymentState::Undesired).await?;
                }

                Ok(())
            }

            DeploymentState::Undesired => {
                info!(&self.log, "tearing down");

                // TODO: delete any resources

                info!(&self.log, "no more resources, setting state to DOWN");
                self.client.set(DeploymentState::Down).await?;
                Ok(())
            }

            DeploymentState::Lingering => {
                info!(&self.log, "lingering...");

                let program = sclc::compile(self.client.clone()).await?;
                debug!(&self.log, "{program:?}");
                Ok(())
            }
        }
    }
}
