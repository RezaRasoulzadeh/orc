use crate::{
    queue::QueueReport,
    registry::AgentDefinition,
    storage::db::{ApprovalRequest, LifecycleEvent},
    storage::{AgentRun, WorkerResult},
    task::Task,
};
use anyhow::{Context, Result};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Dashboard {
    pub queue: QueueReport,
    pub tasks: Vec<Task>,
    pub agents: Vec<AgentDefinition>,
    pub approvals: Vec<ApprovalRequest>,
    pub recent_activity: Vec<LifecycleEvent>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskDetails {
    pub task: Task,
    pub runs: Vec<AgentRun>,
    pub activity: Vec<LifecycleEvent>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunDetails {
    pub run: AgentRun,
    pub result: Option<WorkerResult>,
    pub activity: Vec<LifecycleEvent>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunsWorkspace {
    pub runs: Vec<AgentRun>,
    pub details: Vec<RunDetails>,
}

pub fn runs_workspace(
    db: &crate::storage::Database,
    limit: usize,
    activity_limit: usize,
) -> Result<RunsWorkspace> {
    let project = db.get_project_id()?.context("no project found in DB")?;
    let runs = db.list_agent_runs(project, limit)?;
    let details = runs
        .iter()
        .map(|run| {
            Ok(RunDetails {
                run: run.clone(),
                result: db.get_worker_result(run.id)?,
                activity: db.list_lifecycle_events_for_run(run.id, activity_limit)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RunsWorkspace { runs, details })
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentCapacity {
    pub agents: Vec<AgentDefinition>,
    pub busy: Vec<String>,
    pub quota_reserve_percent: i64,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectHealth {
    pub task_counts: std::collections::BTreeMap<String, usize>,
    pub active_runs: usize,
    pub unresolved_approvals: usize,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlannerSummary {
    pub state: crate::protocol::PlanningProjectState,
}
pub type ReportSummary = crate::protocol::ProjectReport;

pub fn dashboard(db: &crate::storage::Database, activity_limit: usize) -> Result<Dashboard> {
    let project = db.get_project_id()?.context("no project found in DB")?;
    Ok(Dashboard {
        queue: crate::queue::compute_queue(db)?,
        tasks: db.list_tasks()?,
        agents: db.list_agents()?,
        approvals: db.list_approval_requests(project)?,
        recent_activity: db.list_lifecycle_events(activity_limit)?,
    })
}
pub fn task_details(
    db: &crate::storage::Database,
    id: &str,
    activity_limit: usize,
) -> Result<Option<TaskDetails>> {
    let Some(task) = db.get_task(id)? else {
        return Ok(None);
    };
    Ok(Some(TaskDetails {
        task,
        runs: db.list_agent_runs_for_task(id)?,
        activity: db.list_lifecycle_events_for_task(id, activity_limit)?,
    }))
}
pub fn run_details(
    db: &crate::storage::Database,
    id: i64,
    activity_limit: usize,
) -> Result<Option<RunDetails>> {
    let Some(run) = db.get_agent_run(id)? else {
        return Ok(None);
    };
    Ok(Some(RunDetails {
        result: db.get_worker_result(id)?,
        activity: db.list_lifecycle_events_for_run(id, activity_limit)?,
        run,
    }))
}

pub fn agent_capacity(db: &crate::storage::Database) -> Result<AgentCapacity> {
    Ok(AgentCapacity {
        agents: db.list_agents()?,
        busy: db.list_busy_agents()?,
        quota_reserve_percent: db.quota_reserve()?,
    })
}

pub fn project_health(db: &crate::storage::Database) -> Result<ProjectHealth> {
    let tasks = db.list_tasks()?;
    let project = db.get_project_id()?.context("no project found in DB")?;
    let mut task_counts = std::collections::BTreeMap::new();
    for task in tasks {
        *task_counts.entry(task.status.to_string()).or_insert(0) += 1;
    }
    let runs = db.list_agent_runs(project, usize::MAX)?;
    let active_runs = runs
        .iter()
        .filter(|run| matches!(run.status.as_str(), "running" | "waiting_external"))
        .count();
    let unresolved_approvals = db
        .list_approval_requests(project)?
        .into_iter()
        .filter(|approval| !approval.resolved)
        .count();
    Ok(ProjectHealth {
        task_counts,
        active_runs,
        unresolved_approvals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskPriority;
    use crate::worker::TokenUsage;
    use tempfile::tempdir;

    #[test]
    fn historical_run_details_expose_persisted_token_usage() {
        let directory = tempdir().unwrap();
        let db = crate::storage::Database::init(directory.path().join("orc.db")).unwrap();
        let project = db.create_project("project").unwrap();
        let task = db
            .insert_task(project, "task", "work", "developer", TaskPriority::Normal)
            .unwrap();
        let run = db.create_agent_run(project, &task, "codex").unwrap();
        db.update_agent_run_status_with_usage(
            run,
            "completed",
            Some("complete"),
            Some(TokenUsage {
                total_tokens: 34,
                input_tokens: Some(21),
                output_tokens: Some(13),
            }),
        )
        .unwrap();

        let details = run_details(&db, run, 10).unwrap().unwrap();
        let result = details.result.unwrap();
        assert_eq!(result.total_tokens, Some(34));
        assert_eq!(result.input_tokens, Some(21));
        assert_eq!(result.output_tokens, Some(13));
        let workspace = runs_workspace(&db, 10, 10).unwrap();
        assert_eq!(
            workspace.details[0].result.as_ref().unwrap().total_tokens,
            Some(34)
        );
    }
}
