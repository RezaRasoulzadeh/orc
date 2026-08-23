use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::thread;

use crate::backend::WorkerFactory;
use crate::contract;
use crate::git;
use crate::queue::QueueEntry;
use crate::registry::{self, AgentDefinition};
use crate::review::DispatchSummary;
use crate::storage::Database;
use crate::task::{Task, TaskScopeMode, TaskStatus};
use crate::validation::{
    self, SystemValidationRunner, ValidationConfig, ValidationReport, ValidationRunner,
};
use crate::worker::{Worker, WorkerOutcome};

const ENGINEERING_CONTRACT_PATH: &str = ".orc/engineering.md";
const ARCHITECTURE_DECISION_MARKER: &str = "ORC-ARCHITECTURE-DECISION:";

fn architecture_decisions(output: &str) -> Vec<&str> {
    let mut decisions = Vec::new();
    let mut reported = HashSet::new();
    for line in output.lines() {
        let Some(decision) = line.strip_prefix(ARCHITECTURE_DECISION_MARKER) else {
            continue;
        };
        let decision = decision.trim();
        if !decision.is_empty() && reported.insert(decision) {
            decisions.push(decision);
        }
    }
    decisions
}

fn block_automated_run(db: &Database, run_id: i64, task_id: &str, output: &str) -> Result<()> {
    db.update_agent_run_status(run_id, "failed", Some(output))
        .context("failed to update agent run status to failed")?;
    db.update_task_status(task_id, TaskStatus::Blocked)
        .context("failed to set task status to blocked")?;
    Ok(())
}

fn build_worker_prompt(contract: &str, project: &str, task: &Task) -> String {
    let guidance = match task.scope_mode {
        Some(TaskScopeMode::Focused) => format!("\n\n## Targeted Context\n\nRelevant implementation context has already been identified.\n\nRead these files first:\n{}\n\nExpected changes:\n{}\n\nDo not perform broad repository discovery unless this scope proves insufficient. If additional files are necessary, inspect only the minimum required and report which ones were needed.", task.context_files.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"), task.expected_changes.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")),
        Some(TaskScopeMode::Module) => format!("\n\n## Targeted Context\n\nSupplied modules/directories:\n{}\nConstrain discovery to these areas where practical.\n\nExpected changes:\n{}", task.context_files.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"), task.expected_changes.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")),
        Some(TaskScopeMode::Project) => "\n\n## Targeted Context\n\nProject scope is configured; broader repository inspection is allowed.".into(),
        None => String::new(),
    };
    let objective = format!("{}{}", task.objective, guidance);
    format!(
        "# Engineering Contract\n\n{contract}\n\n---\n\n# Task\n\nProject: {project}\nTask ID: {id}\nTitle: {title}\nObjective: {objective}\nRole: {role}\n\nInspect the repository rooted at the current working directory and implement ONLY the changes required to complete this single task. Run any relevant checks and tests (e.g., cargo test) and fix failures you encounter. Do not modify unrelated files or change task status. Stop after completing the task and summarize what you changed and any follow-up steps.\n",
        contract = contract,
        project = project,
        id = task.id,
        title = task.title,
        objective = objective,
        role = task.role,
    )
}

