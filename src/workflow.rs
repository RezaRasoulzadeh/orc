//! Persisted, restart-safe orchestration for Orc's complete product lifecycle.
//!
//! Existing command APIs remain deliberately one-shot. This module is the one
//! authoritative continuation path which composes those boundaries and
//! persists every legal edge before proceeding.

use anyhow::{Context, Result};

use crate::lead::LeadDecisionKind;
use crate::storage::Database;
use crate::task::TaskStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Running,
    WaitingUser,
    AcceptanceReady,
    WaitingExternal,
    Blocked,
    BudgetExhausted,
    NonConvergent,
    Cancelled,
    Superseded,
    Completed,
}

impl WorkflowStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::WaitingUser => "waiting_user",
            Self::AcceptanceReady => "acceptance_ready",
            Self::WaitingExternal => "waiting_external",
            Self::Blocked => "blocked",
            Self::BudgetExhausted => "budget_exhausted",
            Self::NonConvergent => "non_convergent",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
            Self::Completed => "completed",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Blocked
                | Self::BudgetExhausted
                | Self::NonConvergent
                | Self::Cancelled
                | Self::Superseded
                | Self::Completed
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStage {
    Discovery,
    Lead,
    ApplyDirect,
    Planner,
    PlannerRevision,
    PlanReview,
    ApplyPlan,
    Tasks,
    Dispatch,
    Review,
    Revision,
    Acceptance,
    Done,
}

