mod protocol;
mod state;
mod task;

use anyhow::Result;
use clap::{Parser, Subcommand};
use protocol::{EngineeringLeadRequest, EngineeringLeadResponse};
use state::OrcState;

#[derive(Parser)]
#[command(name = "orc", version, about = "Local AI engineering orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Init,
    Status,
    Ask {
        request: String,
    },
    ApplyResponse {
        /// Path to JSON response file produced by the engineering lead (use - for stdin)
        path: String,
    },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init => {
            let state = OrcState::new("orc");
            state.save()?;
            println!("Initialized Orc in .orc/state.json");
        }
        Command::Status => match OrcState::load() {
            Ok(state) => {
                println!("Project: {}", state.project);
                println!("Tasks: {}", state.tasks.len());
                for task in state.tasks {
                    println!("{}  {:<10} {}", task.id, task.status, task.title);
                }
            }
            Err(_) => {
                eprintln!("No state found. Run `orc init` to initialize repository state.");
            }
        },
        Command::Ask { request } => match OrcState::load() {
            Ok(state) => {
                let lead_request = EngineeringLeadRequest::from_state(request, &state);
                println!("{}", serde_json::to_string_pretty(&lead_request)?);
            }
            Err(_) => {
                eprintln!("No state found. Run `orc init` to initialize repository state.");
            }
        },
        Command::ApplyResponse { path } => {
            let mut state = match OrcState::load() {
                Ok(s) => s,
                Err(_) => {
                    eprintln!("No state found. Run `orc init` to initialize repository state.");
                    return Ok(());
                }
            };

            let data = if path == "-" {
                use std::io::{self, Read};
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                std::fs::read_to_string(&path)?
            };

            let response: EngineeringLeadResponse = serde_json::from_str(&data)?;
            apply_lead_response(&mut state, response);
            state.save()?;
            println!("Applied response and saved state.");
        }
        Command::Task { command } => match command {
            TaskCommand::List => match OrcState::load() {
                Ok(state) => {
                    for task in state.tasks {
                        println!("{}\t{}\t{}", task.id, task.status, task.title);
                    }
                }
                Err(_) => {
                    eprintln!("No state found. Run `orc init` to initialize repository state.");
                }
            },
        },
    }

    Ok(())
}

#[allow(dead_code)]
fn apply_lead_response(state: &mut OrcState, response: EngineeringLeadResponse) {
    for action in response.actions {
        state.apply_action(action);
    }
}
