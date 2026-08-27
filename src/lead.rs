use crate::protocol::{PROTOCOL_VERSION, PlanResponse, PlannedTask, PlanningProjectState};
use crate::storage::db::{ApprovalRequest, DbError, LifecycleEvent, WorkerResult};
use crate::storage::{AgentRun, Database};
use crate::task::Task;
use crate::{backend, registry::ReasoningEffort};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeadRole {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeadTurn {
    pub id: i64,
    pub role: LeadRole,
    pub content: String,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeadProposalStatus {
    Pending,
    Applying,
    Applied,
    Rejected,
    Superseded,
}

/// The single operator-actionable outcome of a Lead assessment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeadDecision {
    #[serde(rename = "kind")]
    pub kind: LeadDecisionKind,
    #[serde(default)]
    pub details: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedLeadDecision {
    pub id: i64,
    pub run_id: Option<i64>,
    pub created_at: String,
    pub source_request: String,
    pub summary: String,
    pub kind: LeadDecisionKind,
    pub details: String,
    pub snapshot: Option<String>,
    pub status: String,
    pub actionable: bool,
    pub resolution: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LeadDecisionKind {
    DirectTasks,
    PlanRequired,
    UserDecisionRequired,
    Approve,
    RevisePlan,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", content = "details", rename_all = "snake_case")]
pub enum LeadProposalKind {
    Plan(PlanResponse),
    Task(PlannedTask),
    Revision { task_id: String, feedback: String },
    ApprovalRequest { reason: String, details: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeadProposal {
    pub id: i64,
    pub proposal: LeadProposalKind,
    pub status: LeadProposalStatus,
    pub created_at: String,
    pub applying_at: Option<String>,
    pub resolved_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeadContext {
    pub discovery: Option<crate::discovery::ProjectDiscoverySnapshot>,
    pub project_id: i64,
    pub project_name: String,
    pub repository_path: String,
    pub engineering_contract: String,
    pub architecture: Option<String>,
    pub facts: BTreeMap<String, String>,
    pub state: PlanningProjectState,
    pub tasks: Vec<Task>,
    pub dependencies: BTreeMap<String, Vec<String>>,
    pub queue: crate::queue::QueueReport,
    pub events: Vec<LifecycleEvent>,
    pub runs: Vec<AgentRun>,
    pub results: Vec<WorkerResult>,
    pub approvals: Vec<ApprovalRequest>,
    pub agents: Vec<crate::registry::AgentDefinition>,
    pub turns: Vec<LeadTurn>,
    pub proposals: Vec<LeadProposal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeadBackendResponse {
    pub message: String,
    #[serde(default)]
    pub proposals: Vec<LeadProposalKind>,
    #[serde(default)]
    pub decision: Option<LeadDecision>,
}

pub trait LeadBackend {
    fn invoke(&self, context: &LeadContext, message: &str) -> Result<LeadBackendResponse, String>;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeadProviderConfig {
    pub agent_id: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

pub struct CodexLeadBackend {
    profile_path: Option<PathBuf>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    repo_path: PathBuf,
}

impl CodexLeadBackend {
    pub fn from_agent(
        agent: &crate::registry::AgentDefinition,
        repo_path: &Path,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Self, String> {
        if agent.backend != "codex" {
            return Err(format!(
                "Lead provider agent '{}' must use the codex backend",
                agent.id
            ));
        }
        Ok(Self {
            profile_path: agent.profile_path.as_deref().map(PathBuf::from),
            model: model.or_else(|| agent.model.clone()),
            reasoning_effort: reasoning_effort.or(agent.reasoning_effort),
            repo_path: repo_path.to_path_buf(),
        })
    }

    pub fn command_args(&self, prompt: &str) -> Vec<String> {
        let mut args = crate::worker::CodexWorker::command_args_with_execution(
            prompt,
            self.model.as_deref(),
            self.reasoning_effort,
        );
        if let Some(sandbox) = args
            .iter_mut()
            .find(|arg| arg.as_str() == "workspace-write")
        {
            *sandbox = "read-only".into();
        }
        args
    }

    fn prompt(context: &LeadContext, message: &str) -> Result<String, String> {
        let context = serde_json::to_string(context)
            .map_err(|error| format!("failed to serialize Lead context: {error}"))?;
        Ok(format!(
            "You are Orc's project Lead. You are strictly read-only: inspect the supplied persisted project and repository state only. You must not edit files, create commits, create or apply tasks, invoke Planner, dispatch, review, revise, or accept work. Return exactly one decision with kind DIRECT_TASKS, PLAN_REQUIRED, or USER_DECISION_REQUIRED, plus a message. Proposals are optional human-gated suggestions and are never applied by Lead. Respond with only structured JSON.\nProject context:\n{context}\nUser message:\n{message}"
        ))
    }

    pub fn parse_response(output: &str) -> Result<LeadBackendResponse, String> {
        serde_json::from_str(output.trim())
            .map_err(|error| format!("Lead provider returned malformed structured output: {error}"))
    }
}

impl LeadBackend for CodexLeadBackend {
    fn invoke(&self, context: &LeadContext, message: &str) -> Result<LeadBackendResponse, String> {
        let prompt = Self::prompt(context, message)?;
        let mut command = Command::new("codex");
        command.args(self.command_args(&prompt));
        if let Some(profile_path) = &self.profile_path {
            backend::apply_profile_environment(&mut command, profile_path);
        }
        backend::configure_noninteractive(&mut command, &self.repo_path);
        let output = crate::worker::run_command_with_timeout(
            command,
            crate::worker::configured_timeout(
                "ORC_LEAD_TIMEOUT_SECS",
                crate::worker::DEFAULT_WORKER_TIMEOUT,
            ),
        )?;
        if !output.status.success() {
            return Err(format!(
                "Codex Lead exited with non-zero status: {}",
                output
                    .status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| format!("Codex Lead returned non-UTF-8 output: {error}"))?;
        Self::parse_response(&stdout)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeadResponse {
    pub turn: LeadTurn,
    pub proposals: Vec<LeadProposal>,
    pub decision: Option<LeadDecision>,
}

pub struct LeadService<'a> {
    db: &'a Database,
    repo_path: &'a Path,
    require_discovery: bool,
}

impl<'a> LeadService<'a> {
    pub fn new(db: &'a Database, repo_path: &'a Path) -> Self {
        Self {
            db,
            repo_path,
            require_discovery: false,
        }
    }

    pub fn new_with_required_discovery(db: &'a Database, repo_path: &'a Path) -> Self {
        Self {
            db,
            repo_path,
            require_discovery: true,
        }
    }

    pub(crate) fn project_id(&self) -> Result<i64, DbError> {
        self.db
            .get_project_id()?
            .ok_or_else(|| DbError::Scheduler("no project found in DB".into()))
    }

    pub fn context(&self, limit: usize) -> Result<LeadContext, DbError> {
        let project_id = self.project_id()?;
        let tasks = self.db.list_tasks()?;
        let mut dependencies = BTreeMap::new();
        for task in &tasks {
            dependencies.insert(task.id.clone(), self.db.list_task_dependencies(&task.id)?);
        }
        let mut runs = self.db.list_agent_runs(project_id, limit)?;
        runs.reverse();
        let results = runs
            .iter()
            .filter_map(|run| self.db.get_worker_result(run.id).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        let mut events = self.db.list_lifecycle_events(limit)?;
        events.reverse();
        let mut turns = self.db.list_lead_turns(project_id, limit)?;
        turns.reverse();
        let mut proposals = self.db.list_lead_proposals(project_id, limit, None)?;
        proposals.reverse();
        Ok(LeadContext {
            discovery: if self.require_discovery {
                Some(
                    crate::discovery::build_snapshot(self.repo_path).map_err(|error| {
                        DbError::Scheduler(format!(
                            "structured project discovery failed: {error:#}"
                        ))
                    })?,
                )
            } else {
                crate::discovery::build_snapshot(self.repo_path).ok()
            },
            project_id,
            project_name: self.db.get_project_name()?.unwrap_or_default(),
            repository_path: self.repo_path.display().to_string(),
            engineering_contract: read_optional(self.repo_path.join(".orc/engineering.md"))?,
            architecture: read_optional_value(self.repo_path.join(".orc/architecture.md"))?,
            facts: self.db.project_facts(project_id)?,
            state: self.db.planning_project_state()?,
            tasks,
            dependencies,
            queue: crate::queue::compute_queue(self.db)
                .map_err(|error| DbError::Scheduler(error.to_string()))?,
            events,
            runs,
            results,
            approvals: self.db.list_approval_requests(project_id)?,
            agents: self.db.list_agents()?,
            turns,
            proposals,
        })
    }

    pub fn invoke(
        &self,
        message: &str,
        backend: &dyn LeadBackend,
        limit: usize,
    ) -> Result<LeadResponse, anyhow::Error> {
        self.invoke_with_run_id(message, backend, limit, None)
    }

    pub fn invoke_with_run_id(
        &self,
        message: &str,
        backend: &dyn LeadBackend,
        limit: usize,
        run_id: Option<i64>,
    ) -> Result<LeadResponse, anyhow::Error> {
        let project_id = self.project_id()?;
        self.db
            .record_lead_turn(project_id, LeadRole::User, message)?;
        let response = match backend.invoke(&self.context(limit)?, message) {
            Ok(response) => response,
            Err(error) => {
                self.db.record_lead_turn(
                    project_id,
                    LeadRole::System,
                    &format!("Lead invocation failed: {error}"),
                )?;
                return Err(anyhow::Error::msg(error));
            }
        };
        for proposal in &response.proposals {
            match proposal {
                LeadProposalKind::Plan(plan) => plan.validate().map_err(|error| {
                    anyhow::anyhow!("Lead returned invalid plan proposal: {error}")
                })?,
                LeadProposalKind::Task(task) => task.validate().map_err(|error| {
                    anyhow::anyhow!("Lead returned invalid task proposal: {error}")
                })?,
                _ => {}
            }
        }
        let turn_id =
            self.db
                .record_lead_turn(project_id, LeadRole::Assistant, &response.message)?;
        let turn = self.db.get_lead_turn(project_id, turn_id)?.ok_or_else(|| {
            anyhow::anyhow!("persisted Lead assistant turn could not be reloaded")
        })?;
        if let Some(decision) = response.decision.as_ref() {
            let snapshot = serde_json::to_string(&self.context(limit)?)?;
            self.db.record_lead_decision(
                project_id,
                &decision.kind,
                &decision.details,
                crate::storage::db::LeadDecisionMetadata {
                    snapshot: &snapshot,
                    run_id,
                    source_request: message,
                    summary: &response.message,
                },
            )?;
        }
        let mut proposals = Vec::new();
        for proposal in response.proposals {
            let id = self.db.record_lead_proposal(project_id, &proposal)?;
            proposals.push(
                self.db.get_lead_proposal(project_id, id)?.ok_or_else(|| {
                    anyhow::anyhow!("persisted Lead proposal could not be reloaded")
                })?,
            );
        }
        Ok(LeadResponse {
            turn,
            proposals,
            decision: response.decision,
        })
    }

    pub fn pending_proposals(&self) -> Result<Vec<LeadProposal>, DbError> {
        self.db.list_lead_proposals(
            self.project_id()?,
            usize::MAX,
            Some(LeadProposalStatus::Pending),
        )
    }

    pub fn reject_proposal(&self, id: i64) -> Result<bool, DbError> {
        self.db
            .resolve_lead_proposal(self.project_id()?, id, LeadProposalStatus::Rejected)
    }

    pub(crate) fn proposal(&self, id: i64) -> Result<Option<LeadProposal>, DbError> {
        self.db.get_lead_proposal(self.project_id()?, id)
    }
    pub(crate) fn claim_proposal(&self, id: i64) -> Result<bool, DbError> {
        self.db.transition_lead_proposal(
            self.project_id()?,
            id,
            LeadProposalStatus::Pending,
            LeadProposalStatus::Applying,
        )
    }
    pub(crate) fn finish_proposal(&self, id: i64) -> Result<bool, DbError> {
        self.db.transition_lead_proposal(
            self.project_id()?,
            id,
            LeadProposalStatus::Applying,
            LeadProposalStatus::Applied,
        )
    }
    pub(crate) fn release_proposal(&self, id: i64) -> Result<bool, DbError> {
        self.db.transition_lead_proposal(
            self.project_id()?,
            id,
            LeadProposalStatus::Applying,
            LeadProposalStatus::Pending,
        )
    }

    pub fn recover_proposal(&self, id: i64) -> Result<bool, DbError> {
        self.release_proposal(id)
    }
}

fn read_optional(path: PathBuf) -> Result<String, DbError> {
    Ok(read_optional_value(path)?.unwrap_or_default())
}
fn read_optional_value(path: PathBuf) -> Result<Option<String>, DbError> {
    match std::fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn single_task_plan(task: PlannedTask) -> PlanResponse {
    PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: task.objective.clone(),
        assumptions: Vec::new(),
        risks: Vec::new(),
        questions: Vec::new(),
        tasks: vec![task],
    }
}
