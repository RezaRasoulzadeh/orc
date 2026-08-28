use crate::protocol::{PROTOCOL_VERSION, PlanResponse, PlannedTask, PlanningProjectState};
use crate::registry::ReasoningEffort;
use crate::storage::db::{ApprovalRequest, DbError, LifecycleEvent, WorkerResult};
use crate::storage::{AgentRun, Database};
use crate::task::Task;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// Kept as a compatibility re-export for callers that used the old location.
pub use crate::backend::CodexLeadBackend;

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
    pub resolved_at: Option<String>,
    pub superseded_by_id: Option<i64>,
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

impl LeadProposalKind {
    /// Validate the provider-independent proposal after tagged deserialization.
    /// Provider schemas may flatten variants for transport compatibility, so
    /// variant-specific semantics are deliberately enforced here.
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Plan(plan) => plan
                .validate()
                .map_err(|error| anyhow::anyhow!("invalid plan proposal: {error}")),
            Self::Task(task) => task
                .validate()
                .map_err(|error| anyhow::anyhow!("invalid task proposal: {error}")),
            Self::Revision { task_id, feedback } => {
                if task_id.trim().is_empty() || feedback.trim().is_empty() {
                    anyhow::bail!("revision proposals require a task_id and feedback")
                }
                Ok(())
            }
            Self::ApprovalRequest { reason, details } => {
                if reason.trim().is_empty() || details.trim().is_empty() {
                    anyhow::bail!("approval proposals require a reason and details")
                }
                Ok(())
            }
        }
    }
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
            proposal
                .validate()
                .map_err(|error| anyhow::anyhow!("Lead returned invalid proposal: {error}"))?;
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