pub fn build_manual_packet(contract: &str, project: &str, task: &Task, agent_id: &str) -> String {
    let guidance = match task.scope_mode {
        Some(TaskScopeMode::Focused) => format!(
            "\n\nRelevant implementation context has already been identified. Read these files first:\n{}\nExpected changes:\n{}\nDo not perform broad repository discovery unless this scope proves insufficient.",
            task.context_files
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n"),
            task.expected_changes
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        Some(TaskScopeMode::Module) => {
            "\n\nConstrain discovery to the supplied modules/directories where practical.".into()
        }
        Some(TaskScopeMode::Project) => {
            "\n\nProject scope allows broader repository inspection.".into()
        }
        None => String::new(),
    };
    let objective = format!("{}{}", task.objective, guidance);
    format!(
        "# Orc Manual Task Packet\n\nAgent ID: {agent_id}\nProject: {project}\n\n## Engineering Contract\n\n{contract}\n\n## Task\n\nTask ID: {id}\nTitle: {title}\nObjective: {objective}\nRole: {role}\n\n## Constraints\n\nStay strictly inside this task's scope. Do not modify unrelated project work or assume access to credentials, private memory, or external systems.\n\n## Required validation\n\nDescribe the checks and tests you performed. If you could not run a check, say why.\n\n## Required response / handoff format\n\nSummarize changes or recommendations, list files affected (if any), report validation results, and identify follow-up risks or questions.\n",
        id = task.id,
        title = task.title,
        objective = objective,
        role = task.role
    )
}

/// Build a worker prompt with engineering contract and task information.
pub fn build_worker_prompt_for_testing(contract: &str, project: &str, task: &Task) -> String {
    build_worker_prompt(contract, project, task)
}

/// Dispatch a task for execution using the provided worker and custom DB path.
/// This is the internal implementation that handles the full lifecycle.
/// For testing purposes, accepts a custom db_path parameter.
pub fn dispatch_with_worker_and_db(
    task_id: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
) -> Result<()> {
    dispatch_with_worker_and_db_as(task_id, worker, db_path, repo_path, "copilot").map(|_| ())
}

pub fn dispatch_with_worker_and_db_as(
    task_id: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
) -> Result<()> {
    dispatch_with_worker_and_db_as_with_runner(
        task_id,
        worker,
        db_path,
        repo_path,
        agent_id,
        &SystemValidationRunner,
    )
    .map(|_| ())
}

pub fn dispatch_with_worker_and_db_as_with_runner(
    task_id: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
) -> Result<DispatchSummary> {
    let db = Database::open(db_path)
        .with_context(|| format!("failed to open orc DB ({}); run `orc init` first", db_path))?;
    dispatch_with_worker_on_db(task_id, worker, &db, repo_path, agent_id, validation_runner)
}

pub fn dispatch_with_worker_on_db(
    task_id: &str,
    worker: &dyn Worker,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
) -> Result<DispatchSummary> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;

    let project_id = db
        .get_project_id()
        .with_context(|| "failed to read project id from DB")?
        .with_context(|| "no project found in DB")?;

    let project = db
        .get_project_name()
        .with_context(|| "failed to read project name from DB")?;
    let project_name = project.unwrap_or_else(|| "orc".into());

    let task = db
        .get_task(task_id)
        .with_context(|| format!("failed to fetch task '{}' from DB", task_id))?
        .with_context(|| format!("task '{}' not found in DB", task_id))?;

    // Check if task is already active or done
    if task.status == TaskStatus::Active {
        anyhow::bail!("Task {} is already active; cannot dispatch again", task_id);
    }
    if task.status == TaskStatus::Done {
        anyhow::bail!("Task {} is already done; cannot dispatch", task_id);
    }
    if task.status == TaskStatus::Cancelled {
        anyhow::bail!("Task {} is cancelled; cannot dispatch", task_id);
    }

    // Set task status to active
    db.update_task_status(task_id, TaskStatus::Active)
        .with_context(|| "failed to set task status to active")?;

    // Create an agent run
    let run_id = db
        .create_agent_run_with_mode(project_id, task_id, agent_id, registry::AUTOMATED)
        .with_context(|| "failed to create agent run")?;

    // Create a worktree for the task
    let (branch_name, worktree_path) = match git::ensure_worktree(task_id, repo_path) {
        Ok((branch, path)) => (branch, path),
        Err(e) => {
            let error_msg = format!("Failed to create worktree: {}", e);
            block_automated_run(db, run_id, task_id, &error_msg)
                .context("failed to record worktree creation failure")?;
            anyhow::bail!("{}", error_msg);
        }
    };
    let progress = |phase: &str| {
        if let Err(error) = db.update_agent_run_phase(run_id, phase) {
            eprintln!("warning: failed to persist run progress: {error}");
        }
        println!("[orc] {phase}");
    };
    let worker_output = |line: &str| {
        if let Err(error) = db.record_worker_output(run_id, line) {
            eprintln!("warning: failed to persist worker output: {error}");
        }
        println!("[orc] worker output: {line}");
    };
    progress("worktree prepared");

    // Store worktree metadata
    if let Err(e) = db.store_worktree_metadata(
        run_id,
        task_id,
        &branch_name,
        &worktree_path.to_string_lossy(),
    ) {
        let error_msg = format!("Failed to store worktree metadata: {}", e);
        block_automated_run(db, run_id, task_id, &error_msg)
            .context("failed to record worktree metadata failure")?;
        anyhow::bail!("{}", error_msg);
    }

    let prompt = build_worker_prompt(&contract, &project_name, &task);

    // Execute the worker in the worktree directory
    let worktree_dir = repo_path.join(&worktree_path);
    progress("worker spawned");
    progress("worker running");
    match worker.execute_with_progress_and_usage(&prompt, &worktree_dir, &|line| {
        worker_output(line);
    }) {
        Ok(execution) => {
            let outcome = execution.outcome;
            let output = execution.output;
            let token_usage = execution.token_usage;
            match outcome {
                WorkerOutcome::Success => {
                    progress("worker completed");
                    let changes = match git::inspect_worktree(&worktree_dir, repo_path) {
                        Ok(changes) => changes,
                        Err(error) => {
                            let output = format!(
                                "{}\n\nPost-worker inspection failed: {error:#}",
                                output.as_deref().unwrap_or_default()
                            );
                            db.update_agent_run_status_with_usage(
                                run_id,
                                "failed",
                                Some(&output),
                                token_usage,
                            )?;
                            db.update_task_status(task_id, TaskStatus::Blocked)?;
                            anyhow::bail!("could not inspect task worktree after worker completion")
                        }
                    };
                    if changes.files.is_empty() {
                        let output = format!(
                            "{}\n\nDispatch result: no meaningful project changes.",
                            output.as_deref().unwrap_or_default()
                        );
                        db.update_agent_run_status_with_usage(
                            run_id,
                            "no_changes",
                            Some(&output),
                            token_usage,
                        )?;
                        db.update_task_status(task_id, TaskStatus::Blocked)?;
                        anyhow::bail!(
                            "worker completed without meaningful project changes; task remains blocked"
                        );
                    }
                    let validation_config = match ValidationConfig::load(&worktree_dir) {
                        Ok(config) => config,
                        Err(error) => {
                            let output = format!(
                                "{}\n\nValidation setup failed: {error:#}",
                                output.as_deref().unwrap_or_default()
                            );
                            db.update_agent_run_status_with_usage(
                                run_id,
                                "failed",
                                Some(&output),
                                token_usage,
                            )?;
                            db.update_task_status(task_id, TaskStatus::Blocked)?;
                            anyhow::bail!("validation setup failed for task {task_id}")
                        }
                    };
                    progress("validation started");
                    let report = match validation::run_validation_pipeline(
                        validation_runner,
                        &validation_config.commands,
                        &worktree_dir,
                    ) {
                        Ok(report) => report,
                        Err(error) => {
                            let output = format!(
                                "{}\n\nValidation execution failed: {error:#}",
                                output.as_deref().unwrap_or_default()
                            );
                            db.update_agent_run_status_with_usage(
                                run_id,
                                "failed",
                                Some(&output),
                                token_usage,
                            )?;
                            db.update_task_status(task_id, TaskStatus::Blocked)?;
                            anyhow::bail!("validation execution failed for task {task_id}")
                        }
                    };
                    progress("validation completed");
                    let validation_summary = report.summary();
                    db.record_lifecycle_event(
                        "validation_result",
                        Some(task_id),
                        Some(run_id),
                        Some(agent_id),
                        Some(if report.is_success() {
                            "success"
                        } else {
                            "failure"
                        }),
                    )?;
                    let combined_output = if validation_summary.is_empty() {
                        output.unwrap_or_default()
                    } else {
                        format!(
                            "{}\n\nValidation:\n{}",
                            output.unwrap_or_default(),
                            validation_summary
                        )
                    };
                    for decision in architecture_decisions(&combined_output) {
                        db.insert_approval_request(project_id, decision)
                            .with_context(
                                || "failed to record architecture decision approval request",
                            )?;
                    }
                    if !report.is_success() {
                        db.update_agent_run_status_with_usage(
                            run_id,
                            "failed",
                            Some(&combined_output),
                            token_usage,
                        )?;
                        db.update_task_status(task_id, TaskStatus::Blocked)?;
                        anyhow::bail!("validation failed for task {task_id}; task remains blocked");
                    }
                    db.update_agent_run_status_with_usage(
                        run_id,
                        "completed",
                        Some(&combined_output),
                        token_usage,
                    )
                    .with_context(|| "failed to update agent run status to completed")?;
                    db.update_task_status(task_id, TaskStatus::Review)
                        .with_context(|| "failed to set task status to review")?;
                    progress("review transition");
                    let task = db
                        .get_task(task_id)?
                        .context("task disappeared after dispatch")?;
                    Ok(DispatchSummary {
                        task,
                        agent: agent_id.to_owned(),
                        backend: "unknown".to_owned(),
                        profile: None,
                        model: None,
                        reasoning_effort: None,
                        worktree_path: worktree_path.display().to_string(),
                        run_id,
                        run_status: "completed".to_owned(),
                        validation: "PASS".to_owned(),
                        changes,
                    })
                }
                WorkerOutcome::Failure(error) => {
                    // Mark agent run as failed and task as blocked
                    let error_msg = format!("Worker failed: {}", error);
                    db.update_agent_run_status_with_usage(
                        run_id,
                        "failed",
                        Some(&error_msg),
                        token_usage,
                    )
                    .with_context(|| "failed to update agent run status to failed")?;
                    db.update_task_status(task_id, TaskStatus::Blocked)
                        .with_context(|| "failed to set task status to blocked")?;
                    anyhow::bail!("{}", error_msg);
                }
            }
        }
        Err(spawn_error) => {
            // Spawn failed, mark run as failed and task as blocked
            db.update_agent_run_status(run_id, "failed", Some(&spawn_error))
                .with_context(|| "failed to update agent run status after spawn failure")?;
            db.update_task_status(task_id, TaskStatus::Blocked)
                .with_context(|| "failed to set task status to blocked after spawn failure")?;
            anyhow::bail!("{}", spawn_error);
        }
    }
}

