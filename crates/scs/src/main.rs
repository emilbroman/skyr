use clap::Parser;

#[derive(Parser)]
enum Program {}

#[tokio::main]
async fn main() {
    let _program = Program::parse();
}
