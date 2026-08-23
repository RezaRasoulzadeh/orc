use anyhow::{Context, Result};
use std::path::Path;

use crate::git::{self, WorktreeChanges};
use crate::storage::{AgentRun, WorkerResult};
use crate::task::Task;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DispatchSummary {
    pub task: Task,
    pub agent: String,
    pub backend: String,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<crate::registry::ReasoningEffort>,
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
    if let Some(scope) = summary.task.scope_mode {
        out.push_str(&format!(
            "Scope      {scope}\nContext    {} files\n",
            summary.task.context_files.len()
        ));
    }
    if let Some(profile) = &summary.profile {
        out.push_str(&format!("Profile    {profile}\n"));
    }
    if let Some(model) = &summary.model {
        out.push_str(&format!("Model      {model}\n"));
    }
    if let Some(effort) = summary.reasoning_effort {
        out.push_str(&format!("Effort     {}\n", effort.as_str()));
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewSummary {
    pub task: Task,
    pub run: Option<AgentRun>,
    pub result: Option<WorkerResult>,
    pub worktree_path: Option<String>,
    pub changes: WorktreeChanges,
    pub change_evidence: Option<WorktreeChanges>,
    pub validation_evidence: Option<String>,
    pub prior_reviews: Vec<PriorReview>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PriorReview {
    pub verdict: String,
    pub blocking_findings: Vec<String>,
    pub non_blocking_findings: Vec<String>,
    pub revision_feedback: Option<String>,
}

pub fn build_review(
    db: &crate::storage::Database,
    task_id: &str,
    repo: &Path,
) -> Result<ReviewSummary> {
    let task = db
        .get_task(task_id)?
        .with_context(|| format!("task '{task_id}' not found"))?;
    let task_runs = db.list_agent_runs_for_task(task_id)?;
    let run = task_runs
        .iter()
        .filter(|run| run.execution_class != "review")
        .max_by_key(|run| run.id)
        .cloned();
    let result = match &run {
        Some(run) => db.get_worker_result(run.id)?,
        None => None,
    };
    let worktree_path = db.get_worktree_metadata(task_id)?.map(|(_, path)| path);
    let changes = match &worktree_path {
        Some(path) if repo.join(path).exists() => git::inspect_worktree(repo.join(path), repo)?,
        _ => WorktreeChanges::default(),
    };
    let change_evidence = run
        .as_ref()
        .map(|value| db.get_change_evidence(value.id))
        .transpose()?
        .flatten();
    let validation_evidence = run
        .as_ref()
        .map(|value| db.latest_validation_result_for_run(value.id))
        .transpose()?
        .flatten();
    let mut review_runs = task_runs
        .iter()
        .filter(|value| value.execution_class == "review")
        .collect::<Vec<_>>();
    review_runs.sort_by_key(|value| value.id);
    let prior_reviews = review_runs
        .into_iter()
        .filter_map(|value| value.output.as_deref())
        .filter_map(|output| serde_json::from_str::<PriorReview>(output).ok())
        .collect();
    Ok(ReviewSummary {
        task,
        run,
        result,
        worktree_path,
        changes,
        change_evidence,
        validation_evidence,
        prior_reviews,
    })
}

pub fn build_review_for_run(
    db: &crate::storage::Database,
    run_id: i64,
    _repo: &Path,
) -> Result<ReviewSummary> {
    let run = db
        .get_agent_run(run_id)?
        .with_context(|| format!("run {run_id} not found"))?;
    let task_id = run
        .task_id
        .as_deref()
        .context("run has no associated task")?;
    let task = db
        .get_task(task_id)?
        .with_context(|| format!("task '{task_id}' not found"))?;
    let result = db.get_worker_result(run_id)?;
    let worktree_path = db
        .get_worktree_metadata_for_run(run_id)?
        .map(|(_, path)| path);
    let changes = WorktreeChanges::default();
    let change_evidence = db.get_change_evidence(run_id)?;
    let validation_evidence = db.latest_validation_result_for_run(run_id)?;
    Ok(ReviewSummary {
        task,
        run: Some(run),
        result,
        worktree_path,
        changes,
        change_evidence,
        validation_evidence,
        prior_reviews: Vec::new(),
    })
}

pub fn format_review(summary: &ReviewSummary) -> String {
    format_review_with_diff(summary, None)
}

pub fn format_review_with_diff(summary: &ReviewSummary, diff: Option<&str>) -> String {
    format_review_with_diff_text(summary, diff.unwrap_or_default())
}

pub fn format_review_file(summary: &ReviewSummary, path: &str) -> Result<String> {
    let diff = file_diff(&summary.changes.diff, path)
        .with_context(|| format!("changed file '{path}' not found"))?;
    Ok(format_review_with_diff_text(summary, &diff))
}

fn format_review_with_diff_text(summary: &ReviewSummary, diff: &str) -> String {
    let mut out = format!(
        "Task       {}  {}\nStatus     {}\n",
        summary.task.id, summary.task.title, summary.task.status
    );
    if let Some(run) = &summary.run {
        out.push_str(&format!(
            "Agent      {}\nMode       {}\nRun        {} {}\n",
            run.agent, run.execution_mode, run.id, run.status
        ));
        if let Some(result) = &summary.result {
            out.push_str(&format!("Result     {}\n", result.outcome));
            if let Some(category) = &result.failure_category {
                out.push_str(&format!("Failure    {category}\n"));
            }
            if let Some(duration_ms) = result.duration_ms {
                out.push_str(&format!(
                    "Duration   {}\n",
                    crate::format::duration(duration_ms / 1000)
                ));
            }
        }
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
    if !diff.is_empty() {
        out.push_str(&format!("\nDiff\n{diff}"));
    }
    out
}

fn file_diff(diff: &str, path: &str) -> Option<String> {
    let mut selected = None;
    for section in diff.split_inclusive("diff --git ").skip(1) {
        let header = section.lines().next()?;
        if header.ends_with(&format!(" b/{path}")) || header.contains(&format!(" b/{path} ")) {
            selected = Some(section.to_owned());
            break;
        }
    }
    selected
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::AgentRunExecution;
    use crate::task::TaskPriority;
    use tempfile::tempdir;

    fn fixture() -> (tempfile::TempDir, crate::storage::Database, String, i64) {
        let directory = tempdir().unwrap();
        let db = crate::storage::Database::init(directory.path().join("orc.db")).unwrap();
        let project_id = db.create_project("project").unwrap();
        let task_id = db
            .insert_task(
                project_id,
                "task",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        (directory, db, task_id, project_id)
    }

    fn create_run(
        db: &crate::storage::Database,
        project_id: i64,
        task_id: &str,
        class: &str,
    ) -> i64 {
        db.create_agent_run_with_execution(
            project_id,
            task_id,
            "agent",
            "automated",
            AgentRunExecution {
                class,
                model: None,
                effort: None,
                source: "test",
            },
        )
        .unwrap()
    }

    #[test]
    fn prior_reviews_are_chronological_even_when_runs_are_returned_newest_first() {
        let (directory, db, task_id, project_id) = fixture();
        let blocker = serde_json::json!({
            "verdict": "REVISE",
            "blocking_findings": ["validation is incomplete"],
            "non_blocking_findings": [],
            "revision_feedback": "complete validation"
        });
        let pass = serde_json::json!({
            "verdict": "PASS",
            "blocking_findings": [],
            "non_blocking_findings": [],
            "revision_feedback": null
        });
        let first = create_run(&db, project_id, &task_id, "review");
        db.update_agent_run_status(first, "completed", Some(&blocker.to_string()))
            .unwrap();
        let second = create_run(&db, project_id, &task_id, "review");
        db.update_agent_run_status(second, "completed", Some(&pass.to_string()))
            .unwrap();

        let summary = build_review(&db, &task_id, directory.path()).unwrap();

        assert_eq!(summary.prior_reviews[0].verdict, "REVISE");
        assert_eq!(summary.prior_reviews[1].verdict, "PASS");
    }

    #[test]
    fn newest_non_review_run_and_latest_validation_result_are_selected_exactly() {
        let (directory, db, task_id, project_id) = fixture();
        let older = create_run(&db, project_id, &task_id, "code");
        db.record_lifecycle_event(
            "validation_result",
            Some(&task_id),
            Some(older),
            Some("agent"),
            Some(r#"{"steps":[{"command":"cargo test","passed":true,"output":""}]}"#),
        )
        .unwrap();
        let newest = create_run(&db, project_id, &task_id, "code");
        db.record_lifecycle_event(
            "validation_result",
            Some(&task_id),
            Some(newest),
            Some("agent"),
            Some(r#"{"steps":[{"command":"stale command","passed":false,"output":"stale"}]}"#),
        )
        .unwrap();
        let expected = r#"{"steps":[{"command":"npm run typecheck","passed":true,"output":""},{"command":"npm run build","passed":true,"output":""},{"command":"cargo tauri build --no-bundle","passed":true,"output":""}]}"#;
        db.record_lifecycle_event(
            "validation_result",
            Some(&task_id),
            Some(newest),
            Some("agent"),
            Some(expected),
        )
        .unwrap();
        let review = create_run(&db, project_id, &task_id, "review");
        db.update_agent_run_status(review, "completed", Some(r#"{"verdict":"PASS","blocking_findings":[],"non_blocking_findings":[],"revision_feedback":null}"#)).unwrap();

        let summary = build_review(&db, &task_id, directory.path()).unwrap();

        assert_eq!(summary.run.as_ref().map(|run| run.id), Some(newest));
        assert_eq!(summary.validation_evidence.as_deref(), Some(expected));
    }
}