/// Dispatch a task for execution using the provided worker.
/// Uses the default DB path (.orc/orc.db).
pub fn dispatch_with_worker(task_id: &str, worker: &dyn Worker) -> Result<()> {
    dispatch_with_worker_and_db(task_id, worker, ".orc/orc.db", ".")
}

pub fn revise_with_worker_and_db_as_with_runner(
    task_id: &str,
    feedback: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
) -> Result<DispatchSummary> {
    let db = Database::open(db_path)?;
    revise_with_worker_on_db(
        task_id,
        feedback,
        worker,
        &db,
        repo_path,
        agent_id,
        validation_runner,
    )
}

pub fn revise_with_worker_on_db(
    task_id: &str,
    feedback: &str,
    worker: &dyn Worker,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
) -> Result<DispatchSummary> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    let project_name = db.get_project_name()?.unwrap_or_else(|| "orc".into());
    let task = db.get_task(task_id)?.context("task not found in DB")?;
    if task.status != TaskStatus::Review {
        anyhow::bail!(
            "task {} can only be revised from review (currently {})",
            task_id,
            task.status
        );
    }
    let (_, worktree_path) = db
        .get_worktree_metadata(task_id)?
        .context("task has no worktree")?;
    let worktree_dir = repo_path.join(&worktree_path);
    if !worktree_dir.exists() {
        anyhow::bail!("task worktree does not exist: {}", worktree_dir.display());
    }
    db.update_task_status(task_id, TaskStatus::Active)?;
    let agent = db
        .list_agents()?
        .into_iter()
        .find(|agent| agent.id == agent_id)
        .with_context(|| format!("agent '{}' not found in registry", agent_id))?;
    let resolution = crate::execution::resolve_with_template(
        &task.role,
        &db.execution_template(crate::execution::class_for_role(&task.role))?,
        agent.model.as_deref(),
        agent.reasoning_effort,
        None,
        None,
    );
    let run_id = db.create_agent_run_with_execution(
        project_id,
        task_id,
        agent_id,
        registry::AUTOMATED,
        crate::storage::AgentRunExecution {
            class: resolution.class.as_str(),
            model: resolution.model.as_deref(),
            effort: resolution.reasoning_effort,
            source: &resolution.source,
        },
    )?;
    db.record_lifecycle_event(
        "review_revision",
        Some(task_id),
        Some(run_id),
        Some(agent_id),
        Some(feedback),
    )?;
    let progress = |phase: &str| {
        if let Err(error) = db.update_agent_run_phase(run_id, phase) {
            eprintln!("warning: failed to persist run progress: {error}");
        }
        println!("[orc] {phase}");
    };
    let worker_output = |line: &str| {
        if let Err(error) = db.record_worker_output(run_id, line) {
            eprintln!("warning: failed to persist worker output: {error}");
        }
        println!("[orc] worker output: {line}");
    };
    progress("revision/worker starting");
    let prompt = format!(
        "{}\n\n## Review feedback\n\n{}\n\nRevise the existing implementation in the existing task worktree, then rerun validation.",
        build_worker_prompt(&contract, &project_name, &task),
        feedback
    );
    let fail = |message: String| -> Result<DispatchSummary> {
        progress(if message.to_ascii_lowercase().contains("timeout") {
            "worker timeout"
        } else {
            "revision failed"
        });
        block_automated_run(db, run_id, task_id, &message)?;
        anyhow::bail!("{message}")
    };
    progress("worker running");
    let execution = match worker.execute_with_progress_and_usage(&prompt, &worktree_dir, &|line| {
        worker_output(line);
    }) {
        Ok(result) => result,
        Err(error) => {
            progress("worker failed");
            return fail(error);
        }
    };
    let outcome = execution.outcome;
    let output = execution.output;
    let token_usage = execution.token_usage;
    let fail = |message: String| -> Result<DispatchSummary> {
        progress(if message.to_ascii_lowercase().contains("timeout") {
            "worker timeout"
        } else {
            "revision failed"
        });
        db.update_agent_run_status_with_usage(run_id, "failed", Some(&message), token_usage)?;
        db.update_task_status(task_id, TaskStatus::Blocked)?;
        anyhow::bail!("{message}")
    };
    if let WorkerOutcome::Failure(error) = outcome {
        progress("worker failed");
        return fail(format!("Worker failed: {error}"));
    }
    progress("worker completed");
    let changes = match git::inspect_worktree(&worktree_dir, repo_path) {
        Ok(changes) => changes,
        Err(error) => return fail(format!("Post-worker inspection failed: {error:#}")),
    };
    if changes.files.is_empty() {
        return fail("Revision completed without meaningful project changes.".into());
    }
    let config = match ValidationConfig::load(&worktree_dir) {
        Ok(config) => config,
        Err(error) => return fail(format!("Validation setup failed: {error:#}")),
    };
    progress("validation started");
    let report = match validation::run_validation_pipeline(
        validation_runner,
        &config.commands,
        &worktree_dir,
    ) {
        Ok(report) => report,
        Err(error) => return fail(format!("Validation execution failed: {error:#}")),
    };
    progress("validation completed");
    let summary = report.summary();
    let combined = format!("{}\n\nValidation:\n{}", output.unwrap_or_default(), summary);
    if !report.is_success() {
        return fail(combined);
    }
    db.update_agent_run_status_with_usage(run_id, "completed", Some(&combined), token_usage)?;
    db.record_lifecycle_event(
        "validation_result",
        Some(task_id),
        Some(run_id),
        Some(agent_id),
        Some(if report.is_success() {
            "success"
        } else {
            "failure"
        }),
    )?;
    db.update_task_status(task_id, TaskStatus::Review)?;
    progress("review transition");
    Ok(DispatchSummary {
        task: db
            .get_task(task_id)?
            .context("task disappeared after revision")?,
        agent: agent_id.into(),
        backend: agent.backend,
        profile: agent.profile_path,
        model: resolution.model,
        reasoning_effort: resolution.reasoning_effort,
        worktree_path,
        run_id,
        run_status: "completed".into(),
        validation: "PASS".into(),
        changes,
    })
}

