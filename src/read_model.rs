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
