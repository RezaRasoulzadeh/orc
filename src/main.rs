use anyhow::Result;
use clap::{Parser, Subcommand};
use orc::adoption;
use orc::agent;
use orc::codex_app_server::{self, CodexAppServer, QuotaSnapshot};
use orc::discovery;
use orc::doctor::{self, CheckStatus};
use orc::protocol::{
    EngineeringLeadRequest, EngineeringLeadResponse, PROTOCOL_VERSION, PlanResponse,
    PlanningRequest,
};
use orc::registry::{self, AgentDefinition};
use orc::storage::Database;
use orc::task::TaskScopeMode;
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

const DB_PATH: &str = ".orc/orc.db";

fn parse_reasoning_effort(value: &str) -> Result<registry::ReasoningEffort, String> {
    registry::ReasoningEffort::parse(value).map_err(|error| error.to_string())
}

fn ensure_codex_automated_agent(db: &Database, id: &str) -> Result<()> {
    let agent = registry::get_agent(db, id)?;
    if agent.backend != "codex" || agent.execution_mode != registry::AUTOMATED {
        anyhow::bail!(
            "only automated Codex agents support model and reasoning-effort configuration"
        );
    }
    Ok(())
}

fn git_identity(root: &std::path::Path, command: &str) -> Result<Option<String>> {
    let output = ProcessCommand::new("git")
        .current_dir(root)
        .args(command.split_whitespace())
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!value.is_empty()).then_some(value))
}