pub fn revise_manual(
    task_id: &str,
    feedback: &str,
    agent: &AgentDefinition,
    db: &Database,
    repo_path: impl AsRef<Path>,
) -> Result<()> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;
    let project = db.get_project_name()?.unwrap_or_else(|| "orc".into());
    let task = db.get_task(task_id)?.context("task not found in DB")?;
    if task.status != TaskStatus::Review {
        anyhow::bail!(
            "task {} can only be revised from review (currently {})",
            task_id,
            task.status
        );
    }
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    db.update_task_status(task_id, TaskStatus::Active)?;
    let run_id = db.create_agent_run_with_mode(project_id, task_id, &agent.id, registry::MANUAL)?;
    db.set_agent_run_waiting_external(run_id)?;
    println!(
        "\n{}\n\n## Review feedback\n\n{}",
        build_manual_packet(&contract, &project, &task, &agent.id),
        feedback
    );
    Ok(())
}

/// Public dispatch function using the Copilot worker and default DB path
pub fn dispatch(task_id: &str) -> Result<()> {
    dispatch_selected(task_id, None)
}

pub fn dispatch_selected(task_id: &str, requested_agent: Option<&str>) -> Result<()> {
    dispatch_selected_with_options(task_id, requested_agent, None, None).map(|summary| {
        println!("{}", crate::review::format_dispatch(&summary));
    })
}

