use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::agent;
use crate::protocol::{PlanResponse, PlanningProjectState, ProjectReport};
use crate::queue::{QueueReport, compute_queue};
use crate::registry::{self, AgentDefinition};
use crate::review::{DispatchSummary, ReviewSummary, build_review};
use crate::storage::db::ApprovalRequest;
use crate::storage::{AgentRun, Database};
use crate::task::{Task, TaskScopeMode};

pub struct OrcApp {
    db: Database,
    repo_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum CancelError {
    #[error("database error: {0}")]
    Database(#[from] crate::storage::db::DbError),
    #[error("{0}")]
    Invalid(String),
}

impl OrcApp {
    pub fn open(db_path: impl AsRef<Path>, repo_path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            db: Database::open(db_path)?,
            repo_path: repo_path.as_ref().to_path_buf(),
        })
    }

    pub fn project_report(
        &self,
        engineering_contract: String,
        architecture: crate::protocol::ReportArchitecture,
    ) -> Result<ProjectReport> {
        let id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        let name = self.db.get_project_name()?.unwrap_or_else(|| "orc".into());
        Ok(self.db.project_report(
            id,
            name,
            self.repo_path.display().to_string(),
            engineering_contract,
            architecture,
        )?)
    }
    pub fn tasks(&self) -> Result<Vec<Task>> {
        Ok(self.db.list_tasks()?)
    }
    pub fn task(&self, id: &str) -> Result<Option<Task>> {
        Ok(self.db.get_task(id)?)
    }
    pub fn queue(&self) -> Result<QueueReport> {
        Ok(compute_queue(&self.db)?)
    }
    pub fn runs(&self, limit: usize) -> Result<Vec<AgentRun>> {
        let id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        Ok(self.db.list_agent_runs(id, limit)?)
    }
    pub fn agents(&self) -> Result<Vec<AgentDefinition>> {
        Ok(self.db.list_agents()?)
    }
    pub fn approvals(&self) -> Result<Vec<ApprovalRequest>> {
        let id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        Ok(self.db.list_approval_requests(id)?)
    }
    pub fn planning_state(&self) -> Result<PlanningProjectState> {
        Ok(self.db.planning_project_state()?)
    }
    pub fn review(&self, task_id: &str) -> Result<ReviewSummary> {
        build_review(&self.db, task_id, &self.repo_path)
    }
    pub fn requeue(&self, task_id: &str) -> Result<()> {
        Ok(self.db.requeue_task(
            task_id,
            "Task manually requeued after interrupted Orc process recovery",
        )?)
    }
    pub fn cancel(
        &self,
        task_id: &str,
        reason: Option<&str>,
    ) -> std::result::Result<(), CancelError> {
        if self.db.cancel_task(task_id, reason)? {
            return Ok(());
        }
        Err(CancelError::Invalid(format!(
            "task '{}' cannot be cancelled",
            task_id
        )))
    }
    pub fn accept(&self, task_id: &str) -> Result<()> {
        agent::accept_task(&self.db, task_id, &self.repo_path)
    }
    pub fn reject(&self, task_id: &str, reason: Option<&str>) -> Result<()> {
        agent::reject_task(&self.db, task_id, reason)
    }
    pub fn dispatch(&self, task_id: &str, agent_id: Option<&str>) -> Result<DispatchSummary> {
        agent::dispatch_selected_with_db_and_repo(
            &self.db,
            &self.repo_path,
            task_id,
            agent_id,
            None,
            None,
        )
    }
    pub fn revise(&self, task_id: &str, feedback: &str, agent_id: &str) -> Result<()> {
        let agent = self.db.get_agent(agent_id)?.context("agent not found")?;
        if agent.execution_mode == registry::MANUAL {
            agent::revise_manual(task_id, feedback, &agent, &self.db, &self.repo_path)
        } else {
            let worker =
                crate::backend::WorkerFactory::build(&agent).map_err(anyhow::Error::msg)?;
            agent::revise_with_worker_on_db(
                task_id,
                feedback,
                worker.as_ref(),
                &self.db,
                &self.repo_path,
                &agent.id,
                &crate::SystemValidationRunner,
            )?;
            Ok(())
        }
    }
    pub fn submit_manual_run(&self, run_id: i64, output: &str) -> Result<String> {
        agent::submit_run(&self.db, run_id, output)
    }
    pub fn fail_manual_run(&self, run_id: i64, reason: &str) -> Result<String> {
        agent::fail_run(&self.db, run_id, reason)
    }
    pub fn submit_patch(&self, run_id: i64, patch: &str) -> Result<agent::PatchSubmissionOutcome> {
        agent::submit_patch(&self.db, run_id, patch, &self.repo_path)
    }
    pub fn configure_agent(&self, agent: AgentDefinition) -> Result<()> {
        registry::validate_backend(&agent.backend)?;
        Ok(self.db.insert_agent(&agent)?)
    }
    pub fn set_agent_enabled(&self, id: &str, enabled: bool) -> Result<bool> {
        Ok(self.db.set_agent_enabled(id, enabled)?)
    }
    pub fn set_agent_priority(&self, id: &str, priority: i64) -> Result<bool> {
        Ok(self.db.set_agent_priority(id, priority)?)
    }
    pub fn set_agent_profile(&self, id: &str, path: &str) -> Result<bool> {
        Ok(self.db.set_agent_profile_path(id, path)?)
    }
    pub fn set_agent_model(&self, id: &str, model: &str) -> Result<bool> {
        Ok(self.db.set_agent_model(id, model)?)
    }
    pub fn set_agent_effort(&self, id: &str, effort: registry::ReasoningEffort) -> Result<bool> {
        Ok(self.db.set_agent_reasoning_effort(id, effort)?)
    }
    pub fn set_task_scope(&self, id: &str, scope: TaskScopeMode) -> Result<bool> {
        Ok(self.db.set_task_scope(id, scope)?)
    }
    pub fn apply_plan(
        &self,
        response: &PlanResponse,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        let id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        Ok(self.db.apply_plan(id, response)?)
    }
    pub fn add_dependency(&self, task_id: &str, dependency_id: &str) -> Result<()> {
        Ok(self.db.add_task_dependency(task_id, dependency_id)?)
    }
    pub fn remove_dependency(&self, task_id: &str, dependency_id: &str) -> Result<bool> {
        Ok(self.db.remove_task_dependency(task_id, dependency_id)?)
    }
    pub fn resolve_approval(&self, id: i64) -> Result<()> {
        let project_id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        if !self.db.resolve_approval_request(project_id, id)? {
            anyhow::bail!("approval request {id} not found for current project")
        }
        Ok(())
    }
}
