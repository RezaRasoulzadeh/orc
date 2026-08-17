use anyhow::Result;
use clap::{Parser, Subcommand};
use orc::adoption;
use orc::agent;
use orc::codex_app_server::{self, CodexAppServer, QuotaSnapshot};
use orc::discovery;
use orc::doctor::{self, CheckStatus};
use orc::protocol::{EngineeringLeadRequest, EngineeringLeadResponse};
use orc::registry::{self, AgentDefinition};
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
    /// List registered agents.
    Agents {
        /// Synchronize quota for enabled supported agents before listing.
        #[arg(long)]
        sync: bool,
    },
    /// Diagnose project and configured agent health without consuming model quota.
    Doctor,
    Status,
    Ask {
        request: String,
    },
    ApplyResponse {
        /// Path to JSON response file produced by the engineering lead (use - for stdin)
        path: String,
    },
    /// Dispatch a task using a selected registered agent
    Dispatch {
        /// Task ID to dispatch (e.g., T-0001)
        task_id: String,
        /// Explicit agent override; selection validity checks still apply.
        #[arg(long)]
        agent: Option<String>,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
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
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
}

#[derive(Subcommand)]
enum RunCommand {
    Submit {
        run_id: i64,
        #[arg(long)]
        file: Option<String>,
    },
    Fail {
        run_id: i64,
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
enum AgentCommand {
    List,
    Add {
        id: String,
        #[arg(long)]
        backend: String,
        #[arg(long, default_value_t = 0)]
        priority: i64,
        #[arg(long)]
        capability: Vec<String>,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, value_parser = ["automated", "manual"], default_value = "automated")]
        mode: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Unavailable {
        id: String,
        reason: String,
    },
    Available {
        id: String,
    },
    Priority {
        id: String,
        priority: i64,
    },
    Quota {
        id: String,
        #[arg(long, value_parser = clap::value_parser!(i64).range(0..=100))]
        remaining: i64,
        #[arg(long)]
        reset: Option<String>,
    },
    QuotaClear {
        id: String,
    },
    /// Synchronize quota through the provider's machine-readable protocol.
    Sync {
        id: String,
    },
    Show {
        id: String,
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
        Command::Agents { sync } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            if sync {
                sync_enabled_agents(&db);
            }
            print_agents(&db)?;
        }
        Command::Doctor => print_doctor(&doctor::inspect(".", &doctor::SystemHealthRunner)),
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
        Command::Dispatch { task_id, agent } => {
            if let Err(e) = agent::dispatch_selected(&task_id, agent.as_deref()) {
                eprintln!("Dispatch failed: {:#}", e);
                return Err(e);
            }
        }
        Command::Run { command } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            match command {
                RunCommand::Submit { run_id, file } => {
                    let output = match file.as_deref() {
                        Some(path) => std::fs::read_to_string(path)?,
                        None => {
                            use std::io::Read;
                            let mut output = String::new();
                            std::io::stdin().read_to_string(&mut output)?;
                            output
                        }
                    };
                    let task_id = agent::submit_run(&db, run_id, &output)?;
                    println!(
                        "Run {} completed; task {} moved to review.",
                        run_id, task_id
                    );
                }
                RunCommand::Fail { run_id, reason } => {
                    let task_id = agent::fail_run(
                        &db,
                        run_id,
                        reason.as_deref().unwrap_or("manual run failed"),
                    )?;
                    println!("Run {} failed; task {} moved to blocked.", run_id, task_id);
                }
            }
        }
        Command::Agent { command } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            match command {
                AgentCommand::List => {
                    print_agents(&db)?;
                }
                AgentCommand::Add {
                    id,
                    backend,
                    priority,
                    capability,
                    display_name,
                    profile,
                    mode,
                } => {
                    registry::validate_backend(&backend)?;
                    if mode == registry::AUTOMATED && backend != "codex" && backend != "copilot" {
                        anyhow::bail!("backend '{}' requires --mode manual", backend);
                    }
                    let agent = AgentDefinition {
                        display_name: display_name.unwrap_or_else(|| id.clone()),
                        id,
                        backend,
                        execution_mode: mode,
                        enabled: true,
                        priority,
                        capabilities: capability,
                        status: registry::AVAILABLE.to_owned(),
                        unavailable_reason: None,
                        profile_path: profile,
                        config_metadata: None,
                        quota_remaining_percent: None,
                        quota_reset_at: None,
                        quota_checked_at: None,
                        quota_source: None,
                    };
                    db.insert_agent(&agent).map_err(|e| anyhow::anyhow!(e))?;
                    println!("Added agent {}", agent.id);
                }
                AgentCommand::Enable { id } => update_agent_enabled(&db, &id, true)?,
                AgentCommand::Disable { id } => update_agent_enabled(&db, &id, false)?,
                AgentCommand::Unavailable { id, reason } => {
                    ensure_agent_updated(
                        db.set_agent_availability(&id, registry::UNAVAILABLE, Some(&reason))
                            .map_err(|e| anyhow::anyhow!(e))?,
                        &id,
                    )?;
                }
                AgentCommand::Available { id } => {
                    ensure_agent_updated(
                        db.set_agent_availability(&id, registry::AVAILABLE, None)
                            .map_err(|e| anyhow::anyhow!(e))?,
                        &id,
                    )?;
                }
                AgentCommand::Priority { id, priority } => ensure_agent_updated(
                    db.set_agent_priority(&id, priority)
                        .map_err(|e| anyhow::anyhow!(e))?,
                    &id,
                )?,
                AgentCommand::Quota {
                    id,
                    remaining,
                    reset,
                } => ensure_agent_updated(
                    db.set_agent_quota(&id, remaining, reset.as_deref())
                        .map_err(|e| anyhow::anyhow!(e))?,
                    &id,
                )?,
                AgentCommand::QuotaClear { id } => ensure_agent_updated(
                    db.clear_agent_quota(&id).map_err(|e| anyhow::anyhow!(e))?,
                    &id,
                )?,
                AgentCommand::Sync { id } => {
                    let agent = registry::get_agent(&db, &id)?;
                    let snapshot = codex_app_server::sync_agent(&db, &agent, &CodexAppServer)
                        .map_err(anyhow::Error::msg)?;
                    print_synced_quota(&id, &snapshot);
                }
                AgentCommand::Show { id } => {
                    let agent = registry::get_agent(&db, &id)?;
                    println!("ID:                 {}", agent.id);
                    println!("Backend:            {}", agent.backend);
                    println!("Execution mode:     {}", agent.execution_mode);
                    println!("Display name:       {}", agent.display_name);
                    println!("Enabled:            {}", agent.enabled);
                    println!("Availability:       {}", agent.status);
                    println!(
                        "Unavailable reason: {}",
                        agent.unavailable_reason.as_deref().unwrap_or("-")
                    );
                    println!("Priority:            {}", agent.priority);
                    println!(
                        "Quota:               {}",
                        agent
                            .quota_remaining_percent
                            .map(|value| format!("{value}%"))
                            .unwrap_or_else(|| "unknown".into())
                    );
                    println!(
                        "Quota reset:         {}",
                        agent.quota_reset_at.as_deref().unwrap_or("-")
                    );
                    println!(
                        "Quota checked:       {}",
                        agent.quota_checked_at.as_deref().unwrap_or("-")
                    );
                    println!(
                        "Quota source:        {}",
                        agent.quota_source.as_deref().unwrap_or("-")
                    );
                    println!("Capabilities:        {}", agent.capabilities.join(", "));
                    println!(
                        "Profile/config:      {}",
                        agent.profile_path.as_deref().unwrap_or("-")
                    );
                }
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
                            "{} {} {} {} {}",
                            run.id,
                            run.task_id.as_deref().unwrap_or("-"),
                            run.agent,
                            run.execution_mode,
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

fn update_agent_enabled(db: &Database, id: &str, enabled: bool) -> Result<()> {
    ensure_agent_updated(
        db.set_agent_enabled(id, enabled)
            .map_err(|e| anyhow::anyhow!(e))?,
        id,
    )
}

fn ensure_agent_updated(changed: bool, id: &str) -> Result<()> {
    if !changed {
        anyhow::bail!("agent '{}' is not registered", id);
    }
    Ok(())
}

fn sync_enabled_agents(db: &Database) {
    match codex_app_server::sync_enabled_agents(db, &CodexAppServer) {
        Ok(results) => {
            for (id, result) in results {
                match result {
                    Ok(snapshot) => print_synced_quota(&id, &snapshot),
                    Err(error) => eprintln!("{id}: quota sync failed: {error}"),
                }
            }
        }
        Err(error) => eprintln!("quota sync failed: {error}"),
    }
}

fn print_synced_quota(id: &str, snapshot: &QuotaSnapshot) {
    println!("{id}:");
    println!("  remaining: {}%", snapshot.remaining_percent);
    println!(
        "  reset: {}",
        snapshot
            .reset_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_owned())
    );
}

fn print_agents(db: &Database) -> Result<()> {
    println!(
        "{:<18} {:<9} {:<10} {:<12} {:<10} {:<7} RESET",
        "ID", "BACKEND", "MODE", "STATUS", "PRIORITY", "QUOTA"
    );
    for agent in db.list_agents().map_err(|e| anyhow::anyhow!(e))? {
        println!(
            "{:<18} {:<9} {:<10} {:<12} {:<10} {:<7} {}",
            agent.id,
            agent.backend,
            agent.execution_mode,
            if agent.enabled {
                agent.status.as_str()
            } else {
                "disabled"
            },
            agent.priority,
            agent
                .quota_remaining_percent
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "?".into()),
            agent.quota_reset_at.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

fn print_doctor(report: &doctor::DoctorReport) {
    println!("ORC DOCTOR\n\nProject");
    for check in &report.project {
        print_check(check);
    }
    println!("\nAgents");
    if report.agents.is_empty() {
        println!("  (none enabled)");
    } else {
        for check in &report.agents {
            print_check(check);
        }
    }
    println!("\nOverall: {}", report.overall());
}

fn print_check(check: &doctor::Check) {
    let detail = check
        .detail
        .as_deref()
        .map(|detail| format!(" ({detail})"))
        .unwrap_or_default();
    match &check.status {
        CheckStatus::Ok => println!("  {:<20} OK{detail}", check.name),
        CheckStatus::Unavailable(reason) => {
            println!("  {:<20} UNAVAILABLE: {reason}{detail}", check.name)
        }
        CheckStatus::Failed(reason) => println!("  {:<20} FAILED: {reason}{detail}", check.name),
    }
}