pub fn dispatch_selected_with_summary(
    task_id: &str,
    requested_agent: Option<&str>,
) -> Result<DispatchSummary> {
    dispatch_selected_with_options(task_id, requested_agent, None, None)
}

pub fn dispatch_selected_with_options(
    task_id: &str,
    requested_agent: Option<&str>,
    model_override: Option<String>,
    effort_override: Option<crate::registry::ReasoningEffort>,
) -> Result<DispatchSummary> {
    let db_path = ".orc/orc.db";
    let db = Database::open(db_path)
        .with_context(|| format!("failed to open orc DB ({db_path}); run `orc init` first"))?;
    dispatch_selected_with_db_and_repo(
        &db,
        ".",
        task_id,
        requested_agent,
        model_override,
        effort_override,
    )
}

pub fn dispatch_selected_with_db_and_repo(
    db: &Database,
    repo_path: impl AsRef<Path>,
    task_id: &str,
    requested_agent: Option<&str>,
    model_override: Option<String>,
    effort_override: Option<crate::registry::ReasoningEffort>,
) -> Result<DispatchSummary> {
    let repo_path = repo_path.as_ref();
    let task = db
        .get_task(task_id)?
        .with_context(|| format!("task '{}' not found in DB", task_id))?;
    let agent = if let Some(agent_id) = requested_agent {
        let agent = registry::get_agent(db, agent_id)?;
        crate::scheduler::validate_override(&agent, &task)?;
        agent
    } else {
        let agents = db.list_agents()?;
        let decision = crate::scheduler::schedule(&task, &agents, None)?;
        let selected_id = decision.selected_agent_id.ok_or_else(|| {
            anyhow::anyhow!(
                "no eligible agent found for task '{}': {}",
                task_id,
                decision.explanation
            )
        })?;
        agents
            .into_iter()
            .find(|a| a.id == selected_id)
            .ok_or_else(|| {
                anyhow::anyhow!("selected agent '{}' not found in registry", selected_id)
            })?
    };
    if agent.execution_mode == registry::MANUAL {
        if model_override.is_some() || effort_override.is_some() {
            anyhow::bail!(
                "manual agent '{}' does not support Codex model or reasoning-effort overrides",
                agent.id
            );
        }
        dispatch_manual(task_id, &agent, db, repo_path)?;
        let task = db
            .get_task(task_id)?
            .context("task disappeared after manual dispatch")?;
        let run = db
            .list_agent_runs_for_task(task_id)?
            .into_iter()
            .next()
            .context("manual run missing")?;
        return Ok(DispatchSummary {
            task,
            agent: agent.id,
            backend: agent.backend,
            profile: agent.profile_path,
            model: None,
            reasoning_effort: None,
            worktree_path: "(created when patch is submitted)".into(),
            run_id: run.id,
            run_status: run.status,
            validation: "PENDING".into(),
            changes: Default::default(),
        });
    }
    let resolution = crate::execution::resolve_with_template(
        &task.role,
        &db.execution_template(crate::execution::class_for_role(&task.role))?,
        agent.model.as_deref(),
        agent.reasoning_effort,
        model_override,
        effort_override,
    );
    let model = resolution.model.clone();
    let reasoning_effort = resolution.reasoning_effort;
    let worker = WorkerFactory::build_with_codex_overrides(&agent, model.clone(), reasoning_effort)
        .map_err(anyhow::Error::msg)?;
    let mut summary = dispatch_with_worker_on_db(
        task_id,
        worker.as_ref(),
        db,
        repo_path,
        &agent.id,
        &SystemValidationRunner,
    )?;
    db.set_agent_run_execution(
        summary.run_id,
        resolution.class.as_str(),
        model.as_deref(),
        reasoning_effort,
        &resolution.source,
    )?;
    summary.backend = agent.backend;
    summary.profile = agent.profile_path;
    summary.model = model;
    summary.reasoning_effort = reasoning_effort;
    Ok(summary)
}

pub fn plan_dispatch_assignments(
    ready: &[QueueEntry],
    agents: &[AgentDefinition],
    busy_agents: &HashSet<String>,
    quota_reserve: i64,
    concurrency: Option<usize>,
) -> Result<Vec<(String, String)>> {
    if concurrency == Some(0) {
        anyhow::bail!("concurrency must be greater than zero");
    }
    let mut reserved = busy_agents.clone();
    let mut assignments = Vec::new();
    for entry in ready {
        if concurrency.is_some_and(|limit| assignments.len() == limit) {
            break;
        }
        let decision = crate::scheduler::schedule_with_busy_and_quota_reserve(
            &entry.task,
            agents,
            Some(registry::AUTOMATED),
            &reserved,
            quota_reserve,
        )?;
        if let Some(agent_id) = decision.selected_agent_id {
            reserved.insert(agent_id.clone());
            assignments.push((entry.task.id.clone(), agent_id));
        }
    }
    Ok(assignments)
}

pub type DispatchQueueOutcomes = BTreeMap<String, Result<DispatchSummary, String>>;

