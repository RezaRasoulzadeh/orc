use anyhow::{Context, Result};

use crate::contract;
use crate::storage::Database;
use crate::task::{Task, TaskStatus};
use crate::worker::{CopilotWorker, Worker, WorkerOutcome};

const ENGINEERING_CONTRACT_PATH: &str = ".orc/engineering.md";

fn build_worker_prompt(contract: &str, project: &str, task: &Task) -> String {
    format!(
        "# Engineering Contract\n\n{contract}\n\n---\n\n# Task\n\nProject: {project}\nTask ID: {id}\nTitle: {title}\nObjective: {objective}\nRole: {role}\n\nInspect the repository rooted at the current working directory and implement ONLY the changes required to complete this single task. Run any relevant checks and tests (e.g., cargo test) and fix failures you encounter. Do not modify unrelated files or change task status. Stop after completing the task and summarize what you changed and any follow-up steps.\n",
        contract = contract,
        project = project,
        id = task.id,
        title = task.title,
        objective = task.objective,
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
) -> Result<()> {
    let contract = contract::load_contract(ENGINEERING_CONTRACT_PATH)?;

    let db = Database::open(db_path)
        .with_context(|| format!("failed to open orc DB ({}); run `orc init` first", db_path))?;

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

    // Set task status to active
    db.update_task_status(task_id, TaskStatus::Active)
        .with_context(|| "failed to set task status to active")?;

    // Create an agent run
    let run_id = db
        .create_agent_run(project_id, task_id, "copilot")
        .with_context(|| "failed to create agent run")?;

    let prompt = build_worker_prompt(&contract, &project_name, &task);

    // Execute the worker
    match worker.execute(&prompt) {
        Ok((outcome, output)) => {
            match outcome {
                WorkerOutcome::Success => {
                    // Mark agent run as completed and task as review
                    db.update_agent_run_status(run_id, "completed", output.as_deref())
                        .with_context(|| "failed to update agent run status to completed")?;
                    db.update_task_status(task_id, TaskStatus::Review)
                        .with_context(|| "failed to set task status to review")?;
                    Ok(())
                }
                WorkerOutcome::Failure(error) => {
                    // Mark agent run as failed and task as blocked
                    let error_msg = format!("Worker failed: {}", error);
                    db.update_agent_run_status(run_id, "failed", Some(&error_msg))
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
    dispatch_with_worker_and_db(task_id, worker, ".orc/orc.db")
}

/// Public dispatch function using the Copilot worker and default DB path
pub fn dispatch(task_id: &str) -> Result<()> {
    let worker = CopilotWorker;
    dispatch_with_worker(task_id, &worker)
}
