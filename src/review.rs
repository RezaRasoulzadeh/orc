use anyhow::{Context, Result};
use std::path::Path;

use crate::git::{self, WorktreeChanges};
use crate::storage::AgentRun;
use crate::task::Task;

#[derive(Debug, Clone)]
pub struct DispatchSummary {
    pub task: Task,
    pub agent: String,
    pub backend: String,
    pub profile: Option<String>,
    pub worktree_path: String,
    pub run_id: i64,
    pub run_status: String,
    pub validation: String,
    pub changes: WorktreeChanges,
}

pub fn format_dispatch(summary: &DispatchSummary) -> String {
    let mut out = format!(
        "Task       {}  {}\nAgent      {}\nBackend    {}\n",
        summary.task.id, summary.task.title, summary.agent, summary.backend
    );
    if let Some(profile) = &summary.profile {
        out.push_str(&format!("Profile    {profile}\n"));
    }
    out.push_str(&format!(
        "Worktree   {}\n\nPreparing worktree       OK\nStarting worker          OK\nWorker finished          {}\nValidation               {}\n\nChanges\n{}\nRun        {} {}\nTask       {}\n\nNext: orc review {}",
        summary.worktree_path,
        if summary.run_status == "completed" { "OK" } else { "FAILED" },
        summary.validation,
        format_changes(&summary.changes),
        summary.run_id,
        summary.run_status,
        summary.task.status,
        summary.task.id,
    ));
    out
}

#[derive(Debug, Clone)]
pub struct ReviewSummary {
    pub task: Task,
    pub run: Option<AgentRun>,
    pub worktree_path: Option<String>,
    pub changes: WorktreeChanges,
}

pub fn build_review(
    db: &crate::storage::Database,
    task_id: &str,
    repo: &Path,
) -> Result<ReviewSummary> {
    let task = db
        .get_task(task_id)?
        .with_context(|| format!("task '{task_id}' not found"))?;
    let run = db.list_agent_runs_for_task(task_id)?.into_iter().next();
    let worktree_path = db.get_worktree_metadata(task_id)?.map(|(_, path)| path);
    let changes = match &worktree_path {
        Some(path) if repo.join(path).exists() => git::inspect_worktree(repo.join(path), repo)?,
        _ => WorktreeChanges::default(),
    };
    Ok(ReviewSummary {
        task,
        run,
        worktree_path,
        changes,
    })
}

pub fn format_review(summary: &ReviewSummary) -> String {
    let mut out = format!(
        "Task       {}  {}\nStatus     {}\n",
        summary.task.id, summary.task.title, summary.task.status
    );
    if let Some(run) = &summary.run {
        out.push_str(&format!(
            "Agent      {}\nMode       {}\nRun        {} {}\n",
            run.agent, run.execution_mode, run.id, run.status
        ));
        if let Some(output) = &run.output
            && let Some((_, validation)) = output.split_once("Validation:\n")
        {
            let validation = validation
                .split_once("\nPatch:\n")
                .map_or(validation, |(v, _)| v);
            out.push_str(&format!("Validation\n{}\n", validation.trim()));
        }
    }
    if let Some(path) = &summary.worktree_path {
        out.push_str(&format!("Worktree   {path}\n"));
    }
    out.push_str(&format!(
        "\nChanges\n{}\n",
        format_changes(&summary.changes)
    ));
    if !summary.changes.diff.is_empty() {
        out.push_str(&format!("\nDiff\n{}", summary.changes.diff));
    }
    out
}

pub fn format_changes(changes: &WorktreeChanges) -> String {
    if changes.files.is_empty() {
        return "  (no meaningful project changes)".into();
    }
    let mut out = String::new();
    for file in &changes.files {
        out.push_str(&format!("  {}  {}\n", file.status, file.path));
    }
    if !changes.stat.is_empty() {
        out.push_str(&format!("\n{}", changes.stat));
    }
    out.trim_end().to_owned()
}
