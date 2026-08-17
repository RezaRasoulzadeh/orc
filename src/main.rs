use anyhow::Result;
use clap::{Parser, Subcommand};
use orc::adoption;
use orc::agent;
use orc::discovery;
use orc::protocol::{EngineeringLeadRequest, EngineeringLeadResponse};
use orc::storage::Database;

const DB_PATH: &str = ".orc/orc.db";

#[derive(Parser)]
#[command(name = "orc", version, about = "Local AI engineering orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Init,
    /// Adopt the existing Git repository in the current directory.
    Adopt,
    /// Emit a read-only repository discovery request as JSON.
    DiscoveryRequest,
    /// Apply a structured repository discovery response from a JSON file (or - for stdin).
    ApplyDiscovery {
        path: String,
    },
    Status,
    Ask {
        request: String,
    },
    ApplyResponse {
        /// Path to JSON response file produced by the engineering lead (use - for stdin)
        path: String,
    },
    /// Dispatch a worker to execute a single task using Copilot
    Dispatch {
        /// Task ID to dispatch (e.g., T-0001)
        task_id: String,
    },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Manage and view agent runs
    Runs {
        /// Optional task ID to filter runs for a specific task
        task_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    List,
    /// Display task details
    Show {
        /// Task ID to display
        task_id: String,
    },
    /// Show the diff for a task worktree
    Diff {
        /// Task ID
        task_id: String,
    },
    /// Show worktree information for a task
    Worktree {
        /// Task ID
        task_id: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init => {
            // initialize sqlite DB
            let db = Database::init(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let pid = match db.get_project_id().map_err(|e| anyhow::anyhow!(e))? {
                Some(id) => id,
                None => db.create_project("orc").map_err(|e| anyhow::anyhow!(e))?,
            };
            println!("Initialized Orc DB in {} (project id={})", DB_PATH, pid);
        }
        Command::Adopt => {
            let root = adoption::adopt(".")?;
            println!("Adopted repository {}", root.display());
        }
        Command::DiscoveryRequest => {
            let request = discovery::build_request(".")?;
            println!("{}", serde_json::to_string_pretty(&request)?);
        }
        Command::ApplyDiscovery { path } => {
            let data = if path == "-" {
                use std::io::{self, Read};
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                std::fs::read_to_string(&path)?
            };
            let response = discovery::parse_response(&data)?;
            discovery::apply_response(".", &response)?;
            println!("Applied repository discovery.");
        }
        Command::Status => match Database::open(DB_PATH) {
            Ok(db) => {
                let project = db.get_project_name().map_err(|e| anyhow::anyhow!(e))?;
                if let Some(name) = project {
                    println!("Project: {}", name);
                    let tasks = db.list_tasks().map_err(|e| anyhow::anyhow!(e))?;
                    println!("Tasks: {}", tasks.len());
                    for task in tasks {
                        println!("{}  {:<10} {}", task.id, task.status, task.title);
                    }
                } else {
                    eprintln!(
                        "No project found in DB. Run `orc init` to initialize repository state."
                    );
                }
            }
            Err(_) => {
                eprintln!("No DB found. Run `orc init` to initialize repository state.");
            }
        },
        Command::Ask { request } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let project = db
                .get_project_name()
                .map_err(|e| anyhow::anyhow!(e))?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No project found in DB. Run `orc init` to initialize repository state."
                    )
                })?;
            let tasks = db.list_tasks().map_err(|e| anyhow::anyhow!(e))?;
            let lead_request = EngineeringLeadRequest::from_tasks(request, project, &tasks);
            println!("{}", serde_json::to_string_pretty(&lead_request)?);
        }
        Command::ApplyResponse { path } => {
            let data = if path == "-" {
                use std::io::{self, Read};
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                std::fs::read_to_string(&path)?
            };

            let response: EngineeringLeadResponse = serde_json::from_str(&data)?;

            // persist to sqlite
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;

            let project_id = match db.get_project_id().map_err(|e| anyhow::anyhow!(e))? {
                Some(id) => id,
                None => db.create_project("orc").map_err(|e| anyhow::anyhow!(e))?,
            };

            // Apply the response atomically via the storage layer.
            db.apply_engineering_lead_response(project_id, &response)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("Applied response to DB.");
        }
        Command::Dispatch { task_id } => {
            if let Err(e) = agent::dispatch(&task_id) {
                eprintln!("Dispatch failed: {:#}", e);
                return Err(e);
            }
        }
        Command::Task { command } => match command {
            TaskCommand::List => match Database::open(DB_PATH) {
                Ok(db) => {
                    let tasks = db.list_tasks().map_err(|e| anyhow::anyhow!(e))?;
                    for task in tasks {
                        println!("{}\t{}\t{}", task.id, task.status, task.title);
                    }
                }
                Err(_) => {
                    eprintln!("No DB found. Run `orc init` to initialize repository state.");
                }
            },
            TaskCommand::Show { task_id } => match Database::open(DB_PATH) {
                Ok(db) => match db.get_task(&task_id).map_err(|e| anyhow::anyhow!(e))? {
                    Some(task) => {
                        println!("ID:        {}", task.id);
                        println!("Title:     {}", task.title);
                        println!("Objective: {}", task.objective);
                        println!("Role:      {}", task.role);
                        println!("Priority:  {:?}", task.priority);
                        println!("Status:    {}", task.status);
                    }
                    None => {
                        eprintln!("Task {} not found", task_id);
                    }
                },
                Err(_) => {
                    eprintln!("No DB found. Run `orc init` to initialize repository state.");
                }
            },
            TaskCommand::Diff { task_id } => match Database::open(DB_PATH) {
                Ok(db) => {
                    match db
                        .get_worktree_metadata(&task_id)
                        .map_err(|e| anyhow::anyhow!(e))?
                    {
                        Some((_branch, _path)) => match orc::git::show_diff(&task_id, ".") {
                            Ok(diff) => {
                                if diff.is_empty() {
                                    println!("No changes in worktree for task {}", task_id);
                                } else {
                                    println!("{}", diff);
                                }
                            }
                            Err(e) => {
                                eprintln!("Failed to show diff: {}", e);
                            }
                        },
                        None => {
                            eprintln!("No worktree found for task {}", task_id);
                        }
                    }
                }
                Err(_) => {
                    eprintln!("No DB found. Run `orc init` to initialize repository state.");
                }
            },
            TaskCommand::Worktree { task_id } => match Database::open(DB_PATH) {
                Ok(db) => {
                    match db
                        .get_worktree_metadata(&task_id)
                        .map_err(|e| anyhow::anyhow!(e))?
                    {
                        Some((branch, path)) => {
                            println!("Task:     {}", task_id);
                            println!("Branch:   {}", branch);
                            println!("Worktree: {}", path);
                        }
                        None => {
                            eprintln!("No worktree found for task {}", task_id);
                        }
                    }
                }
                Err(_) => {
                    eprintln!("No DB found. Run `orc init` to initialize repository state.");
                }
            },
        },
        Command::Runs { task_id } => match Database::open(DB_PATH) {
            Ok(db) => {
                let runs = if let Some(tid) = task_id {
                    // Show runs for specific task
                    db.list_agent_runs_for_task(&tid)
                        .map_err(|e| anyhow::anyhow!(e))?
                } else {
                    // Show recent runs for project
                    let pid = db
                        .get_project_id()
                        .map_err(|e| anyhow::anyhow!(e))?
                        .ok_or_else(|| anyhow::anyhow!("no project found in DB"))?;
                    db.list_agent_runs(pid, 50)
                        .map_err(|e| anyhow::anyhow!(e))?
                };

                if runs.is_empty() {
                    println!("No agent runs found");
                } else {
                    for run in runs {
                        println!(
                            "{} {} {} {}",
                            run.id,
                            run.task_id.as_deref().unwrap_or("-"),
                            run.agent,
                            run.status
                        );
                        if let Some(finished) = run.finished_at {
                            println!("  Started:  {}", run.started_at);
                            println!("  Finished: {}", finished);
                        } else {
                            println!("  Started: {}", run.started_at);
                        }
                        if let Some(output) = run.output {
                            println!("  Output: {}", output);
                        }
                    }
                }
            }
            Err(_) => {
                eprintln!("No DB found. Run `orc init` to initialize repository state.");
            }
        },
    }

    Ok(())
}
