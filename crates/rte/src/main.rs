use clap::Parser;
use slog::{Drain, info, o};
use tokio::task::yield_now;

#[derive(Parser)]
enum Program {
    Daemon,
}

#[tokio::main]
async fn main() {
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();

    let log = slog::Logger::root(drain, o!());

    match Program::parse() {
        Program::Daemon => {
            info!(log, "starting resource transition engine daemon");
            loop {
                yield_now().await
            }
        }
    }
}