pub fn dispatch_queue(concurrency: Option<usize>) -> Result<DispatchQueueOutcomes> {
    let db = Database::open(".orc/orc.db")?;
    let report = crate::queue::compute_queue(&db)?;
    let agents = db.list_agents()?;
    let quota_reserve = db.quota_reserve()?;
    let assignments = plan_dispatch_assignments(
        &report.ready,
        &agents,
        &db.list_busy_agents()?.into_iter().collect::<HashSet<_>>(),
        quota_reserve,
        concurrency,
    )?;
    let mut outcomes = BTreeMap::new();
    let handles = assignments
        .iter()
        .map(|(task_id, agent_id)| {
            let task_id = task_id.clone();
            let agent_id = agent_id.clone();
            thread::spawn(move || {
                dispatch_selected_with_options(&task_id, Some(&agent_id), None, None)
            })
        })
        .collect::<Vec<_>>();
    for ((task_id, _), handle) in assignments.into_iter().zip(handles) {
        let outcome = match handle.join() {
            Ok(Ok(summary)) => Ok(summary),
            Ok(Err(error)) => Err(format!("{error:#}")),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("worker thread panicked");
                Err(format!("worker thread panicked: {message}"))
            }
        };
        outcomes.insert(task_id, outcome);
    }
    Ok(outcomes)
}

pub fn accept_task(db: &Database, task_id: &str, repo_path: impl AsRef<Path>) -> Result<()> {
    let repo_path = repo_path.as_ref();
    let task = db.get_task(task_id)?.context("task not found")?;
    if task.status != TaskStatus::Review {
        anyhow::bail!(
            "task {} can only be accepted from review (currently {})",
            task_id,
            task.status
        );
    }
    let (branch, path) = db
        .get_worktree_metadata(task_id)?
        .context("task has no worktree")?;
    let worktree = repo_path.join(&path);
    if !worktree.exists() {
        anyhow::bail!("task worktree does not exist: {}", worktree.display());
    }
    if git::inspect_worktree(&worktree, repo_path)?
        .files
        .is_empty()
    {
        anyhow::bail!("task {task_id} has no meaningful changes to accept");
    }
    git::commit_worktree_changes(&worktree, task_id, &task.title)?;
    git::merge_task_branch(repo_path, &branch, task_id)?;
    git::remove_worktree(repo_path, &path)?;
    db.update_task_status(task_id, TaskStatus::Done)?;
    db.record_lifecycle_event("review_accept", Some(task_id), None, None, None)?;
    Ok(())
}

pub fn reject_task(db: &Database, task_id: &str, reason: Option<&str>) -> Result<()> {
    let task = db.get_task(task_id)?.context("task not found")?;
    if task.status != TaskStatus::Review {
        anyhow::bail!(
            "task {} can only be rejected from review (currently {})",
            task_id,
            task.status
        );
    }
    if let (Some(reason), Some(run)) = (
        reason,
        db.list_agent_runs_for_task(task_id)?.into_iter().next(),
    ) {
        let output = format!(
            "{}\n\nReview rejected: {}",
            run.output.unwrap_or_default(),
            reason
        );
        db.update_agent_run_output(run.id, &output)?;
    }
    db.update_task_status(task_id, TaskStatus::Ready)?;
    db.record_lifecycle_event("review_reject", Some(task_id), None, None, reason)?;
    Ok(())
}

pub fn dispatch_manual(
    task_id: &str,
    agent: &AgentDefinition,
    db: &Database,
    repo_path: impl AsRef<Path>,
) -> Result<()> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    let project = db.get_project_name()?.unwrap_or_else(|| "orc".into());
    let task = db.get_task(task_id)?.context("task not found in DB")?;
    if matches!(
        task.status,
        TaskStatus::Active | TaskStatus::Done | TaskStatus::Cancelled
    ) {
        anyhow::bail!(
            "Task {} cannot be manually dispatched from status {}",
            task_id,
            task.status
        );
    }
    db.update_task_status(task_id, TaskStatus::Active)?;
    let run_id =
        db.create_agent_run_with_mode(project_id, task_id, &agent.id, &agent.execution_mode)?;
    if !db.set_agent_run_waiting_external(run_id)? {
        anyhow::bail!("failed to put run {} into waiting_external", run_id);
    }
    println!(
        "Run {} (agent={}, mode=manual, status=waiting_external)",
        run_id, agent.id
    );
    println!(
        "\n{}",
        build_manual_packet(&contract, &project, &task, &agent.id)
    );
    Ok(())
}

pub fn submit_run(db: &Database, run_id: i64, output: &str) -> Result<String> {
    let run = db.get_agent_run(run_id)?.context("run not found")?;
    if run.execution_mode != registry::MANUAL || run.status != "waiting_external" {
        anyhow::bail!("run {} is not a waiting manual run", run_id);
    }
    let task_id = db.complete_manual_run(run_id, output)?;
    db.update_task_status(&task_id, TaskStatus::Review)?;
    Ok(task_id)
}

pub fn fail_run(db: &Database, run_id: i64, reason: &str) -> Result<String> {
    let run = db.get_agent_run(run_id)?.context("run not found")?;
    if run.execution_mode != registry::MANUAL || run.status != "waiting_external" {
        anyhow::bail!("run {} is not a waiting manual run", run_id);
    }
    let task_id = db.fail_run(run_id, reason)?;
    db.update_task_status(&task_id, TaskStatus::Blocked)?;
    Ok(task_id)
}

#[derive(Debug, Clone)]
pub struct PatchSubmissionOutcome {
    pub run_id: i64,
    pub task_id: String,
    pub worktree_path: PathBuf,
    pub branch_name: String,
    pub validation_report: ValidationReport,
}

pub fn submit_patch(
    db: &Database,
    run_id: i64,
    patch_content: &str,
    repo_path: impl AsRef<Path>,
) -> Result<PatchSubmissionOutcome> {
    submit_patch_with_runner(
        db,
        run_id,
        patch_content,
        repo_path,
        &SystemValidationRunner,
    )
}