impl WorkflowStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Lead => "lead",
            Self::ApplyDirect => "apply_direct",
            Self::Planner => "planner",
            Self::PlannerRevision => "planner_revision",
            Self::PlanReview => "plan_review",
            Self::ApplyPlan => "apply_plan",
            Self::Tasks => "tasks",
            Self::Dispatch => "dispatch",
            Self::Review => "review",
            Self::Revision => "revision",
            Self::Acceptance => "acceptance",
            Self::Done => "done",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPlanPath {
    #[default]
    Legacy,
    Controller,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    Agent,
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptancePolicy {
    User,
    Automatic,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowPolicy {
    pub plan_approval: ApprovalPolicy,
    pub acceptance: AcceptancePolicy,
    pub max_plan_revisions: usize,
    pub max_task_revisions: usize,
    pub max_transitions: usize,
}

impl Default for WorkflowPolicy {
    fn default() -> Self {
        Self {
            plan_approval: ApprovalPolicy::Agent,
            acceptance: AcceptancePolicy::User,
            max_plan_revisions: 2,
            max_task_revisions: 2,
            max_transitions: 128,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowRun {
    pub id: i64,
    pub project_id: i64,
    pub objective: String,
    pub status: WorkflowStatus,
    pub stage: WorkflowStage,
    #[serde(default)]
    pub plan_path: WorkflowPlanPath,
    pub version: i64,
    pub policy: WorkflowPolicy,
    pub transition_count: usize,
    pub plan_revision_count: usize,
    pub task_revision_count: usize,
    pub current_task_id: Option<String>,
    pub lead_decision_id: Option<i64>,
    pub plan_id: Option<i64>,
    pub provider_run_id: Option<i64>,
    pub revision_feedback: Option<String>,
    pub resume_stage: Option<WorkflowStage>,
    pub user_resolution: Option<String>,
    pub discovery_fingerprint: Option<String>,
    pub stop_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowTransitionRecord {
    pub id: i64,
    pub workflow_id: i64,
    pub from_stage: String,
    pub to_stage: String,
    pub from_status: String,
    pub to_status: String,
    pub edge: String,
    pub deterministic: bool,
    pub provider_run_id: Option<i64>,
    pub details: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeadOutcome {
    pub decision_id: i64,
    pub provider_run_id: i64,
    pub kind: LeadDecisionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanOutcome {
    pub plan_id: i64,
    pub provider_run_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanReviewOutcome {
    pub provider_run_id: i64,
    pub decision: LeadDecisionKind,
}

/// Result of one Controller Plan persistence boundary. Controller inference
/// has no provider-run identity in the workflow journal, so this outcome only
/// carries the canonical Plan identity returned by trusted storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerPlanOutcome {
    pub plan_id: i64,
}

/// Result of one Controller Plan-review persistence boundary. The workflow
/// kernel owns the transition mapping; Controller output cannot supply any
/// workflow or persistence metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerPlanReviewDecision {
    Approve,
    RevisePlan,
    UserDecisionRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerPlanReviewOutcome {
    pub decision: ControllerPlanReviewDecision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewOutcome {
    pub provider_run_id: i64,
    pub verdict: String,
    pub feedback: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderOutcome {
    pub provider_run_id: i64,
}

/// Semantic and mutation boundaries used by the workflow. Implementations may
/// invoke a provider only in `lead`, `plan`, `revise_plan`, `review_plan`,
/// `dispatch`, `review`, and `revise_task`. All other methods are deterministic.
pub trait WorkflowActions {
    fn discover(&self) -> Result<String>;
    fn lead(&self, workflow: &WorkflowRun) -> Result<LeadOutcome>;
    fn apply_direct(&self) -> Result<()>;
    fn plan(&self) -> Result<PlanOutcome>;
    fn revise_plan(&self) -> Result<PlanOutcome>;
    fn review_plan(&self, workflow: &WorkflowRun, plan_id: i64) -> Result<PlanReviewOutcome>;
    fn apply_plan(&self) -> Result<()>;
    fn dispatch(&self, task_id: &str) -> Result<ProviderOutcome>;
    fn review(&self, task_id: &str) -> Result<ReviewOutcome>;
    fn revise_task(&self, task_id: &str, feedback: &str) -> Result<ProviderOutcome>;
    fn accept(&self, task_id: &str) -> Result<()>;

    fn recover_lead(&self, _: &WorkflowRun) -> Result<Option<LeadOutcome>> {
        Ok(None)
    }
    fn recover_plan(&self, _: &WorkflowRun) -> Result<Option<PlanOutcome>> {
        Ok(None)
    }
    fn recover_plan_review(&self, _: &WorkflowRun) -> Result<Option<PlanReviewOutcome>> {
        Ok(None)
    }
    fn recover_dispatch(&self, _: &WorkflowRun) -> Result<Option<ProviderOutcome>> {
        Ok(None)
    }
    fn recover_review(&self, _: &WorkflowRun) -> Result<Option<ReviewOutcome>> {
        Ok(None)
    }
    fn recover_revision(&self, _: &WorkflowRun) -> Result<Option<ProviderOutcome>> {
        Ok(None)
    }

    /// Optional Controller-routed Plan boundaries. The default keeps custom
    /// and legacy action implementations source-compatible; the production
    /// Controller adapter opts in explicitly when a runtime is supplied to
    /// the engine.
    fn controller_plan(
        &self,
        _: &WorkflowRun,
        _: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanOutcome>> {
        Ok(None)
    }

    fn controller_plan_revision(
        &self,
        _: &WorkflowRun,
        _: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanOutcome>> {
        Ok(None)
    }

    fn controller_plan_review(
        &self,
        _: &WorkflowRun,
        _: i64,
        _: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanReviewOutcome>> {
        Ok(None)
    }

    fn recover_controller_plan(&self, _: &WorkflowRun) -> Result<Option<ControllerPlanOutcome>> {
        Ok(None)
    }

    fn recover_controller_plan_review(
        &self,
        _: &WorkflowRun,
    ) -> Result<Option<ControllerPlanReviewOutcome>> {
        Ok(None)
    }
}

pub struct WorkflowEngine<'a, A: WorkflowActions> {
    db: &'a Database,
    actions: &'a A,
}

impl<'a, A: WorkflowActions> WorkflowEngine<'a, A> {
    pub const fn new(db: &'a Database, actions: &'a A) -> Self {
        Self { db, actions }
    }

    pub fn start(
        &self,
        project_id: i64,
        objective: &str,
        policy: WorkflowPolicy,
    ) -> Result<WorkflowRun> {
        if objective.trim().is_empty() {
            anyhow::bail!("workflow objective must not be empty")
        }
        let run = self.db.start_workflow(project_id, objective, &policy)?;
        self.continue_run(run.id)
    }

    /// Start a workflow with an explicitly supplied local Controller runtime.
    /// The runtime is used only by the Controller Plan boundaries; all stage,
    /// status, approval, and application decisions remain in this engine.
    pub fn start_with_controller_runtime(
        &self,
        project_id: i64,
        objective: &str,
        policy: WorkflowPolicy,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<WorkflowRun> {
        if objective.trim().is_empty() {
            anyhow::bail!("workflow objective must not be empty")
        }
        let run = self
            .db
            .start_controller_workflow(project_id, objective, &policy)?;
        self.continue_run_with_controller_runtime(run.id, runtime)
    }

    pub fn continue_run(&self, workflow_id: i64) -> Result<WorkflowRun> {
        self.continue_run_inner(workflow_id, None)
    }

    pub fn continue_run_with_controller_runtime(
        &self,
        workflow_id: i64,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<WorkflowRun> {
        self.continue_run_inner(workflow_id, Some(runtime))
    }

    fn continue_run_inner(
        &self,
        workflow_id: i64,
        mut runtime: Option<&mut dyn crate::local_runtime::LocalInferenceRuntime>,
    ) -> Result<WorkflowRun> {
        let mut initial = self
            .db
            .get_workflow(workflow_id)?
            .context("workflow not found")?;
        if initial.status == WorkflowStatus::WaitingExternal {
            initial = self.reconcile_external_wait(&initial)?;
        }
        if initial.status != WorkflowStatus::Running {
            return Ok(initial);
        }
        let remaining = initial
            .policy
            .max_transitions
            .saturating_sub(initial.transition_count);
        for _ in 0..remaining {
            let committed = match runtime.as_mut() {
                Some(runtime) => self.continue_one_inner(workflow_id, Some(&mut **runtime))?,
                None => self.continue_one_inner(workflow_id, None)?,
            };
            if committed.status != WorkflowStatus::Running {
                return Ok(committed);
            }
        }
        let current = self
            .db
            .get_workflow(workflow_id)?
            .context("workflow disappeared")?;
        let mut stopped = current.clone();
        stopped.status = WorkflowStatus::NonConvergent;
        stopped.stop_reason = Some("workflow transition budget exhausted".into());
        self.db
            .commit_workflow_transition(
                &current,
                &stopped,
                "transition_budget_exhausted",
                true,
                None,
                stopped.stop_reason.as_deref(),
            )
            .map_err(Into::into)
    }

    /// Reconcile an external/manual dispatch without replaying the completed
    /// dispatch provider call. The task row is the authoritative completion
    /// boundary and this transition is deliberately deterministic.
    fn reconcile_external_wait(&self, current: &WorkflowRun) -> Result<WorkflowRun> {
        let task_id = current
            .current_task_id
            .as_deref()
            .context("external wait has no current task")?;
        let task = self
            .db
            .get_task(task_id)?
            .context("externally dispatched task disappeared")?;
        let mut next = current.clone();
        let edge = match task.status {
            TaskStatus::Active => return Ok(current.clone()),
            TaskStatus::Review => {
                next.status = WorkflowStatus::Running;
                next.stage = WorkflowStage::Review;
                next.stop_reason = None;
                "external_task_ready_for_review"
            }
            TaskStatus::AcceptanceReady => {
                next.status = WorkflowStatus::AcceptanceReady;
                next.stage = WorkflowStage::Acceptance;
                next.resume_stage = Some(WorkflowStage::Acceptance);
                next.stop_reason = Some("configured user acceptance required".into());
                "external_task_ready_for_acceptance"
            }
            TaskStatus::RevisionRequired => {
                next.status = WorkflowStatus::Running;
                next.stage = WorkflowStage::Revision;
                next.stop_reason = None;
                "external_task_requires_revision"
            }
            TaskStatus::Ready | TaskStatus::Backlog => {
                next.status = WorkflowStatus::Running;
                next.stage = WorkflowStage::Tasks;
                next.current_task_id = None;
                next.stop_reason = None;
                "external_task_requeued"
            }
            TaskStatus::Done => {
                next.status = WorkflowStatus::Running;
                next.stage = WorkflowStage::Tasks;
                next.current_task_id = None;
                next.stop_reason = None;
                "external_task_completed"
            }
            TaskStatus::Blocked => {
                next.status = WorkflowStatus::Blocked;
                next.stop_reason = Some("external task became blocked".into());
                "external_task_blocked"
            }
            TaskStatus::Cancelled => {
                next.status = WorkflowStatus::Cancelled;
                next.stop_reason = Some("external task was cancelled".into());
                "external_task_cancelled"
            }
        };
        self.db
            .commit_workflow_transition(
                current,
                &next,
                edge,
                true,
                None,
                next.stop_reason.as_deref(),
            )
            .map_err(Into::into)
    }

    pub fn resolve_user_gate(&self, workflow_id: i64, resolution: &str) -> Result<WorkflowRun> {
        self.resolve_user_gate_inner(workflow_id, resolution, None)
    }

    pub fn resolve_user_gate_with_controller_runtime(
        &self,
        workflow_id: i64,
        resolution: &str,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<WorkflowRun> {
        self.resolve_user_gate_inner(workflow_id, resolution, Some(runtime))
    }

    fn resolve_user_gate_inner(
        &self,
        workflow_id: i64,
        resolution: &str,
        runtime: Option<&mut dyn crate::local_runtime::LocalInferenceRuntime>,
    ) -> Result<WorkflowRun> {
        let current = self
            .db
            .get_workflow(workflow_id)?
            .context("workflow not found")?;
        if !matches!(
            current.status,
            WorkflowStatus::WaitingUser | WorkflowStatus::AcceptanceReady
        ) {
            anyhow::bail!("workflow is not waiting for a user decision")
        }
        let normalized = resolution.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            anyhow::bail!("workflow resolution must not be empty")
        }
        if matches!(
            normalized.as_str(),
            "cancel" | "cancelled" | "reject" | "deny" | "decline" | "no"
        ) {
            let mut next = current.clone();
            next.status = WorkflowStatus::Cancelled;
            next.stop_reason = Some(format!("user resolved gate with '{resolution}'"));
            return Ok(self.db.commit_workflow_transition(
                &current,
                &next,
                "user_cancelled",
                true,
                None,
                next.stop_reason.as_deref(),
            )?);
        }
        let is_approval = matches!(
            normalized.as_str(),
            "accept" | "approve" | "approved" | "continue" | "yes"
        );
        if (current.status == WorkflowStatus::AcceptanceReady
            || current.resume_stage == Some(WorkflowStage::ApplyPlan))
            && !is_approval
        {
            anyhow::bail!(
                "this workflow gate requires an explicit accept/approve/continue or cancel/reject resolution"
            )
        }
        if let Some(decision_id) = current.lead_decision_id {
            let _ = self
                .db
                .resolve_user_decision(current.project_id, decision_id, resolution);
        }
        let mut next = current.clone();
        next.status = WorkflowStatus::Running;
        next.stage = current.resume_stage.unwrap_or(current.stage);
        next.resume_stage = None;
        next.user_resolution = Some(resolution.trim().to_owned());
        next.stop_reason = None;
        let resumed = self.db.commit_workflow_transition(
            &current,
            &next,
            "user_resolved",
            true,
            None,
            Some(resolution),
        )?;
        self.continue_run_inner(resumed.id, runtime)
    }

    pub fn cancel(&self, workflow_id: i64, reason: Option<&str>) -> Result<WorkflowRun> {
        let current = self
            .db
            .get_workflow(workflow_id)?
            .context("workflow not found")?;
        if current.status.is_terminal() {
            anyhow::bail!("workflow is already terminal")
        }
        let mut next = current.clone();
        next.status = WorkflowStatus::Cancelled;
        next.stop_reason = Some(reason.unwrap_or("operator cancelled workflow").to_owned());
        self.db
            .commit_workflow_transition(
                &current,
                &next,
                "cancelled",
                true,
                None,
                next.stop_reason.as_deref(),
            )
            .map_err(Into::into)
    }

    /// Commit at most one legal edge. This is useful for controlled operation
    /// and makes restart tests exercise the exact same production transition
    /// code as continuous orchestration.
    pub fn continue_one(&self, workflow_id: i64) -> Result<WorkflowRun> {
        self.continue_one_inner(workflow_id, None)
    }

    fn continue_one_inner(
        &self,
        workflow_id: i64,
        runtime: Option<&mut dyn crate::local_runtime::LocalInferenceRuntime>,
    ) -> Result<WorkflowRun> {
        let current = self
            .db
            .get_workflow(workflow_id)?
            .context("workflow disappeared")?;
        if current.status != WorkflowStatus::Running {
            return Ok(current);
        }
        let next = match self.advance(&current, runtime) {
            Ok(next) => next,
            Err(error) => self.stop_for_error(&current, &error)?,
        };
        self.db
            .commit_workflow_transition(
                &current,
                &next.run,
                &next.edge,
                next.deterministic,
                next.provider_run_id,
                next.details.as_deref(),
            )
            .map_err(Into::into)
    }

    fn advance(
        &self,
        current: &WorkflowRun,
        mut runtime: Option<&mut dyn crate::local_runtime::LocalInferenceRuntime>,
    ) -> Result<NextTransition> {
        match current.stage {
            WorkflowStage::Discovery => {
                let fingerprint = self.actions.discover()?;
                let mut next = current.clone();
                next.stage = WorkflowStage::Lead;
                next.discovery_fingerprint = Some(fingerprint.clone());
                Ok(NextTransition::deterministic(
                    next,
                    "discovery_completed",
                    Some(fingerprint),
                ))
            }
            WorkflowStage::Lead => {
                let outcome = self
                    .actions
                    .recover_lead(current)?
                    .map_or_else(|| self.actions.lead(current), Ok)?;
                let mut next = current.clone();
                next.lead_decision_id = Some(outcome.decision_id);
                next.provider_run_id = Some(outcome.provider_run_id);
                next.user_resolution = None;
                match outcome.kind {
                    LeadDecisionKind::DirectTasks => next.stage = WorkflowStage::ApplyDirect,
                    LeadDecisionKind::PlanRequired => next.stage = WorkflowStage::Planner,
                    LeadDecisionKind::UserDecisionRequired => {
                        next.status = WorkflowStatus::WaitingUser;
                        next.resume_stage = Some(WorkflowStage::Lead);
                        next.stop_reason = Some("Lead requires a user decision".into());
                    }
                    other => anyhow::bail!("Lead returned illegal intake decision {other:?}"),
                }
                Ok(NextTransition::provider(
                    next,
                    "lead_decided",
                    outcome.provider_run_id,
                ))
            }
            WorkflowStage::ApplyDirect => {
                self.actions.apply_direct()?;
                let mut next = current.clone();
                next.stage = WorkflowStage::Tasks;
                Ok(NextTransition::deterministic(
                    next,
                    "direct_tasks_applied",
                    None,
                ))
            }
            WorkflowStage::Planner => {
                if current.plan_path == WorkflowPlanPath::Controller {
                    let outcome =
                        if let Some(outcome) = self.actions.recover_controller_plan(current)? {
                            outcome
                        } else {
                            let runtime = runtime
                                .as_deref_mut()
                                .context("Controller runtime is required for this workflow")?;
                            self.actions
                                .controller_plan(current, runtime)?
                                .context("Controller Plan adapter returned no outcome")?
                        };
                    let mut next = current.clone();
                    next.stage = WorkflowStage::PlanReview;
                    next.plan_id = Some(outcome.plan_id);
                    return Ok(NextTransition::semantic(next, "plan_proposed"));
                }
                let outcome = self
                    .actions
                    .recover_plan(current)?
                    .map_or_else(|| self.actions.plan(), Ok)?;
                let mut next = current.clone();
                next.stage = WorkflowStage::PlanReview;
                next.plan_id = Some(outcome.plan_id);
                next.provider_run_id = Some(outcome.provider_run_id);
                Ok(NextTransition::provider(
                    next,
                    "plan_proposed",
                    outcome.provider_run_id,
                ))
            }
            WorkflowStage::PlannerRevision => {
                if current.plan_revision_count >= current.policy.max_plan_revisions {
                    return Ok(self.non_convergent(current, "plan revision limit exhausted"));
                }
                if current.plan_path == WorkflowPlanPath::Controller {
                    let outcome =
                        if let Some(outcome) = self.actions.recover_controller_plan(current)? {
                            outcome
                        } else {
                            let runtime = runtime
                                .as_deref_mut()
                                .context("Controller runtime is required for this workflow")?;
                            self.actions
                                .controller_plan_revision(current, runtime)?
                                .context("Controller Plan revision adapter returned no outcome")?
                        };
                    let mut next = current.clone();
                    next.stage = WorkflowStage::PlanReview;
                    next.plan_id = Some(outcome.plan_id);
                    next.plan_revision_count += 1;
                    return Ok(NextTransition::semantic(next, "plan_revised"));
                }
                let outcome = self
                    .actions
                    .recover_plan(current)?
                    .map_or_else(|| self.actions.revise_plan(), Ok)?;
                let mut next = current.clone();
                next.stage = WorkflowStage::PlanReview;
                next.plan_id = Some(outcome.plan_id);
                next.provider_run_id = Some(outcome.provider_run_id);
                next.plan_revision_count += 1;
                Ok(NextTransition::provider(
                    next,
                    "plan_revised",
                    outcome.provider_run_id,
                ))
            }
            WorkflowStage::PlanReview => {
                let plan_id = current.plan_id.context("workflow has no current plan")?;
                if current.plan_path == WorkflowPlanPath::Controller {
                    let outcome = if let Some(outcome) =
                        self.actions.recover_controller_plan_review(current)?
                    {
                        outcome
                    } else {
                        let runtime =
                            runtime.context("Controller runtime is required for this workflow")?;
                        self.actions
                            .controller_plan_review(current, plan_id, runtime)?
                            .context("Controller Plan review adapter returned no outcome")?
                    };
                    let mut next = current.clone();
                    next.user_resolution = None;
                    match outcome.decision {
                        ControllerPlanReviewDecision::Approve => {
                            next.stage = WorkflowStage::ApplyPlan;
                            if current.policy.plan_approval == ApprovalPolicy::User {
                                next.status = WorkflowStatus::WaitingUser;
                                next.resume_stage = Some(WorkflowStage::ApplyPlan);
                                next.stop_reason =
                                    Some("configured user plan approval required".into());
                            }
                        }
                        ControllerPlanReviewDecision::RevisePlan => {
                            next.stage = WorkflowStage::PlannerRevision
                        }
                        ControllerPlanReviewDecision::UserDecisionRequired => {
                            next.status = WorkflowStatus::WaitingUser;
                            next.resume_stage = Some(WorkflowStage::PlanReview);
                            next.stop_reason =
                                Some("Controller plan review requires a user decision".into());
                        }
                    }
                    return Ok(NextTransition::semantic(next, "plan_reviewed"));
                }
                let outcome = self
                    .actions
                    .recover_plan_review(current)?
                    .map_or_else(|| self.actions.review_plan(current, plan_id), Ok)?;
                let mut next = current.clone();
                next.provider_run_id = Some(outcome.provider_run_id);
                next.user_resolution = None;
                match outcome.decision {
                    LeadDecisionKind::Approve => {
                        next.stage = WorkflowStage::ApplyPlan;
                        if current.policy.plan_approval == ApprovalPolicy::User {
                            next.status = WorkflowStatus::WaitingUser;
                            next.resume_stage = Some(WorkflowStage::ApplyPlan);
                            next.stop_reason =
                                Some("configured user plan approval required".into());
                        }
                    }
                    LeadDecisionKind::RevisePlan => next.stage = WorkflowStage::PlannerRevision,
                    LeadDecisionKind::UserDecisionRequired => {
                        next.status = WorkflowStatus::WaitingUser;
                        next.resume_stage = Some(WorkflowStage::PlanReview);
                        next.stop_reason = Some("Lead plan review requires a user decision".into());
                    }
                    other => anyhow::bail!("Lead returned illegal plan review decision {other:?}"),
                }
                Ok(NextTransition::provider(
                    next,
                    "plan_reviewed",
                    outcome.provider_run_id,
                ))
            }
            WorkflowStage::ApplyPlan => {
                self.actions.apply_plan()?;
                let mut next = current.clone();
                next.stage = WorkflowStage::Tasks;
                next.current_task_id = None;
                next.user_resolution = None;
                Ok(NextTransition::deterministic(next, "plan_applied", None))
            }
            WorkflowStage::Tasks => self.route_tasks(current),
            WorkflowStage::Dispatch => {
                let task_id = current
                    .current_task_id
                    .as_deref()
                    .context("dispatch stage has no task")?;
                let outcome = self
                    .actions
                    .recover_dispatch(current)?
                    .map_or_else(|| self.actions.dispatch(task_id), Ok)?;
                let task = self
                    .db
                    .get_task(task_id)?
                    .context("dispatched task disappeared")?;
                let mut next = current.clone();
                next.provider_run_id = Some(outcome.provider_run_id);
                match task.status {
                    TaskStatus::Review => next.stage = WorkflowStage::Review,
                    TaskStatus::Active => {
                        next.status = WorkflowStatus::WaitingExternal;
                        next.stop_reason =
                            Some("task is waiting for an external/manual agent".into());
                    }
                    TaskStatus::Blocked => {
                        next.status = WorkflowStatus::Blocked;
                        next.stop_reason = Some("dispatch left task blocked".into());
                    }
                    status => {
                        anyhow::bail!("dispatch completed in illegal task state '{}'", status)
                    }
                }
                Ok(NextTransition::provider(
                    next,
                    "task_dispatched_and_validated",
                    outcome.provider_run_id,
                ))
            }
            WorkflowStage::Review => {
                let task_id = current
                    .current_task_id
                    .as_deref()
                    .context("review stage has no task")?;
                let outcome = self
                    .actions
                    .recover_review(current)?
                    .map_or_else(|| self.actions.review(task_id), Ok)?;
                let mut next = current.clone();
                next.provider_run_id = Some(outcome.provider_run_id);
                match outcome.verdict.trim().to_ascii_uppercase().as_str() {
                    "PASS" => {
                        next.stage = WorkflowStage::Acceptance;
                        if current.policy.acceptance == AcceptancePolicy::User {
                            next.status = WorkflowStatus::AcceptanceReady;
                            next.resume_stage = Some(WorkflowStage::Acceptance);
                            next.stop_reason = Some("configured user acceptance required".into());
                        }
                    }
                    "REVISE" => {
                        if current.task_revision_count >= current.policy.max_task_revisions {
                            return Ok(
                                self.non_convergent(current, "task revision limit exhausted")
                            );
                        }
                        next.stage = WorkflowStage::Revision;
                        next.revision_feedback = outcome.feedback;
                    }
                    "REJECT" => {
                        next.status = WorkflowStatus::Blocked;
                        next.stop_reason = Some("review rejected the implementation".into());
                    }
                    verdict => anyhow::bail!("review returned invalid verdict '{verdict}'"),
                }
                Ok(NextTransition::provider(
                    next,
                    "task_reviewed",
                    outcome.provider_run_id,
                ))
            }
            WorkflowStage::Revision => {
                let task_id = current
                    .current_task_id
                    .as_deref()
                    .context("revision stage has no task")?;
                let feedback = current
                    .revision_feedback
                    .as_deref()
                    .unwrap_or("Resolve the persisted review blockers.");
                let outcome = self
                    .actions
                    .recover_revision(current)?
                    .map_or_else(|| self.actions.revise_task(task_id, feedback), Ok)?;
                let task = self
                    .db
                    .get_task(task_id)?
                    .context("revised task disappeared")?;
                let mut next = current.clone();
                next.provider_run_id = Some(outcome.provider_run_id);
                next.task_revision_count += 1;
                next.revision_feedback = None;
                match task.status {
                    TaskStatus::Review => next.stage = WorkflowStage::Review,
                    TaskStatus::Blocked => {
                        next.status = WorkflowStatus::Blocked;
                        next.stop_reason = Some("revision left task blocked".into());
                    }
                    status => anyhow::bail!("revision completed in illegal task state '{status}'"),
                }
                Ok(NextTransition::provider(
                    next,
                    "task_revised_and_validated",
                    outcome.provider_run_id,
                ))
            }
            WorkflowStage::Acceptance => {
                let task_id = current
                    .current_task_id
                    .as_deref()
                    .context("acceptance stage has no task")?;
                self.actions.accept(task_id)?;
                let mut next = current.clone();
                next.stage = WorkflowStage::Tasks;
                next.current_task_id = None;
                next.task_revision_count = 0;
                next.stop_reason = None;
                next.user_resolution = None;
                Ok(NextTransition::deterministic(next, "task_accepted", None))
            }
            WorkflowStage::Done => {
                let mut next = current.clone();
                next.status = WorkflowStatus::Completed;
                Ok(NextTransition::deterministic(next, "completed", None))
            }
        }
    }

    fn route_tasks(&self, current: &WorkflowRun) -> Result<NextTransition> {
        let tasks = self.db.list_tasks_for_project(current.project_id)?;
        if tasks.is_empty() {
            let mut next = current.clone();
            next.stage = WorkflowStage::Done;
            return Ok(NextTransition::deterministic(
                next,
                "no_tasks_remaining",
                None,
            ));
        }
        if let Some(task) = tasks.iter().find(|task| task.status == TaskStatus::Review) {
            let mut next = current.clone();
            next.stage = WorkflowStage::Review;
            next.current_task_id = Some(task.id.clone());
            return Ok(NextTransition::deterministic(
                next,
                "review_task_selected",
                None,
            ));
        }
        if let Some(task) = tasks
            .iter()
            .find(|task| task.status == TaskStatus::RevisionRequired)
        {
            let mut next = current.clone();
            next.stage = WorkflowStage::Revision;
            next.current_task_id = Some(task.id.clone());
            return Ok(NextTransition::deterministic(
                next,
                "revision_required_task_selected",
                None,
            ));
        }
        if let Some(task) = tasks
            .iter()
            .find(|task| task.status == TaskStatus::AcceptanceReady)
        {
            let mut next = current.clone();
            next.stage = WorkflowStage::Acceptance;
            next.current_task_id = Some(task.id.clone());
            if current.policy.acceptance == AcceptancePolicy::User {
                next.status = WorkflowStatus::AcceptanceReady;
                next.resume_stage = Some(WorkflowStage::Acceptance);
                next.stop_reason = Some("configured user acceptance required".into());
            }
            return Ok(NextTransition::deterministic(
                next,
                "acceptance_ready_task_selected",
                None,
            ));
        }
        if let Some(task) = tasks.iter().find(|task| task.status == TaskStatus::Active) {
            let mut next = current.clone();
            next.status = WorkflowStatus::WaitingExternal;
            next.current_task_id = Some(task.id.clone());
            next.stop_reason = Some("active task requires external completion".into());
            return Ok(NextTransition::deterministic(
                next,
                "active_task_wait",
                None,
            ));
        }
        if tasks.iter().all(|task| task.status.is_terminal()) {
            let mut next = current.clone();
            if tasks
                .iter()
                .any(|task| task.status == TaskStatus::Cancelled)
            {
                next.status = WorkflowStatus::Cancelled;
                next.stop_reason = Some("one or more workflow tasks were cancelled".into());
                return Ok(NextTransition::deterministic(next, "task_cancelled", None));
            }
            next.stage = WorkflowStage::Done;
            return Ok(NextTransition::deterministic(
                next,
                "all_tasks_terminal",
                None,
            ));
        }
        let queue = crate::queue::compute_queue(self.db)?;
        if let Some(entry) = queue.ready.first() {
            let mut next = current.clone();
            next.stage = WorkflowStage::Dispatch;
            next.current_task_id = Some(entry.task.id.clone());
            return Ok(NextTransition::deterministic(
                next,
                "dispatch_task_selected",
                None,
            ));
        }
        let mut next = current.clone();
        next.status = WorkflowStatus::Blocked;
        next.stop_reason = Some(
            if tasks.iter().any(|task| task.status == TaskStatus::Blocked) {
                "one or more tasks are genuinely blocked".into()
            } else {
                "no dependency-safe task has an eligible available agent".into()
            },
        );
        Ok(NextTransition::deterministic(
            next,
            "task_scheduling_blocked",
            None,
        ))
    }

    fn non_convergent(&self, current: &WorkflowRun, reason: &str) -> NextTransition {
        let mut next = current.clone();
        next.status = WorkflowStatus::NonConvergent;
        next.stop_reason = Some(reason.into());
        NextTransition::deterministic(next, "non_convergence", Some(reason.into()))
    }

    fn stop_for_error(
        &self,
        current: &WorkflowRun,
        error: &anyhow::Error,
    ) -> Result<NextTransition> {
        let message = format!("{error:#}");
        let lower = message.to_ascii_lowercase();
        let status = if lower.contains("token budget")
            || lower.contains("invocation budget")
            || lower.contains("quota")
        {
            WorkflowStatus::BudgetExhausted
        } else if lower.contains("replan_required") || lower.contains("non-conver") {
            WorkflowStatus::NonConvergent
        } else {
            WorkflowStatus::Blocked
        };
        let mut next = current.clone();
        next.status = status;
        next.stop_reason = Some(message.clone());
        Ok(NextTransition::deterministic(
            next,
            "stage_failed",
            Some(message),
        ))
    }
}

struct NextTransition {
    run: WorkflowRun,
    edge: String,
    deterministic: bool,
    provider_run_id: Option<i64>,
    details: Option<String>,
}

impl NextTransition {
    fn deterministic(run: WorkflowRun, edge: &str, details: Option<String>) -> Self {
        Self {
            run,
            edge: edge.into(),
            deterministic: true,
            provider_run_id: None,
            details,
        }
    }

    fn provider(run: WorkflowRun, edge: &str, provider_run_id: i64) -> Self {
        Self {
            run,
            edge: edge.into(),
            deterministic: false,
            provider_run_id: Some(provider_run_id),
            details: None,
        }
    }

    fn semantic(run: WorkflowRun, edge: &str) -> Self {
        Self {
            run,
            edge: edge.into(),
            deterministic: false,
            provider_run_id: None,
            details: None,
        }
    }
}

/// Production adapter. It composes existing one-shot APIs; none of those APIs
/// gains hidden continuation behavior.
pub struct AppWorkflowActions<'a> {
    app: &'a crate::app::OrcApp,
}

impl<'a> AppWorkflowActions<'a> {
    pub const fn new(app: &'a crate::app::OrcApp) -> Self {
        Self { app }
    }

    fn recovered_run(
        &self,
        workflow: &WorkflowRun,
        purpose: &str,
    ) -> Result<Option<crate::storage::AgentRun>> {
        let Some(run_id) = self.app.database().completed_workflow_provider_run(
            workflow.id,
            workflow.stage.as_str(),
            workflow.version,
            purpose,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(
            self.app
                .database()
                .get_agent_run(run_id)?
                .context("persisted workflow provider run disappeared")?,
        ))
    }

    fn require_completed(run: &crate::storage::AgentRun, stage: &str) -> Result<()> {
        if run.status != "completed" {
            anyhow::bail!(
                "persisted {stage} run {} ended '{}': {}",
                run.id,
                run.status,
                run.error
                    .as_deref()
                    .or(run.output.as_deref())
                    .unwrap_or("no diagnostics")
            )
        }
        Ok(())
    }

    fn controller_planning_request(
        &self,
        workflow: &WorkflowRun,
    ) -> Result<crate::protocol::PlanningRequest> {
        let mut request = self.app.planning_request()?;
        request.objective = workflow.objective.clone();
        request.kind = "project_plan".into();
        Ok(request)
    }
}

impl WorkflowActions for AppWorkflowActions<'_> {
    fn discover(&self) -> Result<String> {
        Ok(crate::discovery::discover_and_persist(self.app.repo_path())?.fingerprint)
    }

    fn lead(&self, workflow: &WorkflowRun) -> Result<LeadOutcome> {
        let (run, response) = if let Some(resolution) = workflow.user_resolution.as_deref() {
            self.app.automated_lead(
                &format!(
                    "Continue workflow objective '{}' after the user decision: {}",
                    workflow.objective, resolution
                ),
                &crate::automated::ActionOverrides::default(),
            )?
        } else {
            self.app.new_project_intake(
                &workflow.objective,
                &crate::automated::ActionOverrides::default(),
            )?
        };
        let decision = response.decision.context("Lead returned no decision")?;
        let decision_id = self
            .app
            .lead_decisions()?
            .into_iter()
            .find(|item| item.run_id == Some(run))
            .map(|item| item.id)
            .context("Lead decision was not persisted")?;
        Ok(LeadOutcome {
            decision_id,
            provider_run_id: run,
            kind: decision.kind,
        })
    }

    fn apply_direct(&self) -> Result<()> {
        self.app
            .apply_pending_lead_decision()?
            .context("no actionable DIRECT_TASKS decision")?;
        Ok(())
    }

    fn controller_plan(
        &self,
        workflow: &WorkflowRun,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanOutcome>> {
        let request = self.controller_planning_request(workflow)?;
        let result = self
            .app
            .propose_controller_plan(&request, runtime)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if result.plan.objective != workflow.objective {
            anyhow::bail!("Controller Plan objective does not match workflow objective")
        }
        let proposal = self
            .app
            .propose_controller_plan_persistence_for_workflow(workflow.id, &result)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let authorization = self.app.authorize_controller_plan_persistence(&proposal);
        match self
            .app
            .execute_authorized_controller_plan_persistence(&proposal, Some(authorization))
        {
            crate::controller_plan_persistence::ControllerPlanPersistenceResult::Persisted {
                plan_id,
                ..
            } => Ok(Some(ControllerPlanOutcome { plan_id })),
            result => anyhow::bail!("Controller Plan persistence failed: {result:?}"),
        }
    }

    fn controller_plan_revision(
        &self,
        workflow: &WorkflowRun,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanOutcome>> {
        let plan_id = workflow
            .plan_id
            .context("workflow has no Controller Plan to revise")?;
        let result = self
            .app
            .revise_controller_plan(plan_id, runtime)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let proposal = self
            .app
            .propose_controller_plan_revision_persistence_for_workflow(workflow.id, &result)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let authorization = self
            .app
            .authorize_controller_plan_revision_persistence(&proposal);
        match self.app.execute_authorized_controller_plan_revision_persistence(
            &proposal,
            Some(authorization),
        ) {
            crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceResult::Persisted {
                plan_id,
                ..
            } => Ok(Some(ControllerPlanOutcome { plan_id })),
            result => anyhow::bail!("Controller Plan revision persistence failed: {result:?}"),
        }
    }

    fn controller_plan_review(
        &self,
        workflow: &WorkflowRun,
        plan_id: i64,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanReviewOutcome>> {
        let result = self
            .app
            .review_controller_plan(plan_id, workflow.user_resolution.as_deref(), runtime)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let proposal = self
            .app
            .propose_controller_plan_review_persistence_for_workflow(
                workflow.id,
                plan_id,
                workflow.user_resolution.as_deref(),
                &result,
            )
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let authorization = self
            .app
            .authorize_controller_plan_review_persistence(&proposal);
        match self.app.execute_authorized_controller_plan_review_persistence(
            &proposal,
            Some(authorization),
        ) {
            crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceResult::Persisted {
                decision,
                ..
            } => Ok(Some(ControllerPlanReviewOutcome {
                decision: match decision {
                    crate::controller_plan_review::ControllerPlanReviewDecision::Approve =>
                        ControllerPlanReviewDecision::Approve,
                    crate::controller_plan_review::ControllerPlanReviewDecision::RevisePlan =>
                        ControllerPlanReviewDecision::RevisePlan,
                    crate::controller_plan_review::ControllerPlanReviewDecision::OperatorDecisionRequired =>
                        ControllerPlanReviewDecision::UserDecisionRequired,
                },
            })),
            result => anyhow::bail!("Controller Plan review persistence failed: {result:?}"),
        }
    }

    fn recover_controller_plan(
        &self,
        workflow: &WorkflowRun,
    ) -> Result<Option<ControllerPlanOutcome>> {
        let parent_plan_id = match workflow.stage {
            WorkflowStage::Planner => None,
            WorkflowStage::PlannerRevision => workflow.plan_id,
            _ => return Ok(None),
        };
        let plan = self.app.database().controller_plan_for_workflow(
            workflow.id,
            workflow.project_id,
            parent_plan_id,
        )?;
        Ok(plan.map(|plan| ControllerPlanOutcome { plan_id: plan.id }))
    }

    fn recover_controller_plan_review(
        &self,
        workflow: &WorkflowRun,
    ) -> Result<Option<ControllerPlanReviewOutcome>> {
        // A resolution means the prior operator-decision review has already
        // been consumed by the workflow edge; the resumed review must infer
        // once with that resolution as context.
        if workflow.user_resolution.is_some() {
            return Ok(None);
        }
        let Some(plan_id) = workflow.plan_id else {
            return Ok(None);
        };
        let review = self.app.database().controller_plan_review_for_workflow(
            workflow.id,
            workflow.project_id,
            plan_id,
        )?;
        Ok(review.map(|decision| ControllerPlanReviewOutcome {
            decision: match decision {
                crate::storage::db::PlanReviewDecision::Approve => {
                    ControllerPlanReviewDecision::Approve
                }
                crate::storage::db::PlanReviewDecision::RevisePlan => {
                    ControllerPlanReviewDecision::RevisePlan
                }
                crate::storage::db::PlanReviewDecision::UserDecisionRequired => {
                    ControllerPlanReviewDecision::UserDecisionRequired
                }
            },
        }))
    }

    fn plan(&self) -> Result<PlanOutcome> {
        let outcome = self
            .app
            .run_pending_plan(&crate::automated::ActionOverrides::default())?;
        Ok(PlanOutcome {
            plan_id: outcome.plan_id,
            provider_run_id: outcome.planner_run_id,
        })
    }

    fn revise_plan(&self) -> Result<PlanOutcome> {
        let outcome = self
            .app
            .run_pending_plan_revision(&crate::automated::ActionOverrides::default())?;
        Ok(PlanOutcome {
            plan_id: outcome.plan_id,
            provider_run_id: outcome.planner_run_id,
        })
    }

    fn review_plan(&self, workflow: &WorkflowRun, plan_id: i64) -> Result<PlanReviewOutcome> {
        let outcome = self.app.review_plan_with_backend_and_resolution(
            plan_id,
            &crate::automated::ActionOverrides::default(),
            &crate::automated::WorkerActionBackend::new(self.app.repo_path()),
            workflow.user_resolution.as_deref(),
        )?;
        Ok(PlanReviewOutcome {
            provider_run_id: outcome
                .lead_run_id
                .context("legacy Plan review has no Lead run provenance")?,
            decision: outcome.decision.as_lead(),
        })
    }

    fn apply_plan(&self) -> Result<()> {
        self.app.apply_approved_plan()?;
        Ok(())
    }

    fn dispatch(&self, task_id: &str) -> Result<ProviderOutcome> {
        let outcome = self.app.dispatch(task_id, None)?;
        Ok(ProviderOutcome {
            provider_run_id: outcome.run_id,
        })
    }

    fn review(&self, task_id: &str) -> Result<ReviewOutcome> {
        let (run, outcome) = self
            .app
            .automated_review(task_id, &crate::automated::ActionOverrides::default())?;
        Ok(ReviewOutcome {
            provider_run_id: run,
            verdict: outcome.verdict,
            feedback: outcome.revision_feedback,
        })
    }

    fn revise_task(&self, task_id: &str, feedback: &str) -> Result<ProviderOutcome> {
        let implementation_agent = self
            .app
            .review(task_id)?
            .run
            .context("task has no implementation run")?
            .agent;
        self.app
            .revise_constrained(task_id, feedback, &implementation_agent)?;
        let run = self
            .app
            .review(task_id)?
            .run
            .context("revision run was not persisted")?;
        Ok(ProviderOutcome {
            provider_run_id: run.id,
        })
    }

    fn accept(&self, task_id: &str) -> Result<()> {
        self.app.accept(task_id)
    }

    fn recover_lead(&self, workflow: &WorkflowRun) -> Result<Option<LeadOutcome>> {
        let Some(run) = self.recovered_run(workflow, "lead")? else {
            return Ok(None);
        };
        Self::require_completed(&run, "Lead")?;
        let decision = self
            .app
            .lead_decisions()?
            .into_iter()
            .find(|decision| decision.run_id == Some(run.id))
            .context("completed workflow Lead run has no persisted decision")?;
        Ok(Some(LeadOutcome {
            decision_id: decision.id,
            provider_run_id: run.id,
            kind: decision.kind,
        }))
    }

    fn recover_plan(&self, workflow: &WorkflowRun) -> Result<Option<PlanOutcome>> {
        let Some(run) = self.recovered_run(workflow, "plan")? else {
            return Ok(None);
        };
        Self::require_completed(&run, "Planner")?;
        let mut recovered_plan = None;
        for entry in self
            .app
            .database()
            .list_plan_history(workflow.project_id)?
            .into_iter()
            .rev()
        {
            if let Some(plan) = self.app.database().get_plan(entry.plan_id)?
                && plan.provenance.origin == crate::storage::db::PlanOrigin::LegacyPlanner
                && plan.provenance.source_planner_run_id == Some(run.id)
            {
                recovered_plan = Some(plan);
                break;
            }
        }
        let plan =
            recovered_plan.context("completed workflow Planner run has no persisted plan")?;
        Ok(Some(PlanOutcome {
            plan_id: plan.id,
            provider_run_id: run.id,
        }))
    }

    fn recover_plan_review(&self, workflow: &WorkflowRun) -> Result<Option<PlanReviewOutcome>> {
        let Some(run) = self.recovered_run(workflow, "lead")? else {
            return Ok(None);
        };
        Self::require_completed(&run, "plan review")?;
        let review = self
            .app
            .database()
            .list_plan_reviews(workflow.project_id)?
            .into_iter()
            .rev()
            .find(|review| {
                review.lead_run_id == Some(run.id) && Some(review.plan_id) == workflow.plan_id
            })
            .context("completed workflow plan-review run has no persisted review")?;
        Ok(Some(PlanReviewOutcome {
            provider_run_id: run.id,
            decision: review.decision.as_lead(),
        }))
    }

    fn recover_dispatch(&self, workflow: &WorkflowRun) -> Result<Option<ProviderOutcome>> {
        let Some(run) = self.recovered_run(workflow, "implementation")? else {
            return Ok(None);
        };
        Self::require_completed(&run, "Worker")?;
        Ok(Some(ProviderOutcome {
            provider_run_id: run.id,
        }))
    }

    fn recover_review(&self, workflow: &WorkflowRun) -> Result<Option<ReviewOutcome>> {
        let Some(run) = self.recovered_run(workflow, "review")? else {
            return Ok(None);
        };
        Self::require_completed(&run, "Review")?;
        let result: crate::automated::ReviewResult = serde_json::from_str(
            run.output
                .as_deref()
                .context("completed workflow Review run has no output")?,
        )
        .context("persisted workflow Review output is malformed")?;
        Ok(Some(ReviewOutcome {
            provider_run_id: run.id,
            verdict: result.verdict,
            feedback: result.revision_feedback,
        }))
    }

    fn recover_revision(&self, workflow: &WorkflowRun) -> Result<Option<ProviderOutcome>> {
        let Some(run) = self.recovered_run(workflow, "revision")? else {
            return Ok(None);
        };
        Self::require_completed(&run, "revision Worker")?;
        Ok(Some(ProviderOutcome {
            provider_run_id: run.id,
        }))
    }
}
