use std::{convert::Infallible, path::Path};

use clap::Parser;
use rustyline::{DefaultEditor, error::ReadlineError};
use tokio::task;

struct ReplSource {
    number: usize,
    history: String,
}

impl sclc::SourceRepo for ReplSource {
    type Err = Infallible;

    fn package_id(&self) -> sclc::ModuleId {
        [format!("Repl{}", self.number)]
            .into_iter()
            .collect::<sclc::ModuleId>()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        if path == Path::new("Main.scl") {
            return Ok(Some(self.history.as_bytes().to_vec()));
        }

        Ok(None)
    }
}

#[derive(Parser)]
enum Program {
    Repl,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Program::parse() {
        Program::Repl => {
            run_repl().await?;
        }
    }

    Ok(())
}

async fn run_repl() -> anyhow::Result<()> {
    let mut history = String::new();
    let mut line_number = 0usize;
    let mut program = sclc::Program::<ReplSource>::new();
    let (effects_tx, mut effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let effects_task = task::spawn(async move {
        while let Some(effect) = effects_rx.recv().await {
            match effect {
                sclc::Effect::Print(value) => println!("{value}"),
            }
        }
    });
    let mut editor = DefaultEditor::new()?;

    loop {
        match editor.readline("scl> ") {
            Ok(line) => {
                let input = line.trim();
                if matches!(input, "exit" | "quit") {
                    break;
                }

                editor.add_history_entry(&line)?;

                line_number += 1;
                history.push_str(&line);
                history.push('\n');

                let source = ReplSource {
                    number: line_number,
                    history: history.clone(),
                };
                let _ = program.open_package(source).await;
                let module_id = [format!("Repl{}", line_number), String::from("Main")]
                    .into_iter()
                    .collect::<sclc::ModuleId>();
                let value = program.evaluate(&module_id, effects_tx.clone()).await?;
                println!("{value:?}");
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(err) => return Err(err.into()),
        }
    }

    drop(effects_tx);
    effects_task.await?;

    Ok(())
}