fn format_timestamp(value: &str) -> String {
    let Ok(timestamp) = value.parse::<i64>() else {
        return value.to_owned();
    };
    let date = ProcessCommand::new("date")
        .args(["-d", &format!("@{timestamp}"), "+%Y-%m-%d %H:%M:%S %Z"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|date| !date.is_empty())
        .unwrap_or_else(|| value.to_owned());
    format!("{date} ({})", format_relative_duration(timestamp))
}

fn format_relative_duration(timestamp: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let difference = timestamp.saturating_sub(now);
    let future = difference >= 0;
    let seconds = difference.unsigned_abs();
    let (amount, unit) = if seconds < 60 {
        (seconds, "s")
    } else if seconds < 3_600 {
        (seconds / 60, "m")
    } else if seconds < 86_400 {
        (seconds / 3_600, "h")
    } else {
        (seconds / 86_400, "d")
    };
    if future {
        format!("in {amount}{unit}")
    } else {
        format!("{amount}{unit} ago")
    }
}

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
    /// Emit a structured project report for a manual planner.
    Report {
        #[arg(long)]
        full: bool,
    },
    /// Emit a structured planning request for a high-level objective.
    PlanRequest {
        #[arg(long)]
        full_report: bool,
        objective: String,
    },
    /// Validate and atomically apply a structured plan response.
    ApplyPlan {
        path: String,
    },
    /// Show the deterministic queue of tasks
    Queue {
        /// Explain task readiness, dependency state, and scheduler eligibility
        #[arg(long)]
        explain: bool,
    },
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
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
    /// Dispatch ready automated tasks concurrently.
    DispatchQueue {
        /// Maximum number of tasks to execute at once.
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
    },
    /// Review the latest task run and its worktree changes.
    Review {
        task_id: String,
        /// Show the complete unified diff.
        #[arg(long, conflicts_with = "file")]
        diff: bool,
        /// Show the unified diff for one changed file.
        #[arg(long, conflicts_with = "diff")]
        file: Option<String>,
    },
    /// Revise a reviewed task using review feedback.
    Revise {
        task_id: String,
        #[arg(long)]
        agent: Option<String>,
        feedback: String,
    },
    /// Schedule an agent for a task using deterministic selection rules
    Schedule {
        /// Task ID to schedule (e.g., T-0001)
        task_id: String,
        /// Explain candidate evaluations and selection reasoning
        #[arg(long)]
        explain: bool,
        /// Restrict selection to a specific execution mode
        #[arg(long, value_parser = ["automated", "manual"])]
        mode: Option<String>,
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
    /// Submit a git patch for a waiting manual run
    SubmitPatch {
        run_id: i64,
        /// Path to patch file (use - for stdin)
        patch_file: String,
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
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
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
    /// Set the configuration profile directory for an existing agent.
    Profile {
        id: String,
        path: String,
    },
    Model {
        id: String,
        model: String,
    },
    Effort {
        id: String,
        #[arg(value_parser = parse_reasoning_effort)]
        effort: registry::ReasoningEffort,
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
    QuotaReserve {
        #[arg(value_parser = clap::value_parser!(i64).range(0..=100))]
        remaining: i64,
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
    /// Set required capabilities for a task
    Require {
        /// Task ID to configure
        task_id: String,
        /// Required capabilities (e.g. code terminal architecture review)
        #[arg(required = true, num_args = 1..)]
        capabilities: Vec<String>,
    },
    Scope {
        task_id: String,
        mode: String,
    },
    ContextAdd {
        task_id: String,
        paths: Vec<String>,
    },
    ContextClear {
        task_id: String,
    },
    ExpectChange {
        task_id: String,
        paths: Vec<String>,
    },
    ExpectClear {
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
    /// Accept a reviewed task and integrate its branch.
    Accept {
        task_id: String,
    },
    /// Reject a reviewed task while preserving its worktree.
    Reject {
        task_id: String,
        reason: Option<String>,
    },
    /// Cancel a task while preserving its worktree.
    Cancel {
        task_id: String,
        reason: Option<String>,
    },
    /// Add a dependency to a task
    Depend {
        /// Task ID that depends on another task
        task_id: String,
        /// Task ID that must be completed first
        dependency_id: String,
    },
    /// Remove a dependency from a task
    Undepend {
        /// Task ID that depends on another task
        task_id: String,
        /// Dependency task ID to remove
        dependency_id: String,
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
        Command::Report { full } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let project = db
                .get_project_name()?
                .ok_or_else(|| anyhow::anyhow!("no project found"))?;
            let contract = std::fs::read_to_string(".orc/engineering.md").unwrap_or_default();
            let project_id = db
                .get_project_id()?
                .ok_or_else(|| anyhow::anyhow!("no project found"))?;
            let root = std::env::current_dir()?;
            let branch = git_identity(&root, "symbolic-ref --quiet --short HEAD")?;
            let commit = git_identity(&root, "rev-parse HEAD")?;
            let facts = db.project_facts(project_id)?;
            let architecture = if full {
                orc::protocol::ReportArchitecture {
                    modules: serde_json::from_str(
                        facts.get("modules").map(String::as_str).unwrap_or("[]"),
                    )?,
                    boundaries: serde_json::from_str(
                        facts.get("boundaries").map(String::as_str).unwrap_or("[]"),
                    )?,
                    discovery: facts,
                }
            } else {
                orc::protocol::ReportArchitecture::default()
            };
            let mut report = db.project_report(
                project_id,
                project,
                root.display().to_string(),
                contract,
                architecture,
            )?;
            report.project.branch = branch;
            report.project.commit = commit;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::PlanRequest {
            full_report,
            objective,
        } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let project = db
                .get_project_name()?
                .ok_or_else(|| anyhow::anyhow!("no project found"))?;
            let contract = std::fs::read_to_string(".orc/engineering.md").unwrap_or_default();
            let root = std::env::current_dir()?;
            let branch = git_identity(&root, "symbolic-ref --quiet --short HEAD")?;
            let commit = git_identity(&root, "rev-parse HEAD")?;
            let project_info = orc::protocol::ReportProject {
                name: project.clone(),
                repository: root.display().to_string(),
                branch,
                commit,
            };
            let full_report = if full_report {
                let project_id = db
                    .get_project_id()?
                    .ok_or_else(|| anyhow::anyhow!("no project found"))?;
                let facts = db.project_facts(project_id)?;
                let architecture = orc::protocol::ReportArchitecture {
                    modules: serde_json::from_str(
                        facts.get("modules").map(String::as_str).unwrap_or("[]"),
                    )?,
                    boundaries: serde_json::from_str(
                        facts.get("boundaries").map(String::as_str).unwrap_or("[]"),
                    )?,
                    discovery: facts,
                };
                let mut report = db.project_report(
                    project_id,
                    project,
                    root.display().to_string(),
                    contract.clone(),
                    architecture,
                )?;
                report.project.branch = project_info.branch.clone();
                report.project.commit = project_info.commit.clone();
                Some(report)
            } else {
                None
            };
            let request = PlanningRequest { protocol_version: PROTOCOL_VERSION, kind: "existing_project".into(), project: Some(project_info), engineering_contract: contract, objective, constraints: vec!["Inspect the repository read-only; do not edit files, database state, or dispatch work.".into()], target_platforms: Vec::new(), stack: Vec::new(), non_goals: vec!["Applying or dispatching the plan".into()], deliverables: vec!["A validated PlanResponse JSON document".into()], definition_of_done: vec!["The plan is complete, dependency-safe, scoped, and ready for human approval.".into()], response_schema: orc::protocol::PlanResponseSchema::v1(), role_boundaries: vec!["Planner analyzes and proposes only; Orc persists changes only after ApplyPlan.".into()], planning_constraints: vec!["Planning must not mutate, create tasks, change lifecycle state, or dispatch agents.".into()], approval_requirements: vec!["Human approval is required before the plan is applied.".into()], current_state: Some(db.planning_project_state()?), full_report };
            println!("{}", serde_json::to_string_pretty(&request)?);
        }
        Command::ApplyPlan { path } => {
            let data = if path == "-" {
                use std::io::{self, Read};
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                std::fs::read_to_string(path)?
            };
            let response: PlanResponse = serde_json::from_str(&data)?;
            response.validate()?;
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let project_id = db
                .get_project_id()?
                .ok_or_else(|| anyhow::anyhow!("no project found"))?;
            let mapping = db.apply_plan(project_id, &response)?;
            println!("{}", serde_json::to_string_pretty(&mapping)?);
        }
        Command::Queue { explain } => match Database::open(DB_PATH) {
            Ok(db) => {
                let report = orc::queue::compute_queue(&db).map_err(|e| anyhow::anyhow!(e))?;
                if explain {
                    print!("{}", report.format_explain());
                } else {
                    print!("{}", report.format_concise());
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
        Command::Dispatch {
            task_id,
            agent,
            model,
            effort,
        } => {
            let selected =
                agent::dispatch_selected_with_options(&task_id, agent.as_deref(), model, effort);
            if let Err(e) = selected {
                eprintln!("Dispatch failed: {:#}", e);
                return Err(e);
            }
            sync_enabled_agents_after_automated_run(&task_id);
        }
        Command::DispatchQueue { concurrency } => {
            let summaries = agent::dispatch_queue(concurrency)?;
            for summary in &summaries {
                println!("{}", orc::review::format_dispatch(summary));
                if summary.run_status == "completed" {
                    sync_enabled_agents_after_automated_run(&summary.task.id);
                }
            }
            println!("Dispatched {} task(s).", summaries.len());
        }
        Command::Review {
            task_id,
            diff,
            file,
        } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let review = orc::review::build_review(&db, &task_id, std::path::Path::new("."))?;
            let output = match file {
                Some(path) => orc::review::format_review_file(&review, &path)?,
                None => orc::review::format_review_with_diff(
                    &review,
                    diff.then_some(review.changes.diff.as_str()),
                ),
            };
            println!("{output}");
        }
        Command::Revise {
            task_id,
            agent,
            feedback,
        } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let task = db
                .get_task(&task_id)?
                .ok_or_else(|| anyhow::anyhow!("task not found"))?;
            let selected = if let Some(id) = agent {
                registry::get_agent(&db, &id)?
            } else {
                let run = db
                    .list_agent_runs_for_task(&task_id)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("task has no agent run"))?;
                registry::get_agent(&db, &run.agent)?
            };
            if selected.execution_mode == registry::MANUAL {
                orc::agent::revise_manual(&task_id, &feedback, &selected, &db, ".")?;
            } else {
                let worker =
                    orc::backend::WorkerFactory::build(&selected).map_err(anyhow::Error::msg)?;
                let summary = orc::agent::revise_with_worker_and_db_as_with_runner(
                    &task_id,
                    &feedback,
                    worker.as_ref(),
                    DB_PATH,
                    ".",
                    &selected.id,
                    &orc::SystemValidationRunner,
                )?;
                println!("{}", orc::review::format_dispatch(&summary));
                sync_enabled_agents_after_automated_run(&task_id);
            }
            let _ = task;
        }
        Command::Schedule {
            task_id,
            explain,
            mode,
        } => {
            let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let task = db
                .get_task(&task_id)
                .map_err(|e| anyhow::anyhow!(e))?
                .ok_or_else(|| anyhow::anyhow!("task '{}' not found in DB", task_id))?;
            let agents = db.list_agents().map_err(|e| anyhow::anyhow!(e))?;
            let reserve = db.quota_reserve().map_err(|e| anyhow::anyhow!(e))?;
            let decision = orc::scheduler::schedule_with_quota_reserve(
                &task,
                &agents,
                mode.as_deref(),
                reserve,
            )
            .map_err(|e| anyhow::anyhow!(e))?;
            if explain {
                println!("{}", decision.format_explanation());
            } else {
                match &decision.selected_agent_id {
                    Some(id) => println!("Selected: {}", id),
                    None => {
                        eprintln!("No eligible agent found for task {}", task_id);
                        anyhow::bail!("no eligible agent found for task '{}'", task_id);
                    }
                }
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
                RunCommand::SubmitPatch { run_id, patch_file } => {
                    let patch_content = if patch_file == "-" {
                        use std::io::Read;
                        let mut output = String::new();
                        std::io::stdin().read_to_string(&mut output)?;
                        output
                    } else {
                        std::fs::read_to_string(&patch_file).map_err(|e| {
                            anyhow::anyhow!("failed to read patch file '{}': {}", patch_file, e)
                        })?
                    };

                    match agent::submit_patch(&db, run_id, &patch_content, ".") {
                        Ok(outcome) => {
                            println!("Run {}", outcome.run_id);
                            println!("Patch: valid");
                            println!("Worktree: {}", outcome.worktree_path.display());
                            println!("Applied: yes\n");
                            println!("Validation:");
                            for step in &outcome.validation_report.steps {
                                let status = if step.passed { "PASS" } else { "FAIL" };
                                println!("  {:<42} {}", step.command, status);
                            }
                            println!("\nRun: completed");
                            println!("Task {}: review", outcome.task_id);
                        }
                        Err(e) => {
                            eprintln!("Submit patch failed: {:#}", e);
                            return Err(e);
                        }
                    }
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
                    model,
                    effort,
                    mode,
                } => {
                    registry::validate_backend(&backend)?;
                    if (model.is_some() || effort.is_some())
                        && (backend != "codex" || mode == registry::MANUAL)
                    {
                        anyhow::bail!(
                            "only automated Codex agents support model and reasoning-effort configuration"
                        );
                    }
                    if mode == registry::AUTOMATED
                        && backend != "codex"
                        && backend != "copilot"
                        && backend != "antigravity"
                    {
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
                        model,
                        reasoning_effort: effort,
                        config_metadata: None,
                        quota_remaining_percent: None,
                        quota_reset_at: None,
                        quota_checked_at: None,
                        quota_source: None,
                        quota_limits: None,
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
                AgentCommand::Profile { id, path } => ensure_agent_updated(
                    db.set_agent_profile_path(&id, &path)
                        .map_err(|e| anyhow::anyhow!(e))?,
                    &id,
                )?,
                AgentCommand::Model { id, model } => {
                    ensure_codex_automated_agent(&db, &id)?;
                    ensure_agent_updated(
                        db.set_agent_model(&id, &model)
                            .map_err(|e| anyhow::anyhow!(e))?,
                        &id,
                    )?;
                }
                AgentCommand::Effort { id, effort } => {
                    ensure_codex_automated_agent(&db, &id)?;
                    ensure_agent_updated(
                        db.set_agent_reasoning_effort(&id, effort)
                            .map_err(|e| anyhow::anyhow!(e))?,
                        &id,
                    )?;
                }
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
                AgentCommand::QuotaReserve { remaining } => {
                    db.set_quota_reserve(remaining)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    println!("Automatic dispatch quota reserve set to {remaining}%.");
                }
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
                    println!(
                        "Model:              {}",
                        agent.model.as_deref().unwrap_or("-")
                    );
                    println!(
                        "Reasoning effort:   {}",
                        agent
                            .reasoning_effort
                            .map(|value| value.as_str())
                            .unwrap_or("-")
                    );
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
                        agent
                            .quota_reset_at
                            .as_deref()
                            .map(format_timestamp)
                            .unwrap_or_else(|| "-".into())
                    );
                    println!(
                        "Quota checked:       {}",
                        agent
                            .quota_checked_at
                            .as_deref()
                            .map(format_timestamp)
                            .unwrap_or_else(|| "-".into())
                    );
                    println!(
                        "Quota source:        {}",
                        agent.quota_source.as_deref().unwrap_or("-")
                    );
                    if let Some(limits) = &agent.quota_limits {
                        println!("Effective limit:    {}", limits.effective);
                        print_quota_limit("Primary limit:", limits.primary.as_ref());
                        print_quota_limit("Secondary limit:", limits.secondary.as_ref());
                        if let Some(limit) = &limits.individual_limit {
                            println!(
                                "Individual limit:   {}% remaining, reset {}",
                                limit.remaining_percent,
                                format_timestamp(&limit.reset_at.to_string())
                            );
                        }
                    }
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
                        println!("ID:           {}", task.id);
                        println!("Title:        {}", task.title);
                        println!("Objective:    {}", task.objective);
                        println!("Role:         {}", task.role);
                        println!("Priority:     {:?}", task.priority);
                        println!("Status:       {}", task.status);
                        if let Some(reason) = &task.cancellation_reason {
                            println!("Cancellation: {}", reason);
                        }
                        println!("Capabilities: {}", task.required_capabilities().join(", "));
                        println!(
                            "Scope: {}",
                            task.scope_mode
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "none".into())
                        );
                        println!("Context files:");
                        for path in &task.context_files {
                            println!("  {path}");
                        }
                        println!("Expected changes:");
                        for path in &task.expected_changes {
                            println!("  {path}");
                        }
                        let deps = db
                            .list_task_dependencies(&task_id)
                            .map_err(|e| anyhow::anyhow!(e))?;
                        if deps.is_empty() {
                            println!("Dependencies: none");
                        } else {
                            println!("Dependencies: {}", deps.join(", "));
                        }
                    }
                    None => {
                        eprintln!("Task {} not found", task_id);
                    }
                },
                Err(_) => {
                    eprintln!("No DB found. Run `orc init` to initialize repository state.");
                }
            },
            TaskCommand::Require {
                task_id,
                capabilities,
            } => match Database::open(DB_PATH) {
                Ok(db) => {
                    let changed = db
                        .set_task_required_capabilities(&task_id, &capabilities)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    if !changed {
                        eprintln!("Task {} not found", task_id);
                        anyhow::bail!("task '{}' not found in DB", task_id);
                    }
                    println!(
                        "Updated capabilities for task {}: {}",
                        task_id,
                        capabilities.join(", ")
                    );
                }
                Err(_) => {
                    eprintln!("No DB found. Run `orc init` to initialize repository state.");
                }
            },
            TaskCommand::Cancel { task_id, reason } => {
                let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
                if !db.cancel_task(&task_id, reason.as_deref())? {
                    let task = db.get_task(&task_id)?;
                    match task {
                        None => anyhow::bail!("task '{}' not found", task_id),
                        Some(task) if task.status == orc::task::TaskStatus::Done => {
                            anyhow::bail!("task '{}' is done and cannot be cancelled", task_id)
                        }
                        Some(_) => anyhow::bail!("task '{}' is already cancelled", task_id),
                    }
                }
                println!("Cancelled task {}", task_id);
            }
            TaskCommand::Scope { task_id, mode } => {
                let scope = TaskScopeMode::parse(&mode)
                    .ok_or_else(|| anyhow::anyhow!("invalid scope mode: {mode}"))?;
                let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
                if !db
                    .set_task_scope(&task_id, scope)
                    .map_err(|e| anyhow::anyhow!(e))?
                {
                    anyhow::bail!("task '{task_id}' not found");
                }
            }
            TaskCommand::ContextAdd { task_id, paths } => {
                let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
                let task = db
                    .get_task(&task_id)
                    .map_err(|e| anyhow::anyhow!(e))?
                    .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found"))?;
                let mut values = task.context_files;
                values.extend(paths);
                db.set_task_context(&task_id, &values)?;
            }
            TaskCommand::ExpectChange { task_id, paths } => {
                let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
                let task = db
                    .get_task(&task_id)
                    .map_err(|e| anyhow::anyhow!(e))?
                    .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found"))?;
                let mut values = task.expected_changes;
                values.extend(paths);
                db.set_task_expected_changes(&task_id, &values)?;
            }
            TaskCommand::ContextClear { task_id } => {
                let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
                let empty = Vec::new();
                db.set_task_context(&task_id, &empty)?;
            }
            TaskCommand::ExpectClear { task_id } => {
                let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
                let empty = Vec::new();
                db.set_task_expected_changes(&task_id, &empty)?;
            }
            TaskCommand::Diff { task_id } => match Database::open(DB_PATH) {
                Ok(db) => {
                    match db
                        .get_worktree_metadata(&task_id)
                        .map_err(|e| anyhow::anyhow!(e))?
                    {
                        Some((_branch, path)) => match orc::git::inspect_worktree(path, ".") {
                            Ok(changes) => {
                                if changes.diff.is_empty() {
                                    println!("No changes in worktree for task {}", task_id);
                                } else {
                                    println!("{}", changes.diff);
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
            TaskCommand::Accept { task_id } => {
                let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
                orc::agent::accept_task(&db, &task_id, ".")?;
                println!(
                    "Accepted task {}; changes integrated and task marked done.",
                    task_id
                );
            }
            TaskCommand::Reject { task_id, reason } => {
                let db = Database::open(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
                orc::agent::reject_task(&db, &task_id, reason.as_deref())?;
                println!(
                    "Rejected task {}; worktree preserved and task moved to ready.",
                    task_id
                );
            }
            TaskCommand::Depend {
                task_id,
                dependency_id,
            } => match Database::open(DB_PATH) {
                Ok(db) => {
                    db.add_task_dependency(&task_id, &dependency_id)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    println!("Added dependency: {} depends on {}", task_id, dependency_id);
                }
                Err(_) => {
                    eprintln!("No DB found. Run `orc init` to initialize repository state.");
                }
            },
            TaskCommand::Undepend {
                task_id,
                dependency_id,
            } => match Database::open(DB_PATH) {
                Ok(db) => {
                    let changed = db
                        .remove_task_dependency(&task_id, &dependency_id)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    if !changed {
                        anyhow::bail!("dependency '{}' -> '{}' not found", task_id, dependency_id);
                    }
                    println!(
                        "Removed dependency: {} no longer depends on {}",
                        task_id, dependency_id
                    );
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

fn sync_enabled_agents_after_automated_run(task_id: &str) {
    match Database::open(DB_PATH) {
        Ok(db) => {
            let automated = db
                .list_agent_runs_for_task(task_id)
                .ok()
                .and_then(|runs| runs.into_iter().next())
                .is_some_and(|run| run.execution_mode == registry::AUTOMATED);
            if automated {
                match codex_app_server::sync_enabled_agents_after_automated_run(
                    &db,
                    &CodexAppServer,
                ) {
                    Ok(results) => {
                        for (id, result) in results {
                            if let Err(error) = result {
                                eprintln!("{id}: quota sync failed: {error}");
                            }
                        }
                    }
                    Err(error) => eprintln!("quota sync failed: {error}"),
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
            .map(|value| format_timestamp(&value.to_string()))
            .unwrap_or_else(|| "unknown".to_owned())
    );
    println!("  effective limit: {}", snapshot.limits.effective);
}

fn print_quota_limit(label: &str, limit: Option<&orc::registry::QuotaLimit>) {
    match limit {
        Some(limit) => println!(
            "{label:<20} {}% ({} min), reset {}",
            limit.remaining_percent,
            limit
                .window_duration_mins
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".into()),
            limit
                .reset_at
                .map(|value| format_timestamp(&value.to_string()))
                .unwrap_or_else(|| "unknown".into())
        ),
        None => println!("{label:<20} -"),
    }
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
            agent
                .quota_reset_at
                .as_deref()
                .map(format_timestamp)
                .unwrap_or_else(|| "-".into())
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
