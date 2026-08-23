use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::agent;
use crate::protocol::{PlanResponse, PlanningProjectState, ProjectReport};
use crate::queue::{QueueReport, compute_queue};
use crate::registry::{self, AgentDefinition};
use crate::review::{DispatchSummary, ReviewSummary, build_review};
use crate::storage::db::ApprovalRequest;
use crate::storage::{AgentRun, Database};
use crate::task::{CreateTaskInput, Task, TaskScopeMode};

#[derive(Debug, serde::Serialize)]
pub struct ManualRunContext {
    pub run: AgentRun,
    pub task: Task,
    pub task_packet: String,
}

pub struct OrcApp {
    db: Database,
    repo_path: PathBuf,
    events: crate::events::EventHub,
}

#[derive(Debug, thiserror::Error)]
pub enum CancelError {
    #[error("database error: {0}")]
    Database(#[from] crate::storage::db::DbError),
    #[error("{0}")]
    Invalid(String),
}

impl OrcApp {
    pub fn lead(&self) -> crate::lead::LeadService<'_> {
        crate::lead::LeadService::new(&self.db, &self.repo_path)
    }
    pub fn invoke_lead(
        &self,
        message: &str,
        backend: &dyn crate::lead::LeadBackend,
        context_limit: usize,
    ) -> Result<crate::lead::LeadResponse> {
        self.lead().invoke(message, backend, context_limit)
    }
    pub fn invoke_configured_lead(
        &self,
        message: &str,
        config: &crate::lead::LeadProviderConfig,
        context_limit: usize,
    ) -> Result<crate::lead::LeadResponse> {
        let agent = self.db.get_agent(&config.agent_id)?.with_context(|| {
            format!(
                "configured Lead agent '{}' is unavailable: agent does not exist",
                config.agent_id
            )
        })?;
        if !agent.enabled || agent.status != registry::AVAILABLE {
            anyhow::bail!(
                "configured Lead agent '{}' is unavailable{}",
                agent.id,
                agent
                    .unavailable_reason
                    .as_deref()
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default()
            );
        }
        let backend = crate::lead::CodexLeadBackend::from_agent(
            &agent,
            &self.repo_path,
            config.model.clone(),
            config.reasoning_effort,
        )
        .map_err(anyhow::Error::msg)?;
        self.lead().invoke(message, &backend, context_limit)
    }
    pub fn invoke_persisted_lead(
        &self,
        message: &str,
        context_limit: usize,
    ) -> Result<crate::lead::LeadResponse> {
        let config = self.lead_provider_config()?.ok_or_else(|| anyhow::anyhow!("Lead is not configured. Configure one with `orc lead set <agent>` before running `orc ask`."))?;
        self.invoke_configured_lead(message, &config, context_limit)
    }
    pub fn automated_plan_with_backend(
        &self,
        request: &crate::protocol::PlanningRequest,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
    ) -> Result<(i64, PlanResponse)> {
        crate::automated::run_plan(&self.db, request, overrides, backend)
    }
    pub fn automated_plan(
        &self,
        request: &crate::protocol::PlanningRequest,
        overrides: &crate::automated::ActionOverrides,
    ) -> Result<(i64, PlanResponse)> {
        let backend = crate::automated::WorkerActionBackend::new(&self.repo_path);
        self.automated_plan_with_backend(request, overrides, &backend)
    }
    pub fn automated_lead_with_backend(
        &self,
        message: &str,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
    ) -> Result<(i64, crate::lead::LeadResponse)> {
        crate::automated::run_lead(&self.db, &self.repo_path, message, overrides, backend)
    }
    pub fn automated_lead(
        &self,
        message: &str,
        overrides: &crate::automated::ActionOverrides,
    ) -> Result<(i64, crate::lead::LeadResponse)> {
        let backend = crate::automated::WorkerActionBackend::new(&self.repo_path);
        self.automated_lead_with_backend(message, overrides, &backend)
    }
    pub fn lead_provider_config(&self) -> Result<Option<crate::lead::LeadProviderConfig>> {
        Ok(self.db.lead_provider_config()?)
    }
    pub fn set_lead_provider_config(&self, config: crate::lead::LeadProviderConfig) -> Result<()> {
        let agent = self
            .db
            .get_agent(&config.agent_id)?
            .with_context(|| format!("Lead agent '{}' does not exist", config.agent_id))?;
        if !agent.supports_action(registry::AgentAction::Lead)
            || agent.execution_mode != registry::AUTOMATED
            || agent.backend != "codex"
        {
            anyhow::bail!(
                "Lead agent '{}' is not compatible with Lead execution",
                config.agent_id
            );
        }
        self.db.set_lead_provider_config(&config)?;
        Ok(())
    }
    pub fn agent_action_profiles(
        &self,
        id: &str,
    ) -> Result<Vec<crate::registry::AgentActionProfile>> {
        Ok(self.db.agent_action_profiles(id)?)
    }
    pub fn set_agent_action_profile(
        &self,
        id: &str,
        action: crate::registry::AgentAction,
        model: Option<&str>,
        effort: Option<registry::ReasoningEffort>,
    ) -> Result<bool> {
        registry::get_agent(&self.db, id)?;
        Ok(self
            .db
            .set_agent_action_profile(id, action, model, effort)?)
    }
    pub fn clear_agent_action_profile(
        &self,
        id: &str,
        action: crate::registry::AgentAction,
    ) -> Result<bool> {
        Ok(self.db.clear_agent_action_profile(id, action)?)
    }
    pub fn clear_lead_provider_config(&self) -> Result<()> {
        Ok(self.db.clear_lead_provider_config()?)
    }
    pub fn recover_lead_proposal(&self, proposal_id: i64) -> Result<()> {
        if !self.lead().recover_proposal(proposal_id)? {
            anyhow::bail!("Lead proposal is not applying")
        }
        Ok(())
    }
    pub fn apply_lead_proposal(&self, proposal_id: i64) -> Result<()> {
        use crate::lead::{LeadProposalKind, LeadProposalStatus, single_task_plan};
        let proposal = self
            .lead()
            .proposal(proposal_id)?
            .context("Lead proposal not found for current project")?;
        if proposal.status != LeadProposalStatus::Pending {
            anyhow::bail!("Lead proposal is not pending")
        }
        if matches!(proposal.proposal, LeadProposalKind::Revision { .. }) {
            anyhow::bail!("revision proposals require an explicit agent selection")
        }
        if !self.lead().claim_proposal(proposal_id)? {
            anyhow::bail!("Lead proposal is not pending")
        }
        let result = match proposal.proposal {
            LeadProposalKind::Plan(plan) => self.apply_plan(&plan).map(|_| ()),
            LeadProposalKind::Task(task) => self.apply_plan(&single_task_plan(task)).map(|_| ()),
            LeadProposalKind::ApprovalRequest { reason, details } => (|| -> Result<()> {
                let project_id = self
                    .db
                    .get_project_id()?
                    .context("no project found in DB")?;
                self.db
                    .insert_approval_request(project_id, &format!("{reason}\n\n{details}"))?;
                Ok(())
            })(),
            LeadProposalKind::Revision { .. } => unreachable!(),
        };
        if let Err(error) = result {
            if !self.lead().release_proposal(proposal_id)? {
                anyhow::bail!("{error:#}; Lead proposal could not be returned to pending")
            }
            return Err(error);
        }
        if !self.lead().finish_proposal(proposal_id)? {
            anyhow::bail!("Lead proposal changed while it was being applied")
        }
        Ok(())
    }
    pub fn apply_lead_revision_proposal(&self, proposal_id: i64, agent_id: &str) -> Result<()> {
        use crate::lead::{LeadProposalKind, LeadProposalStatus};
        let proposal = self
            .lead()
            .proposal(proposal_id)?
            .context("Lead proposal not found for current project")?;
        if proposal.status != LeadProposalStatus::Pending {
            anyhow::bail!("Lead proposal is not pending")
        }
        let LeadProposalKind::Revision { task_id, feedback } = proposal.proposal else {
            anyhow::bail!("Lead proposal is not a revision")
        };
        if !self.lead().claim_proposal(proposal_id)? {
            anyhow::bail!("Lead proposal is not pending")
        }
        if let Err(error) = self.revise(&task_id, &feedback, agent_id) {
            if !self.lead().release_proposal(proposal_id)? {
                anyhow::bail!("{error:#}; Lead proposal could not be returned to pending")
            }
            return Err(error);
        }
        if !self.lead().finish_proposal(proposal_id)? {
            anyhow::bail!("Lead proposal changed while it was being applied")
        }
        Ok(())
    }
    pub fn open(db_path: impl AsRef<Path>, repo_path: impl AsRef<Path>) -> Result<Self> {
        let events = crate::events::EventHub::new();
        let mut db = Database::open(db_path)?;
        let sink = events.clone();
        db.set_lifecycle_sink(Some(std::sync::Arc::new(move |event| {
            sink.publish(crate::events::AppEvent::from_lifecycle(event));
        })));
        Ok(Self {
            db,
            repo_path: repo_path.as_ref().to_path_buf(),
            events,
        })
    }

