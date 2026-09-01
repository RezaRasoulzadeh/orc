use anyhow::Result;
use clap::{Parser, Subcommand};
use orc::adoption;
use orc::agent;
use orc::cli::agent::AgentCommand;
use orc::cli::economy::EconomyCommand;
use orc::cli::run::RunCommand;
use orc::cli::task::TaskCommand;
use orc::desktop;
use orc::discovery;
use orc::doctor::{self, CheckStatus};
use orc::protocol::{EngineeringLeadResponse, PROTOCOL_VERSION, PlanResponse, PlanningRequest};
use orc::registry;
use orc::storage::Database;
use std::process::Command as ProcessCommand;
const DB_PATH: &str = ".orc/orc.db";

fn parse_reasoning_effort(value: &str) -> Result<registry::ReasoningEffort, String> {
    registry::ReasoningEffort::parse(value).map_err(|error| error.to_string())
}

#[derive(Clone, Copy)]
enum Concurrency {
    Auto,
    Limited(usize),
}

fn parse_concurrency(value: &str) -> Result<Concurrency, String> {
    if value == "auto" {
        return Ok(Concurrency::Auto);
    }
    let concurrency = value
        .parse::<usize>()
        .map_err(|_| "concurrency must be a positive integer or 'auto'".to_string())?;
    if concurrency == 0 {
        return Err("concurrency must be a positive integer or 'auto'".to_string());
    }
    Ok(Concurrency::Limited(concurrency))
}

