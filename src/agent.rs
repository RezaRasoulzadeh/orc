use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use crate::contract;
use crate::storage::Database;
use crate::task::Task;

const ENGINEERING_CONTRACT_PATH: &str = ".orc/engineering.md";

fn build_worker_prompt(contract: &str, project: &str, task: &Task) -> String {
    // Include the engineering contract before task-specific instructions
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

pub fn dispatch(task_id: &str) -> Result<()> {
    // Load the engineering contract
    let contract = contract::load_contract(ENGINEERING_CONTRACT_PATH)?;

    // use sqlite-backed storage
    let db = Database::open(".orc/orc.db")
        .with_context(|| "failed to open orc DB (.orc/orc.db); run `orc init` first")?;
    let project = db
        .get_project_name()
        .with_context(|| "failed to read project name from DB")?;
    let project_name = project.unwrap_or_else(|| "orc".into());

    let task = db
        .get_task(task_id)
        .with_context(|| format!("failed to fetch task '{}' from DB", task_id))?
        .with_context(|| format!("task '{}' not found in DB", task_id))?;

    let prompt = build_worker_prompt(&contract, &project_name, &task);

    // Spawn copilot with inherited stdio so the user sees progress
    let mut cmd = Command::new("copilot");
    cmd.arg("-p").arg(&prompt).arg("--allow-all-tools");
    cmd.stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit());

    let status = cmd.status().with_context(
        || "failed to spawn 'copilot' executable; ensure it is installed and on PATH",
    )?;

    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".into());
        anyhow::bail!("Copilot exited with non-zero status: {}", code);
    }

    Ok(())
}
