use clap::Parser;
use slog::{Drain, Logger, error, info, o, warn};
use tokio::task;

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "mq-hostname", default_value = "localhost")]
        mq_hostname: String,

        #[clap(long = "worker-index", default_value_t = 0)]
        worker_index: u16,

        #[clap(long = "worker-count", default_value_t = 1)]
        worker_count: u16,

        #[clap(long = "local-workers", default_value_t = 1)]
        local_workers: u16,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();

    let log = slog::Logger::root(drain, o!());

    match Program::parse() {
        Program::Daemon {
            mq_hostname,
            worker_index,
            worker_count,
            local_workers,
        } => {
            info!(log, "starting resource transition engine daemon");

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

            let uri = format!("amqp://{}:5672/%2f", mq_hostname);
            let mut handles = Vec::new();

            for offset in 0..local_workers {
                let index = worker_index + offset;
                let worker_cfg = rtq::WorkerConfig {
                    worker_index: index,
                    worker_count,
                };

                let consumer = rtq::ClientBuilder::new()
                    .uri(uri.clone())
                    .build_consumer(worker_cfg)
                    .await?;

                let worker_log = log.new(o!("worker" => format!("{}/{}", index, worker_count)));
                info!(
                    worker_log,
                    "started rtq consumer";
                    "shards" => format!("{:?}", consumer.owned_shards())
                );

                handles.push(task::spawn(worker_loop(worker_log, consumer)));
            }

            for handle in handles {
                if let Err(e) = handle.await? {
                    error!(log, "worker loop failed: {e}");
                }
            }

            Ok(())
        }
    }
}

async fn worker_loop(log: Logger, mut consumer: rtq::Consumer) -> anyhow::Result<()> {
    loop {
        let Some(delivery) = consumer.next().await? else {
            warn!(log, "rtq consumer stream closed");
            return Ok(());
        };

        // TODO: route message to transition handlers.
        info!(
            log,
            "received rtq message";
            "redelivered" => delivery.redelivered(),
            "message" => format!("{:?}", delivery.message)
        );

        // Placeholder behavior until transition execution is implemented.
        delivery.ack().await?;
    }
}