#[allow(dead_code)]
fn ensure_provider_execution_options(db: &Database, id: &str) -> Result<()> {
    let agent = registry::get_agent(db, id)?;
    if agent.execution_mode != registry::AUTOMATED
        || !orc::backend::provider_supports_execution_options(&agent.backend)
    {
        anyhow::bail!(
            "only automated providers with execution settings support model and reasoning-effort configuration"
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

fn lead_decision_json<T: serde::Serialize>(decision: T) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(decision)?;
    if let Some(details) = value.get_mut("details")
        && let Some(serialized) = details.as_str()
    {
        *details = serde_json::from_str(serialized)?;
    }
    Ok(value)
}

#[derive(Parser)]
#[command(name = "orc", version, about = "Local AI engineering orchestrator")]
struct Cli {
    /// Launch the installed desktop application and return immediately.
    #[arg(long)]
    ui: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Init,
    /// Adopt the existing Git repository, then assess an objective with Lead.
    Adopt {
        /// Operator objective to assess after repository discovery and adoption.
        objective: Option<String>,
    },
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
    /// Inspect the derived, read-only orchestration position.
    #[command(name = "workflow-state", alias = "workflow")]
    WorkflowState {
        #[arg(long)]
        json: bool,
    },
    /// Inspect the chronological, read-only project workflow history.
    WorkflowHistory {
        #[arg(long)]
        json: bool,
    },
    /// Start and automatically continue a persisted end-to-end workflow.
    Orchestrate {
        objective: String,
        /// Automatically accept tasks after authoritative validation and review PASS.
        #[arg(long)]
        auto_accept: bool,
        /// Require a user gate after Lead approves a Planner plan.
        #[arg(long)]
        user_plan_approval: bool,
    },
    /// Continue the active persisted workflow, or a specific workflow ID.
    Continue {
        id: Option<i64>,
    },
    /// Resolve a persisted workflow user/acceptance gate and continue.
    WorkflowResolve {
        id: i64,
        resolution: String,
    },
    /// Cancel a non-terminal persisted workflow.
    WorkflowCancel {
        id: i64,
        #[arg(short, long)]
        reason: Option<String>,
    },
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
    Plan {
        #[command(subcommand)]
        command: Option<PlanCommand>,
        objective: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
    /// Validate and atomically apply a structured plan response.
    ApplyPlan {
        path: String,
    },
    /// Apply the current approved Planner plan exactly once.
    ApplyApprovedPlan,
    /// Cancel an actionable workflow gate. TARGET is `lead` or `plan-review`.
    Cancel {
        target: String,
        id: i64,
        #[arg(short, long)]
        reason: Option<String>,
    },
    /// Show the deterministic queue of tasks
    Queue {
        /// Explain task readiness, dependency state, and scheduler eligibility
        #[arg(long)]
        explain: bool,
    },
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },
    /// Configure model-cost tiers and inspect persisted economy outcomes.
    Economy {
        #[command(subcommand)]
        command: EconomyCommand,
    },
    Lead {
        #[command(subcommand)]
        command: LeadCommand,
    },
    Ask {
        request: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
    /// Gather the new-project objective and discovery snapshot, then run Lead once.
    NewProject {
        objective: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
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
        #[arg(long, default_value = "1", value_parser = parse_concurrency)]
        concurrency: Concurrency,
    },
    /// Review the latest task run and its worktree changes.
    Review {
        task_id: String,
        #[arg(long)]
        automated: bool,
        #[arg(long, requires = "automated")]
        agent: Option<String>,
        #[arg(long, requires = "automated")]
        model: Option<String>,
        #[arg(long, requires = "automated", value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
        /// Show the complete unified diff.
        #[arg(long, conflicts_with = "file")]
        diff: bool,
        /// Show the unified diff for one changed file.
        #[arg(long, conflicts_with = "diff")]
        file: Option<String>,
        /// Emit the persisted automated review history as JSON.
        #[arg(long, conflicts_with_all = ["diff", "file", "review_id", "full"])]
        history: bool,
        /// Inspect one persisted automated review run by ID.
        #[arg(long, conflicts_with_all = ["diff", "file", "history", "full"])]
        review_id: Option<i64>,
        /// Emit the complete persisted review read model as JSON.
        #[arg(long, conflicts_with_all = ["diff", "file", "history", "review_id"])]
        full: bool,
    },
    /// Run an unrestricted project-wide review using a task's captured evidence.
    ProjectReview {
        task_id: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
    /// Revise a reviewed task using review feedback.
    Revise {
        task_id: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
        feedback: Option<String>,
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
    Approvals {
        #[command(subcommand)]
        command: ApprovalCommand,
    },
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
}

#[derive(Subcommand)]
enum PlanCommand {
    /// Run the configured Planner once for the actionable pending Lead decision.
    Run {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
    /// Revise the current persisted Plan for an actionable pending REVISE_PLAN decision.
    Revise {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
}

#[derive(Subcommand)]
enum TemplateCommand {
    List,
    Set {
        class: orc::execution::ExecutionClass,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
    Clear {
        class: orc::execution::ExecutionClass,
    },
}

#[derive(Subcommand)]
enum ApprovalCommand {
    List,
    Resolve { id: i64 },
}

#[derive(Subcommand)]
enum LeadCommand {
    /// Run one read-only Lead assessment and persist its decision.
    Run {
        request: String,
    },
    Review {
        plan_id: i64,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
    Pending,
    /// Show all persisted Lead decisions without changing workflow state.
    History,
    /// Resolve one USER_DECISION_REQUIRED decision without advancing workflow.
    Resolve {
        id: i64,
        resolution: String,
    },
    /// Apply the pending DIRECT_TASKS decision and create its tasks atomically.
    Apply,
    Consume,
    Show,
    Set {
        agent: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
    },
    Clear,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    run(cli)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryRoute {
    Interactive,
    OneShot,
    Desktop,
}

fn entry_route(cli: &Cli) -> Result<EntryRoute> {
    match (cli.ui, cli.command.is_some()) {
        (true, true) => anyhow::bail!("--ui cannot be used with a CLI command"),
        (true, false) => Ok(EntryRoute::Desktop),
        (false, false) => Ok(EntryRoute::Interactive),
        (false, true) => Ok(EntryRoute::OneShot),
    }
}

fn run(cli: Cli) -> Result<()> {
    match entry_route(&cli)? {
        EntryRoute::Desktop => return desktop::launch_desktop(&desktop::DetachedDesktopProcess),
        EntryRoute::Interactive => return orc::interactive::run(),
        EntryRoute::OneShot => {}
    }

    match cli
        .command
        .ok_or_else(|| anyhow::anyhow!("a command is required unless --ui is used"))?
    {
        Command::Init => {
            // initialize sqlite DB
            let db = Database::init_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            adoption::ensure_adoption_files(std::path::Path::new(".orc"))?;
            let pid = match db.get_project_id().map_err(|e| anyhow::anyhow!(e))? {
                Some(id) => id,
                None => db.create_project("orc").map_err(|e| anyhow::anyhow!(e))?,
            };
            println!("Initialized Orc DB in {} (project id={})", DB_PATH, pid);
        }
        Command::Adopt { objective } => {
            let root = adoption::adopt(".")?;
            if let Some(objective) = objective {
                let db = Database::open_global(root.join(".orc/orc.db"))
                    .map_err(|e| anyhow::anyhow!(e))?;
                adoption::ensure_default_lead(&db)?;
                let app = orc::app::OrcApp::open_global(root.join(".orc/orc.db"), &root)?;
                let response =
                    app.invoke_persisted_lead_with_required_discovery(&objective, 100)?;
                if response.decision.is_none() {
                    anyhow::bail!("Lead returned no decision for adoption objective");
                }
                println!("{}", serde_json::to_string_pretty(&response)?);
            } else {
                println!("Adopted repository {}", root.display());
            }
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
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            if sync {
                sync_enabled_agents(&db);
            }
            print_agents(&db)?;
        }
        Command::Doctor => print_doctor(&doctor::inspect(".", &doctor::SystemHealthRunner)),
        Command::Status => match orc::app::OrcApp::open_global(DB_PATH, ".") {
            Ok(app) => {
                let project = app.operations().project_name()?;
                if let Some(name) = project {
                    println!("Project: {}", name);
                    let tasks = app.task_operation_summaries()?;
                    println!("Tasks: {}", tasks.len());
                    for task in tasks {
                        println!("{}  {:<18} {}", task.task_id, task.lifecycle, task.title);
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
        Command::WorkflowState { json } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            let state = app.workflow_state()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&state)?);
            } else {
                println!("Position: {}", state.position);
                println!(
                    "Lead decisions: {}  Plans: {}  Plan reviews: {}",
                    state.lead_decisions.len(),
                    state.plans.len(),
                    state.plan_reviews.len()
                );
                println!(
                    "Tasks: {}  Runs: {}  User decisions: {}",
                    state.tasks.len(),
                    state.runs.len(),
                    state.user_decisions.len()
                );
            }
        }
        Command::WorkflowHistory { json } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            let history = app.workflow_history()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&history)?);
            } else {
                for entry in history {
                    println!("{}  {:<24} {}", entry.timestamp, entry.kind, entry.summary);
                }
            }
        }
        Command::Orchestrate {
            objective,
            auto_accept,
            user_plan_approval,
        } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            let policy = orc::workflow::WorkflowPolicy {
                acceptance: if auto_accept {
                    orc::workflow::AcceptancePolicy::Automatic
                } else {
                    orc::workflow::AcceptancePolicy::User
                },
                plan_approval: if user_plan_approval {
                    orc::workflow::ApprovalPolicy::User
                } else {
                    orc::workflow::ApprovalPolicy::Agent
                },
                ..Default::default()
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&app.start_workflow(&objective, policy)?)?
            );
        }
        Command::Continue { id } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            let id = match id {
                Some(id) => id,
                None => app
                    .active_workflow()?
                    .map(|workflow| workflow.id)
                    .ok_or_else(|| anyhow::anyhow!("no active workflow to continue"))?,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&app.continue_workflow(id)?)?
            );
        }
        Command::WorkflowResolve { id, resolution } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&app.resolve_workflow(id, &resolution)?)?
            );
        }
        Command::WorkflowCancel { id, reason } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&app.cancel_workflow(id, reason.as_deref())?)?
            );
        }
        Command::Approvals { command } => {
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let project_id = db
                .get_project_id()?
                .ok_or_else(|| anyhow::anyhow!("no project found"))?;
            match command {
                ApprovalCommand::List => {
                    for request in db
                        .list_approval_requests(project_id)?
                        .into_iter()
                        .filter(|request| !request.resolved)
                    {
                        println!("{}  {}", request.id, request.reason);
                    }
                }
                ApprovalCommand::Resolve { id } => {
                    if !db.resolve_approval_request(project_id, id)? {
                        anyhow::bail!("approval request {id} not found for current project");
                    }
                    println!("Resolved approval request {id}.");
                }
            }
        }
        Command::Report { full } => {
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
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
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
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
            let request = PlanningRequest { protocol_version: PROTOCOL_VERSION, kind: "existing_project".into(), project: Some(project_info), engineering_contract: contract, objective, constraints: vec!["Inspect the repository read-only; do not edit files, database state, or dispatch work.".into()], target_platforms: Vec::new(), stack: Vec::new(), non_goals: vec!["Applying or dispatching the plan".into()], deliverables: vec!["A validated PlanResponse JSON document".into()], definition_of_done: vec!["The plan is complete, dependency-safe, scoped, and ready for human approval.".into()], response_schema: orc::protocol::PlanResponseSchema::v1(), role_boundaries: vec!["Planner analyzes and proposes only; Orc persists changes only after ApplyPlan.".into()], planning_constraints: vec!["Planning must not mutate, create tasks, change lifecycle state, or dispatch agents.".into()], approval_requirements: vec!["Human approval is required before the plan is applied.".into()], current_state: Some(db.planning_project_state()?), full_report, discovery_snapshot: Some(orc::discovery::build_snapshot(&root)?) };
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
            for task in &response.tasks {
                for context_file in &task.context_files {
                    if !std::path::Path::new(context_file).exists() {
                        eprintln!(
                            "Warning: task '{}' declares non-existent context file '{}'",
                            task.local_id, context_file
                        );
                    }
                }
            }
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let project_id = db
                .get_project_id()?
                .ok_or_else(|| anyhow::anyhow!("no project found"))?;
            let mapping = db.apply_plan(project_id, &response)?;
            println!("{}", serde_json::to_string_pretty(&mapping)?);
        }
        Command::ApplyApprovedPlan => {
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let project_id = db
                .get_project_id()?
                .ok_or_else(|| anyhow::anyhow!("no project found"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&db.apply_approved_plan(project_id)?)?
            );
        }
        Command::Cancel { target, id, reason } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            match target.as_str() {
                "lead" => println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &app.cancel_lead_decision(id, reason.as_deref())?
                    )?
                ),
                "plan-review" => {
                    app.cancel_plan_review(id, reason.as_deref())?;
                    println!("Cancelled plan review {id}.");
                }
                _ => {
                    anyhow::bail!("unknown cancellation target '{target}'; use lead or plan-review")
                }
            }
        }
        Command::Plan {
            command,
            objective,
            agent,
            model,
            effort,
        } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            if let Some(objective) = objective {
                let mut request = app.planning_request()?;
                request.objective = objective;
                let (_, response) = app.automated_plan(
                    &request,
                    &orc::automated::ActionOverrides {
                        agent_id: agent,
                        model,
                        reasoning_effort: effort,
                    },
                )?;
                println!("{}", serde_json::to_string_pretty(&response)?);
                return Ok(());
            }
            let command = command.ok_or_else(|| anyhow::anyhow!("usage: orc plan run|revise"))?;
            let (result, operation) = match command {
                PlanCommand::Run {
                    agent,
                    model,
                    effort,
                } => (
                    app.run_pending_plan(&orc::automated::ActionOverrides {
                        agent_id: agent,
                        model,
                        reasoning_effort: effort,
                    })?,
                    "persisted",
                ),
                PlanCommand::Revise {
                    agent,
                    model,
                    effort,
                } => (
                    app.run_pending_plan_revision(&orc::automated::ActionOverrides {
                        agent_id: agent,
                        model,
                        reasoning_effort: effort,
                    })?,
                    "revised and persisted",
                ),
            };
            println!(
                "Plan {} {} ({} tasks, source Lead decision {}, Planner run {}).",
                result.plan_id,
                operation,
                result.task_count,
                result.lead_decision_id,
                result.planner_run_id
            );
        }
        Command::Queue { explain } => match orc::app::OrcApp::open_global(DB_PATH, ".") {
            Ok(app) => {
                let report = app.operations().project_queue()?;
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
        Command::Template { command } => {
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            match command {
                TemplateCommand::List => {
                    for (class, template) in db.execution_templates()? {
                        println!(
                            "{} model={} effort={}",
                            class.as_str(),
                            template.model.as_deref().unwrap_or("-"),
                            template.reasoning_effort.map(|v| v.as_str()).unwrap_or("-")
                        );
                    }
                }
                TemplateCommand::Set {
                    class,
                    model,
                    effort,
                } => {
                    if model.is_none() && effort.is_none() {
                        anyhow::bail!("set requires --model or --effort")
                    }
                    db.set_execution_template(class, model.as_deref(), effort)?;
                }
                TemplateCommand::Clear { class } => db.clear_execution_template(class)?,
            }
        }
        Command::Economy { command } => orc::cli::economy::run(command, DB_PATH, ".")?,
        Command::Lead { command } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            match command {
                LeadCommand::Review {
                    plan_id,
                    agent,
                    model,
                    effort,
                } => {
                    let review = app.review_plan_with_backend(
                        plan_id,
                        &orc::automated::ActionOverrides {
                            agent_id: agent,
                            model,
                            reasoning_effort: effort,
                        },
                        &orc::automated::WorkerActionBackend::new(std::path::Path::new(".")),
                    )?;
                    println!("{}", serde_json::to_string_pretty(&review)?);
                }
                LeadCommand::Run { request } => {
                    let (run_id, response) =
                        app.automated_lead(&request, &orc::automated::ActionOverrides::default())?;
                    let decision = response.decision.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("Lead provider response did not contain a decision")
                    })?;
                    println!("Lead assessment complete (run {run_id}).");
                    println!("Decision: {:?}", decision.kind);
                    println!("{}", response.turn.content);
                    if !response.proposals.is_empty() {
                        println!(
                            "Proposals persisted: {} (pending operator review).",
                            response.proposals.len()
                        );
                    }
                }
                LeadCommand::Pending => println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &app.pending_lead_decision()?
                            .map(lead_decision_json)
                            .transpose()?
                    )?
                ),
                LeadCommand::History => {
                    let decisions = app
                        .lead_decisions()?
                        .into_iter()
                        .map(lead_decision_json)
                        .collect::<Result<Vec<_>, _>>()?;
                    println!("{}", serde_json::to_string_pretty(&decisions)?)
                }
                LeadCommand::Resolve { id, resolution } => println!(
                    "{}",
                    serde_json::to_string_pretty(&lead_decision_json(
                        app.resolve_user_decision(id, &resolution)?
                    )?)?
                ),
                LeadCommand::Apply => println!(
                    "{}",
                    serde_json::to_string_pretty(&app.apply_pending_lead_decision()?)?
                ),
                LeadCommand::Consume => println!(
                    "{}",
                    serde_json::to_string_pretty(&app.consume_pending_lead_decision()?)?
                ),
                LeadCommand::Show => match app.lead_provider_config()? {
                    Some(config) => println!(
                        "agent={} model={} effort={}",
                        config.agent_id,
                        config.model.as_deref().unwrap_or("-"),
                        config.reasoning_effort.map(|v| v.as_str()).unwrap_or("-")
                    ),
                    None => println!("Lead is not configured."),
                },
                LeadCommand::Set {
                    agent,
                    model,
                    effort,
                } => {
                    app.set_lead_provider_config(orc::lead::LeadProviderConfig {
                        agent_id: agent,
                        model,
                        reasoning_effort: effort,
                    })?;
                    println!("Configured Lead provider.");
                }
                LeadCommand::Clear => {
                    app.clear_lead_provider_config()?;
                    println!("Cleared Lead provider configuration.");
                }
            }
        }
        Command::Ask {
            request,
            agent,
            model,
            effort,
        } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            let (_, response) = app.automated_lead(
                &request,
                &orc::automated::ActionOverrides {
                    agent_id: agent,
                    model,
                    reasoning_effort: effort,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::NewProject {
            objective,
            agent,
            model,
            effort,
        } => {
            if objective.trim().is_empty() {
                anyhow::bail!("new-project objective must not be empty");
            }
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            let (run_id, response) = app.new_project_intake(
                objective.trim(),
                &orc::automated::ActionOverrides {
                    agent_id: agent,
                    model,
                    reasoning_effort: effort,
                },
            )?;
            let decision = response.decision.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Lead provider response did not contain a decision")
            })?;
            println!("New-project intake complete (run {run_id}).");
            println!("Decision: {:?}", decision.kind);
            println!("{}", response.turn.content);
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
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;

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
            let summaries = agent::dispatch_queue(match concurrency {
                Concurrency::Auto => None,
                Concurrency::Limited(limit) => Some(limit),
            })?;
            let mut dispatched = 0;
            let mut failed = 0;
            for (task_id, outcome) in &summaries {
                match outcome {
                    Ok(summary) => {
                        dispatched += 1;
                        println!("{}", orc::review::format_dispatch(summary));
                        if summary.run_status == "completed" {
                            sync_enabled_agents_after_automated_run(&summary.task.id);
                        }
                    }
                    Err(error) => {
                        failed += 1;
                        eprintln!("Dispatch failed for task {}: {}", task_id, error);
                    }
                }
            }
            println!(
                "Dispatched {} task(s); failed {} task(s).",
                dispatched, failed
            );
        }
        Command::Review {
            task_id,
            automated,
            agent,
            model,
            effort,
            diff,
            file,
            history,
            review_id,
            full,
        } => {
            if automated {
                let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
                let (_, result) = app.automated_review(
                    &task_id,
                    &orc::automated::ActionOverrides {
                        agent_id: agent,
                        model,
                        reasoning_effort: effort,
                    },
                )?;
                println!("{}", serde_json::to_string_pretty(&result)?);
                return Ok(());
            }
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            if let Some(run_id) = review_id {
                let review = app.review_for_run(&task_id, run_id)?;
                println!("{}", serde_json::to_string_pretty(&review)?);
                return Ok(());
            }
            if history {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&app.review_history(&task_id)?)?
                );
                return Ok(());
            }
            let review = app.review(&task_id)?;
            if full {
                println!("{}", serde_json::to_string_pretty(&review)?);
                return Ok(());
            }
            let output = match file {
                Some(path) => orc::review::format_review_file(&review, &path)?,
                None => orc::review::format_review_with_diff(
                    &review,
                    diff.then_some(review.changes.diff.as_str()),
                ),
            };
            println!("{output}");
        }
        Command::ProjectReview {
            task_id,
            agent,
            model,
            effort,
        } => {
            let app = orc::app::OrcApp::open_global(DB_PATH, ".")?;
            let (_, result) = app.automated_project_review_with_backend(
                &task_id,
                &orc::automated::ActionOverrides {
                    agent_id: agent,
                    model,
                    reasoning_effort: effort,
                },
                &orc::automated::WorkerActionBackend::new("."),
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Revise {
            task_id,
            agent,
            model,
            effort,
            feedback,
        } => {
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let task = db
                .get_task(&task_id)?
                .ok_or_else(|| anyhow::anyhow!("task not found"))?;
            let explicit_agent = agent.is_some();
            let selected_id = if let Some(id) = agent.as_ref() {
                id.clone()
            } else {
                let run = db
                    .list_agent_runs_for_task(&task_id)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("task has no agent run"))?;
                run.agent
            };
            let selected_mode = db
                .get_agent(&selected_id)?
                .ok_or_else(|| anyhow::anyhow!("agent '{selected_id}' not found"))?
                .execution_mode;
            if selected_mode == registry::MANUAL {
                if model.is_some() || effort.is_some() {
                    anyhow::bail!("--model and --effort require an automated revision agent");
                }
                let decision = orc::scheduler::resolve_task_economy(
                    &db,
                    &task,
                    registry::AgentAction::Code,
                    orc::scheduler::EconomyOverrides {
                        agent_id: explicit_agent.then_some(selected_id.clone()),
                        ..Default::default()
                    },
                    Some(registry::MANUAL),
                    (!explicit_agent).then_some(selected_id),
                    task.reasoning_effort,
                    Some("revision_contract".into()),
                    orc::scheduler::TransportEligibility::Strict,
                    None,
                    "cli_manual_revision",
                )?;
                let selected = decision
                    .resolution
                    .ok_or_else(|| anyhow::anyhow!(decision.schedule.explanation))?
                    .agent;
                orc::agent::revise_manual(
                    &task_id,
                    feedback.as_deref().unwrap_or(""),
                    &selected,
                    &db,
                    ".",
                )?;
            } else {
                let revision_overrides = orc::agent::RevisionExecutionOverrides { model, effort };
                let factory = |agent: &registry::AgentDefinition, model, effort| {
                    orc::backend::WorkerFactory::build_with_overrides(agent, model, effort)
                };
                let summary = if explicit_agent {
                    orc::agent::revise_with_factory_and_global_db_as_with_runner(
                        &task_id,
                        feedback.as_deref().unwrap_or(""),
                        DB_PATH,
                        ".",
                        &selected_id,
                        &orc::SystemValidationRunner,
                        &revision_overrides,
                        factory,
                    )?
                } else {
                    orc::agent::revise_with_factory_and_global_db_as_constrained_with_runner(
                        &task_id,
                        feedback.as_deref().unwrap_or(""),
                        DB_PATH,
                        ".",
                        &selected_id,
                        &orc::SystemValidationRunner,
                        &revision_overrides,
                        factory,
                    )?
                };
                println!("{}", orc::review::format_dispatch(&summary));
                sync_enabled_agents_after_automated_run(&task_id);
            }
        }
        Command::Schedule {
            task_id,
            explain,
            mode,
        } => {
            let db = Database::open_global(DB_PATH).map_err(|e| anyhow::anyhow!(e))?;
            let task = db
                .get_task(&task_id)
                .map_err(|e| anyhow::anyhow!(e))?
                .ok_or_else(|| anyhow::anyhow!("task '{}' not found in DB", task_id))?;
            let decision = orc::scheduler::resolve_task_economy(
                &db,
                &task,
                orc::registry::AgentAction::Code,
                orc::scheduler::EconomyOverrides::default(),
                mode.as_deref(),
                None,
                task.reasoning_effort,
                Some("task_contract".into()),
                orc::scheduler::TransportEligibility::Strict,
                None,
                "cli_schedule",
            )
            .map_err(|e| anyhow::anyhow!(e))?
            .schedule;
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
        Command::Run { command } => orc::cli::run::run(command, DB_PATH)?,
        Command::Agent { command } => orc::cli::agent::run(command, DB_PATH)?,
        Command::Task { command } => orc::cli::task::run(command, DB_PATH)?,
        Command::Runs { task_id } => match orc::app::OrcApp::open_global(DB_PATH, ".") {
            Ok(app) => {
                let runs = app
                    .operations()
                    .execution_summaries(task_id.as_deref(), 50)?;

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
                        if run.is_active {
                            println!("  Phase:    {}", run.phase.as_deref().unwrap_or("unknown"));
                            println!("  Elapsed:  {}", orc::format::elapsed(&run.started_at));
                            println!("  Activity: {}", orc::format::timestamp(&run.last_activity));
                        }
                        if let Some(finished) = run.finished_at {
                            println!("  Started:  {}", orc::format::timestamp(&run.started_at));
                            println!("  Finished: {}", orc::format::timestamp(&finished));
                        } else {
                            println!("  Started: {}", orc::format::timestamp(&run.started_at));
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

#[allow(dead_code)]
fn update_agent_enabled(db: &Database, id: &str, enabled: bool) -> Result<()> {
    ensure_agent_updated(
        db.set_agent_enabled(id, enabled)
            .map_err(|e| anyhow::anyhow!(e))?,
        id,
    )
}

#[allow(dead_code)]
fn ensure_agent_updated(changed: bool, id: &str) -> Result<()> {
    if !changed {
        anyhow::bail!("agent '{}' is not registered", id);
    }
    Ok(())
}

fn sync_enabled_agents(db: &Database) {
    match db.list_agents() {
        Ok(agents) => {
            for agent in agents.into_iter().filter(|agent| {
                agent.enabled && orc::backend::provider_supports_quota(&agent.backend)
            }) {
                match orc::backend::sync_agent_quota(db, &agent) {
                    Ok(snapshot) => print_synced_quota(&agent.id, &snapshot),
                    Err(error) => eprintln!("{}: quota sync failed: {error}", agent.id),
                }
            }
        }
        Err(error) => eprintln!("quota sync failed: {error}"),
    }
}

fn sync_enabled_agents_after_automated_run(task_id: &str) {
    match Database::open_global(DB_PATH) {
        Ok(db) => {
            let automated_agent = db
                .list_agent_runs_for_task(task_id)
                .ok()
                .and_then(|runs| {
                    runs.into_iter()
                        .find(|run| run.execution_mode == registry::AUTOMATED)
                })
                .map(|run| run.agent);
            if let Some(agent_id) = automated_agent {
                match db.get_agent(&agent_id) {
                    Ok(Some(agent))
                        if agent.enabled
                            && orc::backend::provider_supports_quota(&agent.backend)
                            && agent.profile_path.is_some() =>
                    {
                        if let Err(error) = orc::backend::sync_agent_quota(&db, &agent) {
                            eprintln!("{}: quota sync failed: {error}", agent.id);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => eprintln!("quota sync failed: {error}"),
                }
            }
        }
        Err(error) => eprintln!("quota sync failed: {error}"),
    }
}

fn print_synced_quota(id: &str, snapshot: &orc::backend::ProviderQuotaSnapshot) {
    println!("{id}:");
    println!("  remaining: {}%", snapshot.remaining_percent);
    println!(
        "  reset: {}",
        snapshot
            .reset_at
            .map(|value| orc::format::timestamp(&value.to_string()))
            .unwrap_or_else(|| "unknown".to_owned())
    );
    println!("  effective limit: {}", snapshot.limits.effective);
    print_quota_limit("  5-hour / primary", snapshot.limits.primary.as_ref());
    print_quota_limit("  weekly / secondary", snapshot.limits.secondary.as_ref());
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
                .map(|value| orc::format::timestamp(&value.to_string()))
                .unwrap_or_else(|| "unknown".into())
        ),
        None => println!("{label:<20} -"),
    }
}

fn print_agents(db: &Database) -> Result<()> {
    println!(
        "{:<18} {:<9} {:<10} {:<12} {:<10} {:<7} {:<20} {:<16} {:<20} {:<20} AUTH",
        "ID",
        "BACKEND",
        "MODE",
        "STATUS",
        "PRIORITY",
        "QUOTA",
        "RESET",
        "ROLES",
        "CAPABILITIES",
        "PERMISSIONS"
    );
    for agent in db.list_agents().map_err(|e| anyhow::anyhow!(e))? {
        let permissions = db
            .agent_permissions(&agent.id)
            .map_err(|e| anyhow::anyhow!(e))?;
        let authenticated = db
            .agent_authorization(&agent.id)
            .map_err(|e| anyhow::anyhow!(e))?
            .is_some_and(|value| value.authenticated);
        println!(
            "{:<18} {:<9} {:<10} {:<12} {:<10} {:<7} {:<20} {:<16} {:<20} {:<20} {}",
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
                .map(orc::format::timestamp)
                .unwrap_or_else(|| "-".into()),
            agent
                .actions
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(","),
            agent.capabilities.join(","),
            permissions
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(","),
            authenticated
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
    println!("\nActive tasks");
    if report.active_tasks.is_empty() {
        println!("  (none)");
    } else {
        for task in &report.active_tasks {
            println!(
                "  {}  run: {}  started: {}",
                task.task_id,
                task.run_status,
                orc::format::timestamp(&task.started_at)
            );
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

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn command_variant(command: &Command) -> &'static str {
        match command {
            Command::Init => "Init",
            Command::Adopt { .. } => "Adopt",
            Command::DiscoveryRequest => "DiscoveryRequest",
            Command::ApplyDiscovery { .. } => "ApplyDiscovery",
            Command::Agents { .. } => "Agents",
            Command::Doctor => "Doctor",
            Command::Status => "Status",
            Command::WorkflowState { .. } => "WorkflowState",
            Command::WorkflowHistory { .. } => "WorkflowHistory",
            Command::Orchestrate { .. } => "Orchestrate",
            Command::Continue { .. } => "Continue",
            Command::WorkflowResolve { .. } => "WorkflowResolve",
            Command::WorkflowCancel { .. } => "WorkflowCancel",
            Command::Report { .. } => "Report",
            Command::PlanRequest { .. } => "PlanRequest",
            Command::Plan { .. } => "Plan",
            Command::ApplyPlan { .. } => "ApplyPlan",
            Command::ApplyApprovedPlan => "ApplyApprovedPlan",
            Command::Cancel { .. } => "Cancel",
            Command::Queue { .. } => "Queue",
            Command::Template { .. } => "Template",
            Command::Economy { .. } => "Economy",
            Command::Lead { .. } => "Lead",
            Command::Ask { .. } => "Ask",
            Command::NewProject { .. } => "NewProject",
            Command::ApplyResponse { .. } => "ApplyResponse",
            Command::Dispatch { .. } => "Dispatch",
            Command::DispatchQueue { .. } => "DispatchQueue",
            Command::Review { .. } => "Review",
            Command::ProjectReview { .. } => "ProjectReview",
            Command::Revise { .. } => "Revise",
            Command::Schedule { .. } => "Schedule",
            Command::Agent { .. } => "Agent",
            Command::Task { .. } => "Task",
            Command::Runs { .. } => "Runs",
            Command::Approvals { .. } => "Approvals",
            Command::Run { .. } => "Run",
        }
    }

    #[test]
    fn parses_ui_without_a_subcommand() {
        let cli = Cli::try_parse_from(["orc", "--ui"]).unwrap();
        assert!(cli.ui);
        assert!(cli.command.is_none());
    }

    #[test]
    fn preserves_existing_subcommands() {
        let cli = Cli::try_parse_from(["orc", "status"]).unwrap();
        assert!(!cli.ui);
        assert!(matches!(cli.command, Some(Command::Status)));
    }

    #[test]
    fn parses_operator_cancellation_with_reason() {
        let cli =
            Cli::try_parse_from(["orc", "cancel", "plan-review", "42", "--reason", "stop now"])
                .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Cancel { target, id: 42, reason: Some(reason) })
                if target == "plan-review" && reason == "stop now"
        ));
    }

    #[test]
    fn parses_read_only_workflow_state_inspection() {
        let cli = Cli::try_parse_from(["orc", "workflow-state", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::WorkflowState { json: true })
        ));
        let alias = Cli::try_parse_from(["orc", "workflow"]).unwrap();
        assert!(matches!(
            alias.command,
            Some(Command::WorkflowState { json: false })
        ));
    }

    #[test]
    fn parses_revise_execution_overrides_and_optional_feedback() {
        let cli = Cli::try_parse_from([
            "orc",
            "revise",
            "T-0001",
            "--model",
            "gpt-test",
            "--effort",
            "high",
            "--agent",
            "coder",
            "address the failing check",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Revise {
                task_id,
                agent,
                model,
                effort,
                feedback,
            }) => {
                assert_eq!(task_id, "T-0001");
                assert_eq!(agent.as_deref(), Some("coder"));
                assert_eq!(model.as_deref(), Some("gpt-test"));
                assert_eq!(effort, Some(registry::ReasoningEffort::High));
                assert_eq!(feedback.as_deref(), Some("address the failing check"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_revise_without_execution_overrides() {
        let cli = Cli::try_parse_from(["orc", "revise", "T-0001", "operator feedback"]).unwrap();
        match cli.command {
            Some(Command::Revise {
                task_id,
                agent,
                model,
                effort,
                feedback,
            }) => {
                assert_eq!(task_id, "T-0001");
                assert_eq!(agent, None);
                assert_eq!(model, None);
                assert_eq!(effort, None);
                assert_eq!(feedback.as_deref(), Some("operator feedback"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn rejects_ui_with_a_subcommand() {
        let cli = Cli::try_parse_from(["orc", "--ui", "status"]).unwrap();
        let error = run(cli).unwrap_err().to_string();
        assert!(error.contains("--ui cannot be used with a CLI command"));
    }

    #[test]
    fn every_explicit_command_routes_one_shot_without_entering_interactive_mode() {
        let cases: &[(&[&str], EntryRoute, Option<&str>)] = &[
            (&["orc"], EntryRoute::Interactive, None),
            (&["orc", "--ui"], EntryRoute::Desktop, None),
            (&["orc", "init"], EntryRoute::OneShot, Some("Init")),
            (&["orc", "adopt"], EntryRoute::OneShot, Some("Adopt")),
            (
                &["orc", "discovery-request"],
                EntryRoute::OneShot,
                Some("DiscoveryRequest"),
            ),
            (
                &["orc", "apply-discovery", "discovery.json"],
                EntryRoute::OneShot,
                Some("ApplyDiscovery"),
            ),
            (&["orc", "agents"], EntryRoute::OneShot, Some("Agents")),
            (&["orc", "doctor"], EntryRoute::OneShot, Some("Doctor")),
            (&["orc", "status"], EntryRoute::OneShot, Some("Status")),
            (&["orc", "report"], EntryRoute::OneShot, Some("Report")),
            (
                &["orc", "plan-request", "ship it"],
                EntryRoute::OneShot,
                Some("PlanRequest"),
            ),
            (
                &["orc", "plan", "ship it"],
                EntryRoute::OneShot,
                Some("Plan"),
            ),
            (
                &["orc", "apply-plan", "plan.json"],
                EntryRoute::OneShot,
                Some("ApplyPlan"),
            ),
            (&["orc", "queue"], EntryRoute::OneShot, Some("Queue")),
            (
                &["orc", "template", "list"],
                EntryRoute::OneShot,
                Some("Template"),
            ),
            (
                &["orc", "economy", "show"],
                EntryRoute::OneShot,
                Some("Economy"),
            ),
            (&["orc", "lead", "show"], EntryRoute::OneShot, Some("Lead")),
            (&["orc", "ask", "help"], EntryRoute::OneShot, Some("Ask")),
            (
                &["orc", "apply-response", "response.json"],
                EntryRoute::OneShot,
                Some("ApplyResponse"),
            ),
            (
                &["orc", "dispatch", "T-0001"],
                EntryRoute::OneShot,
                Some("Dispatch"),
            ),
            (
                &["orc", "dispatch-queue"],
                EntryRoute::OneShot,
                Some("DispatchQueue"),
            ),
            (
                &["orc", "review", "T-0001"],
                EntryRoute::OneShot,
                Some("Review"),
            ),
            (
                &["orc", "project-review", "T-0001"],
                EntryRoute::OneShot,
                Some("ProjectReview"),
            ),
            (
                &["orc", "revise", "T-0001", "address feedback"],
                EntryRoute::OneShot,
                Some("Revise"),
            ),
            (
                &["orc", "schedule", "T-0001"],
                EntryRoute::OneShot,
                Some("Schedule"),
            ),
            (
                &["orc", "agent", "list"],
                EntryRoute::OneShot,
                Some("Agent"),
            ),
            (&["orc", "task", "list"], EntryRoute::OneShot, Some("Task")),
            (&["orc", "runs"], EntryRoute::OneShot, Some("Runs")),
            (
                &["orc", "approvals", "list"],
                EntryRoute::OneShot,
                Some("Approvals"),
            ),
            (
                &["orc", "run", "fail", "1"],
                EntryRoute::OneShot,
                Some("Run"),
            ),
        ];

        let covered_commands = cases
            .iter()
            .filter_map(|(_, _, command)| *command)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(covered_commands.len(), 28);

        for &(arguments, expected_route, expected_command) in cases {
            let cli = Cli::try_parse_from(arguments).unwrap_or_else(|error| {
                panic!("failed to parse {arguments:?}: {error}");
            });
            assert_eq!(
                entry_route(&cli).unwrap(),
                expected_route,
                "unexpected route for {arguments:?}"
            );
            assert_eq!(
                cli.command.as_ref().map(command_variant),
                expected_command,
                "unexpected command variant for {arguments:?}"
            );
        }
    }
}