pub fn submit_patch_with_runner(
    db: &Database,
    run_id: i64,
    patch_content: &str,
    repo_path: impl AsRef<Path>,
    validation_runner: &dyn ValidationRunner,
) -> Result<PatchSubmissionOutcome> {
    let repo_path = repo_path.as_ref();
    let run = db
        .get_agent_run(run_id)?
        .with_context(|| format!("run {} not found", run_id))?;

    if run.execution_mode != registry::MANUAL {
        anyhow::bail!(
            "run {} has execution_mode '{}'; only manual runs accept submit-patch",
            run_id,
            run.execution_mode
        );
    }
    if run.status != "waiting_external" {
        anyhow::bail!(
            "run {} is in status '{}'; only waiting_external manual runs accept submit-patch",
            run_id,
            run.status
        );
    }
    let task_id = run
        .task_id
        .clone()
        .with_context(|| format!("run {} has no associated task", run_id))?;

    let task = db
        .get_task(&task_id)?
        .with_context(|| format!("task '{}' not found in DB", task_id))?;

    if task.status == TaskStatus::Done {
        anyhow::bail!("task {} is already done; cannot submit patch", task_id);
    }

    if patch_content.trim().is_empty() {
        let err_msg = "malformed patch: patch content is empty";
        db.update_agent_run_output(run_id, err_msg)
            .context("failed to record malformed patch error")?;
        anyhow::bail!("{}", err_msg);
    }

    // Ensure task worktree exists
    let (branch_name, worktree_path) = match db.get_worktree_metadata(&task_id)? {
        Some((branch, path_str)) if repo_path.join(&path_str).exists() => {
            (branch, PathBuf::from(path_str))
        }
        _ => {
            let (branch, path) = git::ensure_worktree(&task_id, repo_path)?;
            (branch, path)
        }
    };

    // Record worktree metadata for this run
    db.store_worktree_metadata(
        run_id,
        &task_id,
        &branch_name,
        &worktree_path.to_string_lossy(),
    )
    .context("failed to store worktree metadata for patch submission")?;

    let absolute_worktree = repo_path.join(&worktree_path);

    // 1. Validate patch against worktree (git apply --check)
    if let Err(e) = git::validate_patch(&absolute_worktree, patch_content) {
        let err_msg = format!("patch validation failed: {:#}", e);
        db.update_agent_run_output(run_id, &err_msg)
            .context("failed to record patch validation error")?;
        anyhow::bail!("{}", err_msg);
    }

    // 2. Apply patch to worktree (git apply)
    if let Err(e) = git::apply_patch(&absolute_worktree, patch_content) {
        let err_msg = format!("patch apply failed: {:#}", e);
        db.fail_run(run_id, &err_msg)
            .context("failed to mark patch submission run as failed")?;
        db.update_task_status(&task_id, TaskStatus::Blocked)
            .context("failed to block task after patch apply failure")?;
        anyhow::bail!("{}", err_msg);
    }

    // 3. Run project validation pipeline
    let validation_config = ValidationConfig::load(repo_path)?;
    let report = validation::run_validation_pipeline(
        validation_runner,
        &validation_config.commands,
        &absolute_worktree,
    )?;

    if !report.is_success() {
        let failure_summary = format!(
            "Worktree: {}\nApplied: yes\n\nValidation:\n{}\nFailure: project validation",
            worktree_path.display(),
            report.summary()
        );
        db.fail_run(run_id, &failure_summary)
            .context("failed to mark patch validation run as failed")?;
        db.update_task_status(&task_id, TaskStatus::Blocked)
            .context("failed to block task after patch validation failure")?;
        anyhow::bail!(
            "Validation failed after applying patch to {}:\n{}",
            worktree_path.display(),
            report.summary()
        );
    }

    // 4. Success: persist output and transition lifecycle
    let success_output = format!(
        "Worktree: {}\nApplied: yes\n\nValidation:\n{}\nPatch:\n{}",
        worktree_path.display(),
        report.summary(),
        patch_content
    );
    db.complete_manual_run(run_id, &success_output)?;
    db.update_task_status(&task_id, TaskStatus::Review)?;

    Ok(PatchSubmissionOutcome {
        run_id,
        task_id,
        worktree_path,
        branch_name,
        validation_report: report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{AVAILABLE, MANUAL};
    use crate::storage::Database;
    use crate::task::TaskPriority;
    use crate::validation::test_helpers::FakeValidationRunner;
    use std::process::Command;
    use tempfile::tempdir;

    fn manual_agent() -> AgentDefinition {
        AgentDefinition {
            id: "chatgpt-lead".into(),
            backend: "chatgpt".into(),
            execution_mode: MANUAL.into(),
            display_name: "ChatGPT Lead".into(),
            enabled: true,
            priority: 100,
            capabilities: vec!["planning".into(), "review".into()],
            status: AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: None,
            model: None,
            reasoning_effort: None,
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
        }
    }

    fn init_git_repo(repo_path: &Path) {
        Command::new("git")
            .current_dir(repo_path)
            .arg("init")
            .arg(".")
            .output()
            .expect("init git");
        Command::new("git")
            .current_dir(repo_path)
            .arg("config")
            .arg("user.email")
            .arg("test@example.com")
            .output()
            .expect("git config email");
        Command::new("git")
            .current_dir(repo_path)
            .arg("config")
            .arg("user.name")
            .arg("Test User")
            .output()
            .expect("git config name");
        std::fs::write(repo_path.join("README.md"), "initial content\n").unwrap();
        Command::new("git")
            .current_dir(repo_path)
            .arg("add")
            .arg(".")
            .output()
            .expect("git add");
        Command::new("git")
            .current_dir(repo_path)
            .arg("commit")
            .arg("-m")
            .arg("initial commit")
            .output()
            .expect("git commit");
    }

    fn setup() -> (tempfile::TempDir, Database, String) {
        let dir = tempdir().unwrap();
        init_git_repo(dir.path());
        std::fs::create_dir_all(dir.path().join(".orc")).unwrap();
        std::fs::write(dir.path().join(".orc/engineering.md"), "Do focused work.").unwrap();
        let db = Database::init(dir.path().join(".orc/orc.db")).unwrap();
        let project = db.create_project("demo").unwrap();
        db.insert_task(
            project,
            "Review API",
            "Review the API design",
            "review",
            TaskPriority::Normal,
        )
        .unwrap();
        db.insert_agent(&manual_agent()).unwrap();
        (dir, db, "T-0001".into())
    }

    #[test]
    fn manual_dispatch_creates_waiting_run_without_worker_and_packet() {
        let (dir, db, task_id) = setup();
        let agent = manual_agent();
        let packet = build_manual_packet(
            "contract text",
            "demo",
            &db.get_task(&task_id).unwrap().unwrap(),
            &agent.id,
        );
        assert!(packet.contains("contract text"));
        assert!(packet.contains("T-0001"));
        dispatch_manual(&task_id, &agent, &db, dir.path()).unwrap();
        let task = db.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Active);
        let run = db
            .list_agent_runs_for_task(&task_id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(run.execution_mode, MANUAL);
        assert_eq!(run.status, "waiting_external");
        db.set_agent_execution_mode(&agent.id, registry::AUTOMATED)
            .unwrap();
        drop(db);
        let reopened = Database::open(dir.path().join(".orc/orc.db")).unwrap();
        let run = reopened
            .list_agent_runs_for_task(&task_id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(run.execution_mode, MANUAL);
    }

    #[test]
    fn submit_and_fail_manual_runs_transition_tasks_and_preserve_output() {
        let (dir, db, task_id) = setup();
        dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
        let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;
        assert_eq!(
            submit_run(&db, run_id, "review completed").unwrap(),
            task_id
        );
        let run = db.get_agent_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, "completed");
        assert_eq!(run.output.as_deref(), Some("review completed"));
        assert_eq!(
            db.get_task(&task_id).unwrap().unwrap().status,
            TaskStatus::Review
        );
        assert!(submit_run(&db, run_id, "again").is_err());

        let second_task = db
            .insert_task(
                db.get_project_id().unwrap().unwrap(),
                "Second",
                "Second",
                "review",
                TaskPriority::Normal,
            )
            .unwrap();
        dispatch_manual(&second_task, &manual_agent(), &db, dir.path()).unwrap();
        let second_run = db.list_agent_runs_for_task(&second_task).unwrap()[0].id;
        assert_eq!(
            fail_run(&db, second_run, "needs more detail").unwrap(),
            second_task
        );
        assert_eq!(
            db.get_task(&second_task).unwrap().unwrap().status,
            TaskStatus::Blocked
        );
        assert!(fail_run(&db, second_run, "again").is_err());
    }

    #[test]
    fn submit_patch_success_flow() {
        let (dir, db, task_id) = setup();
        dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
        let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

        let patch = "diff --git a/new_file.txt b/new_file.txt
new file mode 100644
--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1 @@
+hello manual patch
";
        let runner = FakeValidationRunner::success();
        let outcome = submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner)
            .expect("submit patch");

        assert_eq!(outcome.run_id, run_id);
        assert_eq!(outcome.task_id, task_id);
        assert!(outcome.validation_report.is_success());

        // Check task status moved to review
        let task = db.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Review);

        // Check run marked completed with output
        let run = db.get_agent_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, "completed");
        assert!(run.output.as_ref().unwrap().contains("hello manual patch"));

        // Check applied in worktree, not main
        let worktree_file = dir.path().join(&outcome.worktree_path).join("new_file.txt");
        assert!(worktree_file.exists());
        assert_eq!(
            std::fs::read_to_string(worktree_file).unwrap(),
            "hello manual patch\n"
        );
        assert!(!dir.path().join("new_file.txt").exists());
    }

    #[test]
    fn submit_patch_validation_failure_leaves_run_actionable() {
        let (dir, db, task_id) = setup();
        dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
        let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

        // Invalid patch
        let bad_patch = "not a valid diff";
        let runner = FakeValidationRunner::success();
        let err =
            submit_patch_with_runner(&db, run_id, bad_patch, dir.path(), &runner).unwrap_err();
        assert!(err.to_string().contains("patch validation failed"));

        // Run is still waiting_external and task is still active
        let run = db.get_agent_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, "waiting_external");
        let task = db.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Active);

        // Can resubmit with a valid patch
        let good_patch = "diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-initial content
+updated content
";
        let outcome = submit_patch_with_runner(&db, run_id, good_patch, dir.path(), &runner)
            .expect("resubmission should succeed");
        assert_eq!(outcome.task_id, task_id);
        assert_eq!(
            db.get_task(&task_id).unwrap().unwrap().status,
            TaskStatus::Review
        );
    }
}
