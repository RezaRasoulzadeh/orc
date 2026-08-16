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
        Command::Status => {
            let state = OrcState::load()?;
            println!("Project: {}", state.project);
            println!("Tasks: {}", state.tasks.len());
            for task in state.tasks {
                println!("{}  {:<10} {}", task.id, task.status, task.title);
            }
        }
        Command::Ask { request } => {
            let state = OrcState::load()?;
            let lead_request = EngineeringLeadRequest::from_state(request, &state);
            println!("{}", serde_json::to_string_pretty(&lead_request)?);
        }
        Command::Task { command } => match command {
            TaskCommand::List => {
                let state = OrcState::load()?;
                for task in state.tasks {
                    println!("{}\t{}\t{}", task.id, task.status, task.title);
                }
            }
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