    pub fn project_report(&self) -> Result<ProjectReport> {
        let engineering_contract =
            crate::contract::load_contract(self.repo_path.join(".orc/engineering.md"))?;
        self.project_report_with(
            engineering_contract,
            crate::protocol::ReportArchitecture::default(),
        )
    }

    fn project_report_with(
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
    pub fn dashboard(&self, activity_limit: usize) -> Result<crate::read_model::Dashboard> {
        crate::read_model::dashboard(&self.db, &self.repo_path, activity_limit)
    }
    pub fn task_details(
        &self,
        id: &str,
        activity_limit: usize,
    ) -> Result<Option<crate::read_model::TaskDetails>> {
        crate::read_model::task_details(&self.db, id, activity_limit)
    }
    pub fn run_details(
        &self,
        id: i64,
        activity_limit: usize,
    ) -> Result<Option<crate::read_model::RunDetails>> {
        crate::read_model::run_details(&self.db, id, activity_limit)
    }
    pub fn subscribe(&self) -> crate::events::EventSubscription {
        self.events.subscribe()
    }
    pub fn lifecycle_events(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::storage::db::LifecycleEvent>> {
        Ok(self.db.list_lifecycle_events(limit)?)
    }
    pub fn agent_capacity(&self) -> Result<crate::read_model::AgentCapacity> {
        crate::read_model::agent_capacity(&self.db)
    }
    pub fn project_health(&self) -> Result<crate::read_model::ProjectHealth> {
        crate::read_model::project_health(&self.db)
    }
    pub fn planner_summary(&self) -> Result<crate::read_model::PlannerSummary> {
        Ok(crate::read_model::PlannerSummary {
            state: self.planning_state()?,
        })
    }
    pub fn planning_request(&self) -> Result<crate::protocol::PlanningRequest> {
        let report = self.project_report()?;
        let contract = report.engineering_contract.clone();
        Ok(crate::protocol::PlanningRequest {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            kind: "project_plan".into(),
            project: Some(report.project.clone()),
            engineering_contract: contract,
            objective: "Plan the next useful project work from persisted state.".into(),
            constraints: report.planning_constraints.clone(),
            target_platforms: Vec::new(),
            stack: Vec::new(),
            non_goals: vec!["Do not mutate project state while planning.".into()],
            deliverables: vec!["A validated PlanResponse JSON document.".into()],
            definition_of_done: vec!["Every task has a unique id and valid dependencies.".into()],
            response_schema: crate::protocol::PlanResponseSchema::v1(),
            role_boundaries: report.role_boundaries.clone(),
            planning_constraints: report.planning_constraints.clone(),
            approval_requirements: report.approval_requirements.clone(),
            current_state: Some(self.planning_state()?),
            full_report: Some(report),
        })
    }
    pub fn validate_plan_json(&self, json: &str) -> Result<PlanResponse> {
        let response: PlanResponse =
            serde_json::from_str(json).context("invalid PlanResponse JSON")?;
        response.validate()?;
        Ok(response)
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
    pub fn runs_workspace(
        &self,
        limit: usize,
        activity_limit: usize,
    ) -> Result<crate::read_model::RunsWorkspace> {
        crate::read_model::runs_workspace(&self.db, limit, activity_limit)
    }
    pub fn agents(&self) -> Result<Vec<AgentDefinition>> {
        Ok(self.db.list_agents()?)
    }
    pub fn manual_runs(&self, agent_id: &str) -> Result<Vec<ManualRunContext>> {
        let agent = registry::get_agent(&self.db, agent_id)?;
        if agent.execution_mode != registry::MANUAL {
            anyhow::bail!("agent '{}' is not a manual agent", agent_id)
        }
        let project_id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        let project = self.db.get_project_name()?.unwrap_or_else(|| "orc".into());
        let contract = crate::contract::load_contract(self.repo_path.join(".orc/engineering.md"))?;
        self.db
            .list_agent_runs(project_id, usize::MAX)?
            .into_iter()
            .filter(|run| run.agent == agent_id && run.status == "waiting_external")
            .map(|run| {
                let task_id = run
                    .task_id
                    .as_deref()
                    .context("manual run has no task id")?;
                let task = self
                    .db
                    .get_task(task_id)?
                    .context("manual run task not found")?;
                let task_packet = agent::build_manual_packet(&contract, &project, &task, agent_id);
                Ok(ManualRunContext {
                    run,
                    task,
                    task_packet,
                })
            })
            .collect()
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
    pub fn automated_review_with_backend(
        &self,
        task_id: &str,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
    ) -> Result<(i64, crate::automated::ReviewResult)> {
        let summary = self.review(task_id)?;
        crate::automated::run_review(&self.db, &summary, overrides, backend)
    }
    pub fn automated_review(
        &self,
        task_id: &str,
        overrides: &crate::automated::ActionOverrides,
    ) -> Result<(i64, crate::automated::ReviewResult)> {
        let backend = crate::automated::WorkerActionBackend::new(&self.repo_path);
        self.automated_review_with_backend(task_id, overrides, &backend)
    }
    pub fn requeue(&self, task_id: &str) -> Result<()> {
        self.db.requeue_task(
            task_id,
            "Operator manually requeued task after recoverable run failure",
        )?;
        Ok(())
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
        agent::accept_task(&self.db, task_id, &self.repo_path)?;
        Ok(())
    }
    pub fn reject(&self, task_id: &str, reason: Option<&str>) -> Result<()> {
        agent::reject_task(&self.db, task_id, reason)?;
        Ok(())
    }
    pub fn dispatch(&self, task_id: &str, agent_id: Option<&str>) -> Result<DispatchSummary> {
        let result = agent::dispatch_selected_with_db_and_repo(
            &self.db,
            &self.repo_path,
            task_id,
            agent_id,
            None,
            None,
        )?;
        Ok(result)
    }
    pub fn revise(&self, task_id: &str, feedback: &str, agent_id: &str) -> Result<()> {
        let agent = self.db.get_agent(agent_id)?.context("agent not found")?;
        if agent.execution_mode == registry::MANUAL {
            agent::revise_manual(task_id, feedback, &agent, &self.db, &self.repo_path)?;
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
        }
        Ok(())
    }
    pub fn submit_manual_run(&self, run_id: i64, output: &str) -> Result<String> {
        let result = agent::submit_run(&self.db, run_id, output)?;
        Ok(result)
    }
    pub fn fail_manual_run(&self, run_id: i64, reason: &str) -> Result<String> {
        let result = agent::fail_run(&self.db, run_id, reason)?;
        Ok(result)
    }
    pub fn submit_patch(&self, run_id: i64, patch: &str) -> Result<agent::PatchSubmissionOutcome> {
        agent::submit_patch(&self.db, run_id, patch, &self.repo_path)
    }
    pub fn configure_agent(&self, agent: AgentDefinition) -> Result<()> {
        registry::validate_backend(&agent.backend)?;
        if (agent.model.is_some() || agent.reasoning_effort.is_some())
            && (agent.backend != "codex" || agent.execution_mode == registry::MANUAL)
        {
            anyhow::bail!(
                "only automated Codex agents support model and reasoning-effort configuration"
            );
        }
        if agent.execution_mode == registry::AUTOMATED
            && !matches!(agent.backend.as_str(), "codex" | "copilot" | "antigravity")
        {
            anyhow::bail!("backend '{}' requires --mode manual", agent.backend);
        }
        self.db.insert_agent(&agent)?;
        Ok(())
    }
    pub fn set_agent_enabled(&self, id: &str, enabled: bool) -> Result<bool> {
        let result = self.db.set_agent_enabled(id, enabled)?;
        Ok(result)
    }
    pub fn set_agent_availability(
        &self,
        id: &str,
        available: bool,
        reason: Option<&str>,
    ) -> Result<bool> {
        Ok(self.db.set_agent_availability(
            id,
            if available {
                registry::AVAILABLE
            } else {
                registry::UNAVAILABLE
            },
            reason,
        )?)
    }
    pub fn remove_agent(&self, id: &str) -> Result<()> {
        self.db.archive_agent(id).map_err(anyhow::Error::from)
    }
    pub fn purge_agent(&self, id: &str) -> Result<()> {
        self.db.purge_agent(id).map_err(anyhow::Error::from)
    }
    pub fn purge_task(&self, id: &str, force: bool) -> Result<()> {
        self.db.validate_task_purge(id, force)?;
        let path = self.db.get_worktree_metadata(id)?.map(|(_, path)| path);
        let expected = crate::git::worktree_path_for_task(id);
        let absolute = self.repo_path.join(&expected);
        if let Some(path) = &path {
            if std::path::Path::new(path) != expected {
                anyhow::bail!(
                    "refusing to purge task '{}' with unsafe worktree path '{}', expected '{}'",
                    id,
                    path,
                    expected.display()
                );
            }
            if !force
                && absolute.exists()
                && crate::git::worktree_has_meaningful_changes(&absolute)?
            {
                anyhow::bail!(
                    "task '{}' worktree contains meaningful changes; use --force to purge",
                    id
                );
            }
        } else if !force
            && absolute.exists()
            && crate::git::worktree_has_meaningful_changes(&absolute)?
        {
            anyhow::bail!(
                "task '{}' worktree contains meaningful changes; use --force to purge",
                id
            );
        }
        if absolute.exists() && (force || path.is_some()) {
            crate::git::remove_worktree(&self.repo_path, &expected)?;
        }
        self.db.purge_task(id, force)?;
        Ok(())
    }
    pub fn set_agent_priority(&self, id: &str, priority: i64) -> Result<bool> {
        let result = self.db.set_agent_priority(id, priority)?;
        Ok(result)
    }
    pub fn set_agent_profile(&self, id: &str, path: &str) -> Result<bool> {
        registry::get_agent(&self.db, id)?;
        Ok(self.db.set_agent_profile_path(id, path)?)
    }
    pub fn set_agent_model(&self, id: &str, model: &str) -> Result<bool> {
        let agent = registry::get_agent(&self.db, id)?;
        if agent.backend != "codex" || agent.execution_mode != registry::AUTOMATED {
            anyhow::bail!("agent '{}' does not support Codex model settings", id)
        }
        Ok(self.db.set_agent_model(id, model)?)
    }
    pub fn set_agent_effort(&self, id: &str, effort: registry::ReasoningEffort) -> Result<bool> {
        let agent = registry::get_agent(&self.db, id)?;
        if agent.backend != "codex" || agent.execution_mode != registry::AUTOMATED {
            anyhow::bail!("agent '{}' does not support Codex reasoning settings", id)
        }
        Ok(self.db.set_agent_reasoning_effort(id, effort)?)
    }
    pub fn set_agent_quota(&self, id: &str, remaining: i64, reset: Option<&str>) -> Result<bool> {
        Ok(self.db.set_agent_quota(id, remaining, reset)?)
    }
    pub fn clear_agent_quota(&self, id: &str) -> Result<bool> {
        Ok(self.db.clear_agent_quota(id)?)
    }
    pub fn set_quota_reserve(&self, remaining: i64) -> Result<()> {
        Ok(self.db.set_quota_reserve(remaining)?)
    }
    pub fn execution_template(
        &self,
        class: crate::execution::ExecutionClass,
    ) -> Result<crate::execution::ExecutionTemplate> {
        Ok(self.db.execution_template(class)?)
    }
    pub fn set_execution_template(
        &self,
        class: crate::execution::ExecutionClass,
        model: Option<&str>,
        effort: Option<registry::ReasoningEffort>,
    ) -> Result<()> {
        Ok(self.db.set_execution_template(class, model, effort)?)
    }
    pub fn clear_execution_template(&self, class: crate::execution::ExecutionClass) -> Result<()> {
        Ok(self.db.clear_execution_template(class)?)
    }
    pub fn sync_agent_capacity(&self, id: &str) -> Result<()> {
        let agent = registry::get_agent(&self.db, id)?;
        crate::codex_app_server::sync_agent(
            &self.db,
            &agent,
            &crate::codex_app_server::CodexAppServer,
        )
        .map_err(anyhow::Error::msg)?;
        Ok(())
    }
    pub fn set_task_scope(&self, id: &str, scope: TaskScopeMode) -> Result<bool> {
        Ok(self.db.set_task_scope(id, scope)?)
    }
    pub fn set_task_required_capabilities(
        &self,
        id: &str,
        capabilities: &[String],
    ) -> Result<bool> {
        Ok(self.db.set_task_required_capabilities(id, capabilities)?)
    }
    pub fn add_task_context(&self, id: &str, paths: &[String]) -> Result<bool> {
        let task = self.task(id)?.context("task not found")?;
        let mut values = task.context_files;
        values.extend_from_slice(paths);
        Ok(self.db.set_task_context(id, &values)?)
    }
    pub fn clear_task_context(&self, id: &str) -> Result<bool> {
        Ok(self.db.set_task_context(id, &[])?)
    }
    pub fn add_expected_changes(&self, id: &str, paths: &[String]) -> Result<bool> {
        let task = self.task(id)?.context("task not found")?;
        let mut values = task.expected_changes;
        values.extend_from_slice(paths);
        Ok(self.db.set_task_expected_changes(id, &values)?)
    }
    pub fn clear_expected_changes(&self, id: &str) -> Result<bool> {
        Ok(self.db.set_task_expected_changes(id, &[])?)
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
    pub fn create_task(&self, input: CreateTaskInput) -> Result<String> {
        let project_id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        Ok(self.db.create_task(project_id, &input)?)
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
