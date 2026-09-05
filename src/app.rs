use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::agent;
use crate::protocol::{PlanResponse, PlanningProjectState, ProjectReport};
use crate::queue::QueueReport;
use crate::registry::{self, AgentDefinition};
use crate::review::{DispatchSummary, PriorReview, ReviewSummary, build_review};
use crate::storage::db::ApprovalRequest;
use crate::storage::{AgentRun, Database};
use crate::task::{CreateTaskInput, Task, TaskScopeMode};

#[derive(Debug, serde::Serialize)]
pub struct ManualRunContext {
    pub run: AgentRun,
    pub task: Task,
    pub task_packet: String,
}

#[derive(Debug, Clone, Copy)]
pub struct PersistedPlanRun {
    pub plan_id: i64,
    pub lead_decision_id: i64,
    pub planner_run_id: i64,
    pub task_count: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowState {
    pub position: String,
    pub lead_decisions: Vec<crate::lead::PersistedLeadDecision>,
    pub plans: Vec<crate::storage::db::PlanHistoryEntry>,
    pub plan_reviews: Vec<crate::storage::db::PlanReview>,
    pub user_decisions: Vec<crate::lead::PersistedLeadDecision>,
    pub tasks: Vec<Task>,
    pub runs: Vec<AgentRun>,
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
    pub(crate) fn database(&self) -> &Database {
        &self.db
    }

    pub(crate) fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Return the canonical memory facade for this application's project.
    /// Memory persistence remains explicit and is never invoked by a model.
    pub fn memories(&self) -> Result<crate::memory::MemoryService<'_>> {
        let project_id = self.lead().project_id()?;
        Ok(crate::memory::MemoryService::new(&self.db, project_id))
    }

    /// Build the bounded, read-only Controller memory projection through the
    /// canonical application memory service.
    pub fn controller_memory_context(
        &self,
    ) -> Result<crate::controller_memory::ControllerMemoryContext> {
        let memories = self.memories()?;
        Ok(crate::controller_memory::ControllerMemoryContext::from_memory_service(&memories)?)
    }

    /// Judge one explicitly supplied memory candidate through the bounded,
    /// read-only Controller capture seam. A proposed intent remains separate
    /// from the M06-009 proposal, authorization, and execution boundaries.
    pub fn judge_controller_memory_capture(
        &self,
        request: &crate::controller_memory_capture::ControllerMemoryCaptureRequest,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> std::result::Result<
        crate::controller_memory_capture::ControllerMemoryCaptureResult,
        crate::controller_memory_capture::ControllerMemoryCaptureError,
    > {
        request.validate()?;
        let memory = self.controller_memory_context().map_err(|error| {
            crate::controller_memory_capture::ControllerMemoryCaptureError::MemoryContext(
                error.to_string(),
            )
        })?;
        let input = crate::controller_memory_capture::ControllerMemoryCaptureInput::from_request(
            request, memory,
        );
        crate::controller_memory_capture::ControllerMemoryCaptureBuilder::new()
            .capture_with_memory(&input, runtime)
    }

    /// Perform at most one supervised Controller memory-capture step.
    ///
    /// This composes the existing M06-010 judgment, M06-009 proposal,
    /// M07-006 grant inspection, and M06-009 execution boundaries exactly
    /// once. It never retries, invokes another inference, or mutates memory
    /// outside the canonical authorized mutation path.
    pub fn capture_controller_memory_once(
        &self,
        request: &crate::controller_memory_capture::ControllerMemoryCaptureRequest,
        grant: &crate::controller_memory_capture_grant::ControllerMemoryCaptureGrant,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> crate::controller_memory_capture::ControllerMemoryCaptureStepResult {
        use crate::controller_memory_capture::{
            ControllerMemoryCaptureMutationSuccess, ControllerMemoryCaptureStepError,
            ControllerMemoryCaptureStepResult,
        };

        let capture = match self.judge_controller_memory_capture(request, runtime) {
            Ok(result) => result,
            Err(error) => {
                return ControllerMemoryCaptureStepResult::Rejected {
                    error: ControllerMemoryCaptureStepError::Capture(error),
                };
            }
        };
        let intent = match capture {
            crate::controller_memory_capture::ControllerMemoryCaptureResult::Ignore => {
                return ControllerMemoryCaptureStepResult::Ignored;
            }
            crate::controller_memory_capture::ControllerMemoryCaptureResult::ProposeMutation {
                intent,
            } => intent,
        };
        let proposal = match self.propose_controller_memory_mutation(intent) {
            Ok(proposal) => proposal,
            Err(error) => {
                return ControllerMemoryCaptureStepResult::Rejected {
                    error: ControllerMemoryCaptureStepError::Proposal(error),
                };
            }
        };
        let authorization = match self.inspect_controller_memory_capture_grant(grant, &proposal) {
            Ok(authorization) => authorization,
            Err(error) => {
                return ControllerMemoryCaptureStepResult::Rejected {
                    error: ControllerMemoryCaptureStepError::Grant(error),
                };
            }
        };
        let execution =
            self.execute_authorized_controller_memory_mutation(&proposal, Some(authorization));
        match ControllerMemoryCaptureMutationSuccess::from_execution(execution) {
            Ok(result) => ControllerMemoryCaptureStepResult::Mutated { result },
            Err(error) => ControllerMemoryCaptureStepResult::Rejected {
                error: ControllerMemoryCaptureStepError::Execution(error),
            },
        }
    }

    /// Resolve one explicit active memory target, then judge maintenance with
    /// bounded read-only data. Any proposed intent remains separate from the
    /// M06-009 proposal, authorization, and execution boundaries.
    pub fn judge_controller_memory_maintenance(
        &self,
        request: &crate::controller_memory_maintenance::ControllerMemoryMaintenanceRequest,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> std::result::Result<
        crate::controller_memory_maintenance::ControllerMemoryMaintenanceResult,
        crate::controller_memory_maintenance::ControllerMemoryMaintenanceError,
    > {
        request.validate()?;
        let project_id = self.lead().project_id().map_err(|error| {
            crate::controller_memory_maintenance::ControllerMemoryMaintenanceError::InvalidTarget(
                error.to_string(),
            )
        })?;
        if let crate::memory::MemoryId::Project {
            project_id: target_project,
            ..
        } = request.target()
            && *target_project != project_id
        {
            return Err(
                crate::controller_memory_maintenance::ControllerMemoryMaintenanceError::CrossProjectTarget,
            );
        }
        let memories = self.memories().map_err(|error| {
            crate::controller_memory_maintenance::ControllerMemoryMaintenanceError::MemoryService(
                error.to_string(),
            )
        })?;
        let target = memories
            .get(request.target())
            .map_err(|error| {
                crate::controller_memory_maintenance::ControllerMemoryMaintenanceError::MemoryService(
                    error.to_string(),
                )
            })?
            .ok_or(
                crate::controller_memory_maintenance::ControllerMemoryMaintenanceError::TargetNotFound,
            )?;
        if target.lifecycle != crate::memory::MemoryLifecycle::Active {
            return Err(
                crate::controller_memory_maintenance::ControllerMemoryMaintenanceError::TargetNotActive,
            );
        }
        let memory = self.controller_memory_context().map_err(|error| {
            crate::controller_memory_maintenance::ControllerMemoryMaintenanceError::MemoryContext(
                error.to_string(),
            )
        })?;
        let input =
            crate::controller_memory_maintenance::ControllerMemoryMaintenanceInput::from_resolved_target(
                request, target, memory,
            );
        crate::controller_memory_maintenance::ControllerMemoryMaintenanceBuilder::new()
            .maintain(&input, runtime)
    }

    /// Propose one bounded typed Controller memory mutation after deterministic
    /// current-project and canonical target validation. This does not mutate
    /// memory or mint authorization.
    pub fn propose_controller_memory_mutation(
        &self,
        intent: crate::controller_memory_mutation::ControllerMemoryMutationIntent,
    ) -> std::result::Result<
        crate::controller_memory_mutation::ControllerMemoryMutationProposal,
        crate::controller_memory_mutation::ControllerMemoryMutationError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_memory_mutation::ControllerMemoryMutationError::NoActiveProject
        })?;
        let memories = self.memories().map_err(|error| {
            crate::controller_memory_mutation::ControllerMemoryMutationError::MemoryService(
                error.to_string(),
            )
        })?;
        crate::controller_memory_mutation::ControllerMemoryMutationProposal::from_intent(
            project_id, intent, &memories,
        )
    }

    /// Create an opaque, finite memory-capture grant bound to this
    /// application's current project. The grant authorizes no capture
    /// judgment and carries no persistence or mutation capability.
    pub fn create_controller_memory_capture_grant(
        &self,
        action_budget: usize,
    ) -> std::result::Result<
        crate::controller_memory_capture_grant::ControllerMemoryCaptureGrant,
        crate::controller_memory_capture_grant::ControllerMemoryCaptureGrantError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_memory_capture_grant::ControllerMemoryCaptureGrantError::InvalidProject
        })?;
        crate::controller_memory_capture_grant::ControllerMemoryCaptureGrant::new(
            project_id,
            action_budget,
        )
    }

    /// Inspect one already validated M06-009 proposal against an explicit
    /// capture grant and mint the existing exact one-shot authorization.
    /// Capture judgment remains a separate caller-owned step.
    pub fn inspect_controller_memory_capture_grant(
        &self,
        grant: &crate::controller_memory_capture_grant::ControllerMemoryCaptureGrant,
        proposal: &crate::controller_memory_mutation::ControllerMemoryMutationProposal,
    ) -> std::result::Result<
        crate::controller_memory_mutation::ControllerMemoryMutationAuthorization,
        crate::controller_memory_capture_grant::ControllerMemoryCaptureGrantError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_memory_capture_grant::ControllerMemoryCaptureGrantError::InvalidProject
        })?;
        grant.inspect_and_authorize(project_id, proposal, |exact_proposal| {
            Ok(crate::controller_memory_mutation::authorize(exact_proposal))
        })
    }

    /// Mint opaque one-shot authorization for one exact validated memory
    /// mutation proposal.
    pub fn authorize_controller_memory_mutation(
        &self,
        proposal: &crate::controller_memory_mutation::ControllerMemoryMutationProposal,
    ) -> crate::controller_memory_mutation::ControllerMemoryMutationAuthorization {
        crate::controller_memory_mutation::authorize(proposal)
    }

    /// Consume authorization, revalidate fresh canonical memory state, and
    /// execute only through the existing M06-001 MemoryService operations.
    pub fn execute_authorized_controller_memory_mutation(
        &self,
        proposal: &crate::controller_memory_mutation::ControllerMemoryMutationProposal,
        authorization: Option<
            crate::controller_memory_mutation::ControllerMemoryMutationAuthorization,
        >,
    ) -> crate::controller_memory_mutation::ControllerMemoryMutationExecutionResult {
        let operation = proposal.operation();
        let Ok(project_id) = self.lead().project_id() else {
            return crate::controller_memory_mutation::ControllerMemoryMutationExecutionResult::FreshValidationRejected {
                operation,
                reason: crate::controller_memory_mutation::ControllerMemoryMutationRejection::StorageReadFailed,
            };
        };
        let Ok(memories) = self.memories() else {
            return crate::controller_memory_mutation::ControllerMemoryMutationExecutionResult::FreshValidationRejected {
                operation,
                reason: crate::controller_memory_mutation::ControllerMemoryMutationRejection::StorageReadFailed,
            };
        };
        crate::controller_memory_mutation::execute(proposal, authorization, project_id, &memories)
    }

    pub fn start_workflow(
        &self,
        objective: &str,
        policy: crate::workflow::WorkflowPolicy,
    ) -> Result<crate::workflow::WorkflowRun> {
        let project_id = self.lead().project_id()?;
        let actions = crate::workflow::AppWorkflowActions::new(self);
        crate::workflow::WorkflowEngine::new(&self.db, &actions)
            .start(project_id, objective, policy)
    }

    /// Start the persisted workflow with an explicitly supplied Controller
    /// runtime. This is the opt-in supervised path for Controller Plan
    /// generation/review/revision; legacy entry points remain available.
    pub fn start_workflow_with_controller_runtime(
        &self,
        objective: &str,
        policy: crate::workflow::WorkflowPolicy,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<crate::workflow::WorkflowRun> {
        let project_id = self.lead().project_id()?;
        let actions = crate::workflow::AppWorkflowActions::new(self);
        crate::workflow::WorkflowEngine::new(&self.db, &actions)
            .start_with_controller_runtime(project_id, objective, policy, runtime)
    }

    /// Alias emphasizing that the runtime is supplied by the caller rather
    /// than selected by workflow policy.
    pub fn start_workflow_with_runtime(
        &self,
        objective: &str,
        policy: crate::workflow::WorkflowPolicy,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<crate::workflow::WorkflowRun> {
        self.start_workflow_with_controller_runtime(objective, policy, runtime)
    }

    pub fn continue_workflow(&self, id: i64) -> Result<crate::workflow::WorkflowRun> {
        let actions = crate::workflow::AppWorkflowActions::new(self);
        crate::workflow::WorkflowEngine::new(&self.db, &actions).continue_run(id)
    }

    pub fn continue_workflow_with_controller_runtime(
        &self,
        id: i64,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<crate::workflow::WorkflowRun> {
        let actions = crate::workflow::AppWorkflowActions::new(self);
        crate::workflow::WorkflowEngine::new(&self.db, &actions)
            .continue_run_with_controller_runtime(id, runtime)
    }

    pub fn continue_workflow_with_runtime(
        &self,
        id: i64,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<crate::workflow::WorkflowRun> {
        self.continue_workflow_with_controller_runtime(id, runtime)
    }

    pub fn resolve_workflow(
        &self,
        id: i64,
        resolution: &str,
    ) -> Result<crate::workflow::WorkflowRun> {
        let actions = crate::workflow::AppWorkflowActions::new(self);
        crate::workflow::WorkflowEngine::new(&self.db, &actions).resolve_user_gate(id, resolution)
    }

    pub fn resolve_workflow_with_controller_runtime(
        &self,
        id: i64,
        resolution: &str,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<crate::workflow::WorkflowRun> {
        let actions = crate::workflow::AppWorkflowActions::new(self);
        crate::workflow::WorkflowEngine::new(&self.db, &actions)
            .resolve_user_gate_with_controller_runtime(id, resolution, runtime)
    }

    pub fn resolve_workflow_with_runtime(
        &self,
        id: i64,
        resolution: &str,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<crate::workflow::WorkflowRun> {
        self.resolve_workflow_with_controller_runtime(id, resolution, runtime)
    }

    pub fn cancel_workflow(
        &self,
        id: i64,
        reason: Option<&str>,
    ) -> Result<crate::workflow::WorkflowRun> {
        let actions = crate::workflow::AppWorkflowActions::new(self);
        crate::workflow::WorkflowEngine::new(&self.db, &actions).cancel(id, reason)
    }

    pub fn workflow_run(&self, id: i64) -> Result<Option<crate::workflow::WorkflowRun>> {
        Ok(self.db.get_workflow(id)?)
    }

    pub fn active_workflow(&self) -> Result<Option<crate::workflow::WorkflowRun>> {
        let project_id = self.lead().project_id()?;
        Ok(self.db.active_workflow(project_id)?)
    }

    pub fn workflow_transitions(
        &self,
        id: i64,
    ) -> Result<Vec<crate::workflow::WorkflowTransitionRecord>> {
        Ok(self.db.workflow_transitions(id)?)
    }

    pub fn lead(&self) -> crate::lead::LeadService<'_> {
        crate::lead::LeadService::new(&self.db, &self.repo_path)
    }

    pub fn pending_lead_decision(&self) -> Result<Option<crate::lead::PersistedLeadDecision>> {
        Ok(self.db.pending_lead_decision(self.lead().project_id()?)?)
    }

    pub fn lead_decisions(&self) -> Result<Vec<crate::lead::PersistedLeadDecision>> {
        Ok(self.db.list_lead_decisions(self.lead().project_id()?)?)
    }

    pub fn resolve_user_decision(
        &self,
        id: i64,
        resolution: &str,
    ) -> Result<crate::lead::PersistedLeadDecision> {
        Ok(self
            .db
            .resolve_user_decision(self.lead().project_id()?, id, resolution)?)
    }

    pub fn cancel_lead_decision(
        &self,
        id: i64,
        reason: Option<&str>,
    ) -> Result<crate::lead::PersistedLeadDecision, CancelError> {
        let project = self
            .lead()
            .project_id()
            .map_err(|error| CancelError::Invalid(error.to_string()))?;
        Ok(self.db.cancel_lead_decision(project, id, reason)?)
    }

    pub fn cancel_plan_review(&self, id: i64, reason: Option<&str>) -> Result<(), CancelError> {
        let project = self
            .lead()
            .project_id()
            .map_err(|error| CancelError::Invalid(error.to_string()))?;
        Ok(self.db.cancel_plan_review(project, id, reason)?)
    }

    pub fn consume_pending_lead_decision(
        &self,
    ) -> Result<Option<crate::lead::PersistedLeadDecision>> {
        Ok(self
            .db
            .consume_pending_lead_decision(self.lead().project_id()?)?)
    }
    pub fn apply_pending_lead_decision(
        &self,
    ) -> Result<Option<std::collections::BTreeMap<String, String>>> {
        Ok(self
            .db
            .apply_pending_lead_decision(self.lead().project_id()?)?)
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
        _context_limit: usize,
    ) -> Result<crate::lead::LeadResponse> {
        let backend = crate::automated::WorkerActionBackend::without_quota_refresh(&self.repo_path);
        let (_, response) = crate::automated::run_lead(
            &self.db,
            &self.repo_path,
            message,
            &crate::automated::ActionOverrides {
                agent_id: Some(config.agent_id.clone()),
                model: config.model.clone(),
                reasoning_effort: config.reasoning_effort,
            },
            &backend,
        )?;
        Ok(response)
    }
    pub fn invoke_persisted_lead(
        &self,
        message: &str,
        context_limit: usize,
    ) -> Result<crate::lead::LeadResponse> {
        let config = self.lead_provider_config()?.ok_or_else(|| anyhow::anyhow!("Lead is not configured. Configure one with `orc lead set <agent>` before running `orc ask`."))?;
        self.invoke_configured_lead(message, &config, context_limit)
    }
    pub fn invoke_persisted_lead_with_required_discovery(
        &self,
        message: &str,
        context_limit: usize,
    ) -> Result<crate::lead::LeadResponse> {
        crate::discovery::snapshot_for_provider(&self.repo_path)
            .context("Lead requires a current discovery snapshot")?;
        self.invoke_persisted_lead(message, context_limit)
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

    /// Propose a bounded plan through the read-only Controller boundary.
    ///
    /// This deliberately does not use the application database or any of the
    /// durable Planner/Lead APIs. The canonical request is projected into a
    /// bounded Controller request, inferred once, and returned without plan,
    /// task, workflow, or decision mutation.
    pub fn propose_controller_plan(
        &self,
        request: &crate::protocol::PlanningRequest,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<
        crate::controller_planning::ControllerPlanResult,
        crate::controller_planning::ControllerPlanningError,
    > {
        let memory = self.controller_memory_context().map_err(|error| {
            crate::controller_planning::ControllerPlanningError::MemoryContext(error.to_string())
        })?;
        let request =
            crate::controller_planning::ControllerPlanningInput::from_canonical(request, memory)?;
        crate::controller_planning::ControllerPlanningBuilder::new()
            .propose_with_memory(&request, runtime)
    }

    /// Classify normal workflow intake through the bounded read-only
    /// Controller capability. No workflow or task state is changed here.
    pub fn propose_controller_intake(
        &self,
        request: &crate::controller_intake::ControllerIntakeRequest,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<
        crate::controller_intake::ControllerIntakeResult,
        crate::controller_intake::ControllerIntakeError,
    > {
        let memory = self.controller_memory_context().map_err(|error| {
            crate::controller_intake::ControllerIntakeError::MemoryContext(error.to_string())
        })?;
        crate::controller_intake::ControllerIntakeBuilder::new()
            .classify_with_memory(request, memory, runtime)
    }

    /// Persist one exact Controller intake boundary for the supplied
    /// workflow. Routing and task application remain separate kernel edges.
    pub fn persist_controller_intake_for_workflow(
        &self,
        workflow_id: i64,
        operator_resolution: Option<&str>,
        result: &crate::controller_intake::ControllerIntakeResult,
    ) -> Result<()> {
        let project_id = self.lead().project_id()?;
        self.db.store_controller_intake_for_workflow(
            project_id,
            workflow_id,
            operator_resolution,
            result,
        )?;
        Ok(())
    }

    /// Review a current valid persisted Plan through the read-only Controller
    /// judgment boundary. This never persists the returned decision.
    pub fn review_controller_plan(
        &self,
        plan_id: i64,
        operator_resolution: Option<&str>,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> Result<
        crate::controller_plan_review::ControllerPlanReviewResult,
        crate::controller_plan_review::ControllerPlanReviewError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_plan_review::ControllerPlanReviewError::NoActiveProject
        })?;
        let plan = self
            .db
            .get_plan(plan_id)
            .map_err(crate::controller_plan_review::ControllerPlanReviewError::Storage)?
            .ok_or(
                crate::controller_plan_review::ControllerPlanReviewError::PlanNotFound(plan_id),
            )?;
        if !self
            .db
            .is_current_valid_controller_review_plan(project_id, &plan, operator_resolution)
            .map_err(crate::controller_plan_review::ControllerPlanReviewError::Storage)?
        {
            return Err(
                crate::controller_plan_review::ControllerPlanReviewError::PlanNotCurrent(plan_id),
            );
        }
        let state = self
            .db
            .planning_project_state()
            .map_err(crate::controller_plan_review::ControllerPlanReviewError::Storage)?;
        let project_name = self
            .db
            .get_project_name()
            .map_err(crate::controller_plan_review::ControllerPlanReviewError::Storage)?;
        let request = crate::controller_plan_review::ControllerPlanReviewRequest::from_persisted(
            &plan,
            project_name.as_deref(),
            &state,
            operator_resolution,
        )?;
        let memory = self.controller_memory_context().map_err(|error| {
            crate::controller_plan_review::ControllerPlanReviewError::MemoryContext(
                error.to_string(),
            )
        })?;
        let input = crate::controller_plan_review::ControllerPlanReviewInput::from_request(
            &request, memory,
        );
        crate::controller_plan_review::ControllerPlanReviewBuilder::new()
            .review_with_memory(&input, runtime)
    }

    /// Generate one read-only Controller revision for the current
    /// Controller-origin Plan and its latest actionable Controller review.
    /// Durable Plan/review mutation remains a later explicitly authorized
    /// boundary.
    pub fn revise_controller_plan(
        &self,
        plan_id: i64,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> std::result::Result<
        crate::controller_plan_revision::ControllerPlanRevisionResult,
        crate::controller_plan_revision::ControllerPlanRevisionError,
    > {
        use crate::controller_plan_revision::{
            ControllerPlanRevisionBuilder, ControllerPlanRevisionError as RevisionError,
            ControllerPlanRevisionRequest, ControllerPlanRevisionResult,
            persisted_revision_feedback,
        };
        let project_id = self
            .lead()
            .project_id()
            .map_err(|_| RevisionError::NoActiveProject)?;
        let plan = self
            .db
            .get_plan(plan_id)
            .map_err(RevisionError::Storage)?
            .ok_or(RevisionError::PlanNotFound(plan_id))?;
        if plan.provenance.origin != crate::storage::db::PlanOrigin::Controller {
            return Err(RevisionError::InvalidPlanOrigin(plan_id));
        }
        if !self
            .db
            .is_current_controller_revision_plan(project_id, &plan)
            .map_err(RevisionError::Storage)?
        {
            return Err(RevisionError::PlanNotCurrent(plan_id));
        }
        let latest_review = self
            .db
            .list_plan_reviews(project_id)
            .map_err(RevisionError::Storage)?
            .into_iter()
            .filter(|review| review.plan_id == plan_id)
            .max_by_key(|review| review.id)
            .ok_or(RevisionError::ReviewNotFound(plan_id))?;
        if latest_review.origin != crate::storage::db::PlanReviewOrigin::Controller
            || latest_review.decision != crate::storage::db::PlanReviewDecision::RevisePlan
            || latest_review.superseded_by_review_id.is_some()
        {
            return Err(RevisionError::ReviewNotActionable(latest_review.id));
        }
        let feedback = persisted_revision_feedback(&latest_review)?;
        let planning_request = self
            .planning_request()
            .map_err(|error| RevisionError::PlanningContext(error.to_string()))?;
        let request =
            ControllerPlanRevisionRequest::from_canonical(&plan, &feedback, &planning_request)?;
        let memory = self
            .controller_memory_context()
            .map_err(|error| RevisionError::MemoryContext(error.to_string()))?;
        let input = crate::controller_plan_revision::ControllerPlanRevisionInput::from_request(
            &request, memory,
        );
        let revised_plan =
            ControllerPlanRevisionBuilder::new().revise_with_memory(&input, runtime)?;
        ControllerPlanRevisionResult::from_generated(
            plan.id,
            plan.version,
            latest_review.id,
            revised_plan,
        )
    }

    /// Derive a read-only trusted-context proposal for persisting one
    /// Controller revision. The result carries no authority or storage
    /// capability.
    pub fn propose_controller_plan_revision_persistence(
        &self,
        result: &crate::controller_plan_revision::ControllerPlanRevisionResult,
    ) -> Result<
        crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposal,
        crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError,
    > {
        use crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposal as Proposal;
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidProject
        })?;
        let parent = self
            .db
            .get_plan(result.parent_plan_id)
            .map_err(|_| {
                crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidParent
            })?
            .ok_or(crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidParent)?;
        let review = self
            .db
            .list_plan_reviews(project_id)
            .map_err(|_| {
                crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview
            })?
            .into_iter()
            .find(|review| review.id == result.review_id)
            .ok_or(crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview)?;
        Proposal::from_controller_result(project_id, &parent, &review, result)
    }

    pub fn propose_controller_plan_revision_persistence_for_workflow(
        &self,
        workflow_id: i64,
        result: &crate::controller_plan_revision::ControllerPlanRevisionResult,
    ) -> Result<
        crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposal,
        crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError,
    > {
        use crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposal as Proposal;
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidProject
        })?;
        let parent = self
            .db
            .get_plan(result.parent_plan_id)
            .map_err(|_| {
                crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidParent
            })?
            .ok_or(crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidParent)?;
        let review = self
            .db
            .list_plan_reviews(project_id)
            .map_err(|_| {
                crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview
            })?
            .into_iter()
            .find(|review| review.id == result.review_id)
            .ok_or(crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview)?;
        Proposal::from_controller_result_for_workflow(
            project_id,
            Some(workflow_id),
            &parent,
            &review,
            result,
        )
    }

    /// Mint trusted authorization for one exact Controller revision
    /// persistence proposal.
    pub fn authorize_controller_plan_revision_persistence(
        &self,
        proposal: &crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposal,
    ) -> crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceAuthorization
    {
        crate::controller_plan_revision_persistence::authorization_for(proposal)
    }

    /// Consume one authorization and atomically persist the Controller
    /// revision after the canonical parent/review eligibility re-check.
    pub fn execute_authorized_controller_plan_revision_persistence(
        &self,
        proposal: &crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceProposal,
        authorization: Option<crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceAuthorization>,
    ) -> crate::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceResult {
        use crate::controller_plan_revision_persistence::{
            ControllerPlanRevisionPersistenceAuthorizationRejection as Rejection,
            ControllerPlanRevisionPersistenceFailure as Failure,
            ControllerPlanRevisionPersistenceResult as Result,
        };
        let Some(authorization) = authorization else {
            return Result::AuthorizationRejected {
                reason: Rejection::Missing,
            };
        };
        if !crate::controller_plan_revision_persistence::matches_authorization(
            proposal,
            &authorization,
        ) {
            return Result::AuthorizationRejected {
                reason: Rejection::NotAuthorizedForProposal,
            };
        }
        if proposal.validate().is_err() {
            return Result::PersistenceFailed {
                reason: Failure::InvalidProposal,
            };
        }
        let Ok(project_id) = self.lead().project_id() else {
            return Result::FreshValidationRejected;
        };
        let stored = match proposal.workflow_id() {
            Some(workflow_id) => self.db.store_controller_plan_revision_for_workflow(
                project_id,
                Some(workflow_id),
                proposal.parent_plan_id(),
                proposal.parent_plan_version(),
                proposal.parent_plan(),
                proposal.source_review_id(),
                proposal.source_review_details(),
                proposal.revised_plan(),
            ),
            None => self.db.store_controller_plan_revision(
                project_id,
                proposal.parent_plan_id(),
                proposal.parent_plan_version(),
                proposal.parent_plan(),
                proposal.source_review_id(),
                proposal.source_review_details(),
                proposal.revised_plan(),
            ),
        };
        let (plan_id, parent_id) = match stored {
            Ok(ids) => ids,
            Err(crate::storage::db::DbError::Scheduler(_)) => {
                return Result::FreshValidationRejected;
            }
            Err(_) => {
                return Result::PersistenceFailed {
                    reason: Failure::CanonicalStorage,
                };
            }
        };
        Result::Persisted {
            plan_id,
            version: proposal.parent_plan_version() + 1,
            status: crate::storage::db::PlanStatus::Proposed,
            origin: crate::storage::db::PlanOrigin::Controller,
            parent_plan_id: parent_id,
            source_review_id: proposal.source_review_id(),
        }
    }

    /// Derive a read-only trusted-context proposal for persisting one
    /// Controller review. The result itself never carries authority.
    pub fn propose_controller_plan_review_persistence(
        &self,
        plan_id: i64,
        result: &crate::controller_plan_review::ControllerPlanReviewResult,
    ) -> Result<
        crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposal,
        crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposalError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposalError::InvalidProject
        })?;
        let plan = self
            .db
            .get_plan(plan_id)
            .map_err(|_| {
                crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposalError::InvalidPlanIdentity
            })?
            .ok_or(crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposalError::InvalidPlanIdentity)?;
        crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposal::from_controller_result(
            project_id,
            &plan,
            result,
        )
    }

    pub fn propose_controller_plan_review_persistence_for_workflow(
        &self,
        workflow_id: i64,
        plan_id: i64,
        operator_resolution: Option<&str>,
        result: &crate::controller_plan_review::ControllerPlanReviewResult,
    ) -> Result<
        crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposal,
        crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposalError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposalError::InvalidProject
        })?;
        let plan = self
            .db
            .get_plan(plan_id)
            .map_err(|_| {
                crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposalError::InvalidPlanIdentity
            })?
            .ok_or(crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposalError::InvalidPlanIdentity)?;
        crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposal::from_controller_result_for_workflow(
            project_id,
            Some(workflow_id),
            operator_resolution,
            &plan,
            result,
        )
    }

    /// Mint trusted authorization for one exact Controller review proposal.
    pub fn authorize_controller_plan_review_persistence(
        &self,
        proposal: &crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposal,
    ) -> crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceAuthorization
    {
        crate::controller_plan_review_persistence::authorization_for(proposal)
    }

    /// Consume one authorization after a fresh current-Plan check and persist
    /// exactly one Controller-origin review through the canonical DB seam.
    pub fn execute_authorized_controller_plan_review_persistence(
        &self,
        proposal: &crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceProposal,
        authorization: Option<
            crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceAuthorization,
        >,
    ) -> crate::controller_plan_review_persistence::ControllerPlanReviewPersistenceResult {
        use crate::controller_plan_review_persistence::{
            ControllerPlanReviewPersistenceAuthorizationRejection as Rejection,
            ControllerPlanReviewPersistenceFailure as Failure,
            ControllerPlanReviewPersistenceResult as Result,
        };
        let Some(authorization) = authorization else {
            return Result::AuthorizationRejected {
                reason: Rejection::Missing,
            };
        };
        if !crate::controller_plan_review_persistence::matches_authorization(
            proposal,
            &authorization,
        ) {
            return Result::AuthorizationRejected {
                reason: Rejection::NotAuthorizedForProposal,
            };
        }
        if proposal.validate().is_err() {
            return Result::PersistenceFailed {
                reason: Failure::InvalidProposal,
            };
        }
        let Ok(project_id) = self.lead().project_id() else {
            return Result::PersistenceFailed {
                reason: Failure::InvalidProposal,
            };
        };
        let Ok(Some(plan)) = self.db.get_plan(proposal.plan_id()) else {
            return Result::FreshValidationRejected;
        };
        if plan.project_id != project_id
            || plan.version != proposal.plan_version()
            || plan.response != *proposal.plan()
            || plan.provenance.origin != crate::storage::db::PlanOrigin::Controller
            || !self
                .db
                .is_current_valid_controller_review_plan(
                    project_id,
                    &plan,
                    proposal.operator_resolution(),
                )
                .unwrap_or(false)
        {
            return Result::FreshValidationRejected;
        }
        let Ok(details) = proposal.persisted_details() else {
            return Result::PersistenceFailed {
                reason: Failure::InvalidProposal,
            };
        };
        let stored = match proposal.workflow_id() {
            Some(workflow_id) => self
                .db
                .store_controller_plan_review_for_workflow_with_resolution(
                    project_id,
                    workflow_id,
                    proposal.operator_resolution(),
                    proposal.plan_id(),
                    proposal.plan_version(),
                    proposal.plan(),
                    crate::controller_plan_review_persistence::database_decision(proposal),
                    &details,
                ),
            None => self.db.store_controller_plan_review(
                project_id,
                proposal.plan_id(),
                proposal.plan_version(),
                proposal.plan(),
                crate::controller_plan_review_persistence::database_decision(proposal),
                &details,
            ),
        };
        let Ok(review_id) = stored else {
            return Result::PersistenceFailed {
                reason: Failure::CanonicalStorage,
            };
        };
        let plan_status = match proposal.decision() {
            crate::controller_plan_review::ControllerPlanReviewDecision::Approve => {
                crate::storage::db::PlanStatus::Approved
            }
            crate::controller_plan_review::ControllerPlanReviewDecision::RevisePlan => {
                crate::storage::db::PlanStatus::RevisionRequested
            }
            crate::controller_plan_review::ControllerPlanReviewDecision::OperatorDecisionRequired => {
                crate::storage::db::PlanStatus::UnderReview
            }
        };
        Result::Persisted {
            review_id,
            plan_id: proposal.plan_id(),
            origin: crate::storage::db::PlanReviewOrigin::Controller,
            decision: proposal.decision(),
            plan_status,
        }
    }

    /// Mint trusted authorization for one exact validated Controller plan
    /// persistence proposal. This is a read-only application boundary.
    pub fn authorize_controller_plan_persistence(
        &self,
        proposal: &crate::controller_plan_persistence::ControllerPlanPersistenceProposal,
    ) -> crate::controller_plan_persistence::ControllerPlanPersistenceAuthorization {
        crate::controller_plan_persistence::authorization_for(proposal)
    }

    pub fn propose_controller_plan_persistence_for_workflow(
        &self,
        workflow_id: i64,
        result: &crate::controller_planning::ControllerPlanResult,
    ) -> Result<
        crate::controller_plan_persistence::ControllerPlanPersistenceProposal,
        crate::controller_plan_persistence::ControllerPlanPersistenceProposalError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_plan_persistence::ControllerPlanPersistenceProposalError::InvalidProject
        })?;
        crate::controller_plan_persistence::ControllerPlanPersistenceProposal::from_controller_result_for_workflow(
            project_id,
            Some(workflow_id),
            result,
        )
    }

    /// Persist one explicitly authorized Controller-origin Proposed Plan.
    /// Validation is repeated immediately before the canonical storage call;
    /// the authorization is consumed regardless of the execution outcome.
    pub fn execute_authorized_controller_plan_persistence(
        &self,
        proposal: &crate::controller_plan_persistence::ControllerPlanPersistenceProposal,
        authorization: Option<
            crate::controller_plan_persistence::ControllerPlanPersistenceAuthorization,
        >,
    ) -> crate::controller_plan_persistence::ControllerPlanPersistenceResult {
        let Some(authorization) = authorization else {
            return crate::controller_plan_persistence::ControllerPlanPersistenceResult::AuthorizationRejected {
                reason: crate::controller_plan_persistence::ControllerPlanPersistenceAuthorizationRejection::Missing,
            };
        };
        if !crate::controller_plan_persistence::matches_authorization(proposal, &authorization) {
            return crate::controller_plan_persistence::ControllerPlanPersistenceResult::AuthorizationRejected {
                reason: crate::controller_plan_persistence::ControllerPlanPersistenceAuthorizationRejection::NotAuthorizedForProposal,
            };
        }
        if proposal.validate().is_err() {
            return crate::controller_plan_persistence::ControllerPlanPersistenceResult::FreshValidationRejected;
        }
        let stored = match proposal.workflow_id() {
            Some(workflow_id) => self.db.store_controller_plan_for_workflow(
                proposal.project_id(),
                workflow_id,
                proposal.plan(),
            ),
            None => self
                .db
                .store_controller_plan(proposal.project_id(), proposal.plan()),
        };
        match stored {
            Ok(plan_id) => {
                crate::controller_plan_persistence::ControllerPlanPersistenceResult::persisted(
                    plan_id,
                )
            }
            Err(_) => crate::controller_plan_persistence::ControllerPlanPersistenceResult::PersistenceFailed {
                reason: crate::controller_plan_persistence::ControllerPlanPersistenceFailure::CanonicalStorage,
            },
        }
    }

    /// Execute exactly one Planner run for the current actionable PLAN_REQUIRED decision.
    pub fn run_pending_plan(
        &self,
        overrides: &crate::automated::ActionOverrides,
    ) -> Result<PersistedPlanRun> {
        let backend = crate::automated::WorkerActionBackend::new(&self.repo_path);
        self.run_pending_plan_with_backend(overrides, &backend)
    }

    /// Testable production boundary for the operator-invoked Planner flow.
    pub fn run_pending_plan_with_backend(
        &self,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
    ) -> Result<PersistedPlanRun> {
        let project_id = self.lead().project_id()?;
        let decision = self
            .db
            .pending_lead_decision(project_id)?
            .ok_or_else(|| anyhow::anyhow!("no actionable pending Lead decision"))?;
        if decision.kind != crate::lead::LeadDecisionKind::PlanRequired || !decision.actionable {
            anyhow::bail!("pending Lead decision is not an actionable PLAN_REQUIRED decision")
        }
        let mut request = self.planning_request()?;
        request.objective = if decision.source_request.trim().is_empty() {
            decision.summary.clone()
        } else {
            decision.source_request.clone()
        };
        if request.objective.trim().is_empty() {
            anyhow::bail!("PLAN_REQUIRED Lead decision has no planning objective")
        }
        let (planner_run_id, response) =
            self.automated_plan_with_backend(&request, overrides, backend)?;
        let plan_id = self.db.store_plan_and_consume_decision(
            project_id,
            decision.id,
            planner_run_id,
            &response,
        )?;
        Ok(PersistedPlanRun {
            plan_id,
            lead_decision_id: decision.id,
            planner_run_id,
            task_count: response.tasks.len(),
        })
    }

    /// Run Planner once for an actionable Lead REVISE_PLAN decision. The
    /// resulting plan is persisted as a new version; it is never applied.
    pub fn run_pending_plan_revision_with_backend(
        &self,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
    ) -> Result<PersistedPlanRun> {
        let project_id = self.lead().project_id()?;
        let decision = self
            .db
            .pending_lead_decision(project_id)?
            .ok_or_else(|| anyhow::anyhow!("no actionable pending Lead revision decision"))?;
        if decision.kind != crate::lead::LeadDecisionKind::RevisePlan || !decision.actionable {
            anyhow::bail!("pending Lead decision is not an actionable REVISE_PLAN decision");
        }
        let review_plan = self
            .db
            .get_plan_review_for_decision(decision.id)?
            .ok_or_else(|| anyhow::anyhow!("REVISE_PLAN decision has no persisted plan review"))?;
        let parent = self
            .db
            .get_plan(review_plan.0)?
            .ok_or_else(|| anyhow::anyhow!("reviewed plan not found"))?;
        let mut request = self.planning_request()?;
        request.kind = "project_plan_revision".into();
        request.objective = format!("Revise the previous plan: {}", parent.response.objective);
        request.planning_constraints.push(format!(
            "Previous persisted Plan (read-only): {}",
            serde_json::to_string(&parent.response)?
        ));
        request.planning_constraints.push(format!(
            "Structured Lead revision feedback: {}",
            decision.details
        ));
        let (run_id, response) = self.automated_plan_with_backend(&request, overrides, backend)?;
        let (plan_id, _) =
            self.db
                .store_plan_revision(project_id, decision.id, run_id, &response)?;
        Ok(PersistedPlanRun {
            plan_id,
            lead_decision_id: decision.id,
            planner_run_id: run_id,
            task_count: response.tasks.len(),
        })
    }

    pub fn run_pending_plan_revision(
        &self,
        overrides: &crate::automated::ActionOverrides,
    ) -> Result<PersistedPlanRun> {
        let backend = crate::automated::WorkerActionBackend::new(&self.repo_path);
        self.run_pending_plan_revision_with_backend(overrides, &backend)
    }

    pub fn review_plan_with_backend(
        &self,
        plan_id: i64,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
    ) -> Result<crate::storage::db::PlanReview> {
        self.review_plan_with_backend_and_resolution(plan_id, overrides, backend, None)
    }

    pub fn review_plan_with_backend_and_resolution(
        &self,
        plan_id: i64,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
        user_resolution: Option<&str>,
    ) -> Result<crate::storage::db::PlanReview> {
        let plan = self
            .db
            .get_plan(plan_id)?
            .ok_or_else(|| anyhow::anyhow!("plan {plan_id} not found"))?;
        let project_id = self.lead().project_id()?;
        if !self.db.is_current_valid_plan(project_id, &plan)? {
            anyhow::bail!("plan {plan_id} is not the current valid plan");
        }
        let resolution = user_resolution
            .map(|value| format!(" Persisted user response to your prior question: {value}."))
            .unwrap_or_default();
        let prompt = format!(
            "Review exactly this current valid Planner plan and the supplied project context.{resolution} Return exactly one decision: APPROVE, REVISE_PLAN, or USER_DECISION_REQUIRED. Do not invoke another workflow stage, apply changes, or dispatch work. Plan: {}",
            serde_json::to_string(&plan)?
        );
        let (run_id, response) = self.automated_lead_with_backend(&prompt, overrides, backend)?;
        let decision = response
            .decision
            .ok_or_else(|| anyhow::anyhow!("Lead review returned no decision"))?;
        if !matches!(
            decision.kind,
            crate::lead::LeadDecisionKind::Approve
                | crate::lead::LeadDecisionKind::RevisePlan
                | crate::lead::LeadDecisionKind::UserDecisionRequired
        ) {
            anyhow::bail!("Lead review returned invalid decision {:?}", decision.kind);
        }
        let decision_id = self
            .lead_decisions()?
            .into_iter()
            .find(|d| d.run_id == Some(run_id))
            .map(|d| d.id)
            .ok_or_else(|| anyhow::anyhow!("Lead review decision was not persisted"))?;
        let review_id = self.db.record_plan_review(
            plan_id,
            run_id,
            decision_id,
            &decision.kind,
            &decision.details.to_string(),
        )?;
        Ok(crate::storage::db::PlanReview {
            id: review_id,
            plan_id,
            origin: crate::storage::db::PlanReviewOrigin::LegacyLead,
            lead_run_id: Some(run_id),
            lead_decision_id: Some(decision_id),
            decision: crate::storage::db::PlanReviewDecision::from_lead(decision.kind)
                .expect("validated legacy review decision must map"),
            details: decision.details.to_string(),
            created_at: String::new(),
            superseded_by_review_id: None,
        })
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
        let config = self.lead_provider_config()?.ok_or_else(|| {
            anyhow::anyhow!(
                "Lead is not configured. Configure one with `orc lead set <agent>` before running `orc lead run`."
            )
        })?;
        let mut configured = overrides.clone();
        configured.agent_id = Some(config.agent_id);
        if configured.model.is_none() {
            configured.model = config.model;
        }
        if configured.reasoning_effort.is_none() {
            configured.reasoning_effort = config.reasoning_effort;
        }
        let backend = crate::automated::WorkerActionBackend::new(&self.repo_path);
        self.automated_lead_with_backend(message, &configured, &backend)
    }

    /// Run the explicit new-project intake: capture the read-only repository
    /// snapshot, ask Lead for an initial decision, and leave that decision
    /// pending for the operator.
    pub fn new_project_intake(
        &self,
        objective: &str,
        overrides: &crate::automated::ActionOverrides,
    ) -> Result<(i64, crate::lead::LeadResponse)> {
        let backend = crate::automated::WorkerActionBackend::new(&self.repo_path);
        self.new_project_intake_with_backend(objective, overrides, &backend)
    }

    /// Testable production boundary for the new-project intake.
    pub fn new_project_intake_with_backend(
        &self,
        objective: &str,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
    ) -> Result<(i64, crate::lead::LeadResponse)> {
        if objective.trim().is_empty() {
            anyhow::bail!("new-project objective must not be empty");
        }
        let snapshot = crate::discovery::snapshot_for_provider(&self.repo_path)?;
        let request = serde_json::json!({
            "kind": "new_project_intake",
            "objective": objective,
            "discovery_snapshot": snapshot,
            "instruction": "Assess this new project read-only. Return one actionable Lead decision; do not apply or dispatch anything."
        });
        self.automated_lead_with_backend(&serde_json::to_string(&request)?, overrides, backend)
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
            || !crate::backend::provider_adapter(&agent.backend)
                .is_some_and(|adapter| adapter.supports_lead())
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
    pub fn add_agent_action(&self, id: &str, action: registry::AgentAction) -> Result<bool> {
        let agent = registry::get_agent(&self.db, id)?;
        Ok(self.db.set_agent_action_profile(
            id,
            action,
            agent.model.as_deref(),
            agent.reasoning_effort,
        )?)
    }
    pub fn remove_agent_action(&self, id: &str, action: registry::AgentAction) -> Result<bool> {
        let agent = registry::get_agent(&self.db, id)?;
        if !agent.supports_action(action) {
            return Ok(false);
        }
        if agent.actions.len() <= 1 {
            anyhow::bail!("cannot remove the final supported action from agent '{id}'")
        }
        if self.db.agent_action_profiles(id)?.is_empty() {
            for supported in &agent.actions {
                self.db.set_agent_action_profile(
                    id,
                    *supported,
                    agent.model.as_deref(),
                    agent.reasoning_effort,
                )?;
            }
        }
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
        Self::open_with_database(Database::open(db_path)?, repo_path)
    }

    pub fn open_with_registry(
        db_path: impl AsRef<Path>,
        repo_path: impl AsRef<Path>,
        registry_path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_with_database(
            Database::open_with_registry(db_path, registry_path)?,
            repo_path,
        )
    }

    /// Open an operator-facing Orc application against the authoritative
    /// global agent registry. Tests and embedders that require an isolated
    /// registry can continue to use `open` or `Database::open_with_registry`.
    pub fn open_global(db_path: impl AsRef<Path>, repo_path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_database(Database::open_global(db_path)?, repo_path)
    }

    fn open_with_database(mut db: Database, repo_path: impl AsRef<Path>) -> Result<Self> {
        let events = crate::events::EventHub::new();
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
        self.operations().tasks()
    }
    pub fn operations(&self) -> crate::operations::ProjectOperations<'_> {
        crate::operations::ProjectOperations::new(&self.db, &self.repo_path)
    }

    /// Read canonical abnormal-state facts and legal recovery operations.
    /// This seam performs no recovery, authorization or persistence mutation.
    pub fn inspect_recovery(
        &self,
        task_id: &str,
    ) -> std::result::Result<crate::recovery::RecoveryInspection, crate::recovery::RecoveryError>
    {
        crate::recovery::inspect_recovery(&self.operations(), task_id)
    }

    /// Produce one bounded, read-only recovery recommendation. This does not
    /// authorize or execute recovery; a trusted caller must make that decision
    /// separately at the M03 action boundary.
    pub fn recommend_recovery(
        &self,
        task_id: &str,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> std::result::Result<
        crate::recovery_controller::RecoveryRecommendationResult,
        crate::recovery_controller::RecoveryControllerError,
    > {
        let memory = self.controller_memory_context().map_err(|error| {
            crate::recovery_controller::RecoveryControllerError::MemoryContext(error.to_string())
        })?;
        crate::recovery_controller::RecoveryRecommendationBuilder::new().recommend_with_memory(
            &self.operations(),
            task_id,
            memory,
            runtime,
        )
    }

    /// Build one read-only Controller recommendation and expose only its
    /// bounded typed action proposal. This seam does not authorize or execute
    /// the proposal; trusted callers must explicitly use the M03-002
    /// authorization and execution boundary afterward.
    pub fn propose_controller_action(
        &self,
        task_id: &str,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
    ) -> std::result::Result<
        crate::controller_actions::ControllerActionProposal,
        crate::controller::ControllerError,
    > {
        let memory = self.controller_memory_context().map_err(|error| {
            crate::controller::ControllerError::MemoryContext(error.to_string())
        })?;
        let recommendation = crate::controller::ControllerStateBuilder::new()
            .recommend_with_memory(&self.operations(), task_id, memory, runtime)?;
        Ok(crate::controller_actions::propose_controller_action(
            &recommendation,
        ))
    }

    /// Create an opaque, finite continuation grant bound to this project's
    /// current identity.  Grant construction is trusted application code;
    /// the grant contains no model, workflow, provider, or execution handle.
    pub fn create_controller_continuation_grant(
        &self,
        allowed_actions: crate::controller_continuation::ControllerContinuationAllowedActions,
        action_budget: usize,
    ) -> std::result::Result<
        crate::controller_continuation::ControllerContinuationGrant,
        crate::controller_continuation::ControllerContinuationGrantError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_continuation::ControllerContinuationGrantError::InvalidProject
        })?;
        crate::controller_continuation::ControllerContinuationGrant::new(
            project_id,
            allowed_actions,
            action_budget,
        )
    }

    /// Inspect one exact existing Controller action against an opaque grant
    /// and current canonical legality.  A successful call mints only the
    /// existing one-shot M03 authorization; it never executes the action.
    pub fn inspect_controller_continuation_grant(
        &self,
        grant: &crate::controller_continuation::ControllerContinuationGrant,
        intent: &crate::controller_actions::ControllerActionIntent,
    ) -> std::result::Result<
        crate::controller_actions::ControllerActionAuthorization,
        crate::controller_continuation::ControllerContinuationGrantError,
    > {
        let project_id = self.lead().project_id().map_err(|_| {
            crate::controller_continuation::ControllerContinuationGrantError::InvalidProject
        })?;
        grant.inspect_and_authorize(project_id, intent, &self.operations(), |exact_intent| {
            self.authorize_controller_action(exact_intent)
        })
    }

    /// Perform at most one supervised Controller continuation step.
    ///
    /// This composes recommendation, exact proposal/task/expected-action
    /// validation, M07-001 grant inspection, and the existing M03 execution
    /// boundary once. It never retries, selects another task, continues a
    /// workflow, or accepts a task automatically.
    pub fn continue_controller_action_once(
        &self,
        task_id: &str,
        expected_action: crate::controller_actions::ControllerActionKind,
        grant: &crate::controller_continuation::ControllerContinuationGrant,
        runtime: &mut dyn crate::local_runtime::LocalInferenceRuntime,
        context: crate::controller_actions::ControllerActionExecutionContext<'_>,
    ) -> std::result::Result<
        crate::controller_continuation::ControllerContinuationStepResult,
        crate::controller::ControllerError,
    > {
        let requested_task_id =
            crate::controller_continuation::ControllerContinuationStepResult::task_id(task_id);
        if !matches!(
            expected_action,
            crate::controller_actions::ControllerActionKind::Dispatch
                | crate::controller_actions::ControllerActionKind::SemanticReview
                | crate::controller_actions::ControllerActionKind::Revise
        ) {
            return Ok(
                crate::controller_continuation::ControllerContinuationStepResult::ExpectedActionRejected {
                    task_id: requested_task_id,
                    expected_action,
                },
            );
        }

        let proposal = self.propose_controller_action(task_id, runtime)?;
        let intent = match proposal {
            crate::controller_actions::ControllerActionProposal::Proposed { intent } => intent,
            proposal => {
                return Ok(
                    crate::controller_continuation::ControllerContinuationStepResult::NoActionableProposal {
                        task_id: requested_task_id,
                        proposal,
                    },
                )
            }
        };

        if intent.task_id() != task_id {
            return Ok(
                crate::controller_continuation::ControllerContinuationStepResult::ProposalTaskMismatch {
                    task_id: requested_task_id,
                    intent,
                },
            );
        }

        let proposed_action = intent.action_kind();
        if proposed_action != expected_action {
            return Ok(
                crate::controller_continuation::ControllerContinuationStepResult::ExpectedActionMismatch {
                    task_id: requested_task_id,
                    expected_action,
                    proposed_action,
                    intent,
                },
            );
        }

        let authorization = match self.inspect_controller_continuation_grant(grant, &intent) {
            Ok(authorization) => authorization,
            Err(error) => {
                return Ok(
                    crate::controller_continuation::ControllerContinuationStepResult::GrantRejected {
                        task_id: requested_task_id,
                        intent,
                        reason: crate::controller_continuation::ControllerContinuationStepGrantRejection::from_grant_error(&error),
                    },
                )
            }
        };

        let execution =
            self.execute_authorized_controller_action(&intent, Some(authorization), context);
        if matches!(
            execution,
            crate::controller_actions::ControllerActionExecutionResult::Executed { .. }
        ) {
            Ok(
                crate::controller_continuation::ControllerContinuationStepResult::Executed {
                    task_id: requested_task_id,
                    intent,
                    result: execution,
                },
            )
        } else {
            Ok(
                crate::controller_continuation::ControllerContinuationStepResult::ExecutionRejected {
                    task_id: requested_task_id,
                    intent,
                    result: execution,
                },
            )
        }
    }

    pub fn task_operations(
        &self,
        id: &str,
    ) -> Result<Option<crate::operations::TaskOperationsDetail>> {
        self.operations().task_detail(id)
    }
    pub fn task_operation_summaries(
        &self,
    ) -> Result<Vec<crate::operations::TaskOperationsSummary>> {
        self.operations().task_summaries()
    }
    pub fn economy_summary(&self) -> Result<crate::operations::ProjectEconomySummary> {
        self.operations().economy_summary()
    }
    pub fn provider_invocation_summaries(
        &self,
    ) -> Result<Vec<crate::operations::EconomyResolutionSummary>> {
        self.operations().provider_invocation_summaries()
    }
    pub fn provider_invocation_summary(
        &self,
        id: i64,
    ) -> Result<Option<crate::operations::EconomyResolutionSummary>> {
        self.operations().provider_invocation_summary(id)
    }
    pub fn dashboard(&self, activity_limit: usize) -> Result<crate::read_model::Dashboard> {
        crate::read_model::dashboard(&self.db, &self.repo_path, activity_limit)
    }
    pub fn task_details(
        &self,
        id: &str,
        activity_limit: usize,
    ) -> Result<Option<crate::read_model::TaskDetails>> {
        crate::read_model::task_details(&self.db, &self.repo_path, id, activity_limit)
    }
    pub fn run_details(
        &self,
        id: i64,
        activity_limit: usize,
    ) -> Result<Option<crate::read_model::RunDetails>> {
        crate::read_model::run_details(&self.db, id, activity_limit)
    }
    pub fn worker_log(&self, id: i64) -> Result<Vec<crate::storage::db::LifecycleEvent>> {
        let run = self.db.get_agent_run(id)?.context("run not found")?;
        let project = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        if run.project_id != project {
            anyhow::bail!("run does not belong to the active project")
        }
        Ok(self.db.list_worker_output(id)?)
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
            definition_of_done: vec![
                "Every task has a unique id and valid dependencies.".into(),
                "Every task supplies a non-authoritative low, medium, or high execution-effort hint with a concise semantic reason, accurate bounded risk factors, and deterministic safeguards expressed through precise acceptance, test, and validation requirements.".into(),
            ],
            response_schema: crate::protocol::PlanResponseSchema::v1(),
            role_boundaries: report.role_boundaries.clone(),
            planning_constraints: report.planning_constraints.clone(),
            approval_requirements: report.approval_requirements.clone(),
            current_state: Some(self.planning_state()?),
            full_report: Some(report),
            discovery_snapshot: crate::discovery::snapshot_for_provider(&self.repo_path).ok(),
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
        self.operations().project_queue()
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
    pub fn review_run(&self, run_id: i64) -> Result<crate::review::ReviewSummary> {
        crate::review::build_review_for_run(&self.db, run_id, &self.repo_path)
    }
    pub fn agents(&self) -> Result<Vec<AgentDefinition>> {
        Ok(self.db.list_agents()?)
    }

    pub fn global_agents(&self) -> Result<Vec<registry::Agent>> {
        Ok(self.db.list_global_agents()?)
    }

    pub fn project_agents(&self) -> Result<Vec<registry::Agent>> {
        let project_id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        Ok(self.db.list_project_agents(project_id)?)
    }

    pub fn reference_global_agent(&self, agent_id: &str) -> Result<bool> {
        let project_id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        Ok(self.db.reference_global_agent(project_id, agent_id)?)
    }

    pub fn remove_global_agent_reference(&self, agent_id: &str) -> Result<bool> {
        let project_id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        Ok(self
            .db
            .remove_global_agent_reference(project_id, agent_id)?)
    }

    pub fn busy_agents(&self) -> Result<std::collections::HashSet<String>> {
        Ok(self.db.list_busy_agents()?.into_iter().collect())
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
    pub fn workflow_state(&self) -> Result<WorkflowState> {
        let project = self.lead().project_id()?;
        let decisions = self.db.list_lead_decisions(project)?;
        let plans = self.db.list_plan_history(project)?;
        let reviews = self.db.list_plan_reviews(project)?;
        let tasks = self.db.list_tasks_for_project(project)?;
        let runs = self.db.list_agent_runs(project, usize::MAX)?;
        let position = if decisions.iter().any(|d| {
            d.status == "pending" && d.kind == crate::lead::LeadDecisionKind::UserDecisionRequired
        }) {
            "user_decision_required"
        } else if decisions
            .iter()
            .any(|d| d.status == "pending" && d.kind == crate::lead::LeadDecisionKind::PlanRequired)
        {
            "planner_required"
        } else if plans.last().is_some_and(|p| {
            p.status == crate::storage::db::PlanStatus::Proposed
                || p.status == crate::storage::db::PlanStatus::UnderReview
                || p.status == crate::storage::db::PlanStatus::RevisionRequested
        }) {
            "plan_review"
        } else if tasks
            .iter()
            .any(|t| t.status == crate::task::TaskStatus::RevisionRequired)
        {
            "task_revision_required"
        } else if tasks
            .iter()
            .any(|t| t.status == crate::task::TaskStatus::AcceptanceReady)
        {
            "task_acceptance_ready"
        } else if tasks
            .iter()
            .any(|t| t.status == crate::task::TaskStatus::Review)
        {
            "task_review"
        } else if tasks
            .iter()
            .any(|t| t.status == crate::task::TaskStatus::Active)
            || runs
                .iter()
                .any(|r| matches!(r.status.as_str(), "running" | "waiting_external"))
        {
            "task_execution"
        } else if tasks
            .iter()
            .any(|t| t.status == crate::task::TaskStatus::Blocked)
        {
            "blocked"
        } else if !tasks.is_empty() && tasks.iter().all(|t| t.status.is_terminal()) {
            "complete"
        } else if plans
            .last()
            .is_some_and(|p| p.status == crate::storage::db::PlanStatus::Applied)
        {
            "tasks_ready"
        } else {
            "lead_decision"
        };
        Ok(WorkflowState {
            position: position.into(),
            user_decisions: decisions
                .iter()
                .filter(|d| d.kind == crate::lead::LeadDecisionKind::UserDecisionRequired)
                .cloned()
                .collect(),
            lead_decisions: decisions,
            plans,
            plan_reviews: reviews,
            tasks,
            runs,
        })
    }
    pub fn workflow_history(&self) -> Result<Vec<crate::read_model::WorkflowHistoryEntry>> {
        crate::read_model::workflow_history(&self.db)
    }
    pub fn review(&self, task_id: &str) -> Result<ReviewSummary> {
        build_review(&self.db, task_id, &self.repo_path)
    }
    pub fn review_history(&self, task_id: &str) -> Result<Vec<PriorReview>> {
        Ok(self.review(task_id)?.automated_reviews)
    }

    /// Return the newest unconsumed REVISE review evidence for a canonical
    /// revision execution. Controller callers receive only the application
    /// fact they need; storage remains behind OrcApp.
    pub(crate) fn actionable_revision_feedback(&self, task_id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .actionable_revision_review(task_id)?
            .map(|(_, feedback)| feedback))
    }

    pub fn review_for_run(&self, task_id: &str, run_id: i64) -> Result<ReviewSummary> {
        crate::review::build_review_for_task_run(&self.db, task_id, run_id, &self.repo_path)
    }
    /// Task review consumes fresh, task-specific validation evidence produced
    /// by Orc before producing a verdict (see `automated::run_review`).
    /// Review itself is semantic-only and does not execute validation.
    pub fn automated_review_with_backend(
        &self,
        task_id: &str,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
        validation_runner: &dyn crate::validation::ValidationRunner,
    ) -> Result<(i64, crate::automated::ReviewResult)> {
        crate::self_hosting::ensure_execution_ready(&self.repo_path)?;
        let summary = self.review(task_id)?;
        crate::automated::run_review(
            &self.db,
            &summary,
            overrides,
            backend,
            &self.repo_path,
            validation_runner,
        )
    }
    pub fn automated_review(
        &self,
        task_id: &str,
        overrides: &crate::automated::ActionOverrides,
    ) -> Result<(i64, crate::automated::ReviewResult)> {
        let backend = crate::automated::WorkerActionBackend::new(&self.repo_path);
        self.automated_review_with_backend(
            task_id,
            overrides,
            &backend,
            &crate::validation::SystemValidationRunner,
        )
    }
    pub fn automated_project_review_with_backend(
        &self,
        task_id: &str,
        overrides: &crate::automated::ActionOverrides,
        backend: &dyn crate::automated::ActionBackend,
    ) -> Result<(i64, crate::automated::ReviewResult)> {
        crate::self_hosting::ensure_execution_ready(&self.repo_path)?;
        let summary = self.review(task_id)?;
        crate::automated::run_project_review(&self.db, &summary, overrides, backend)
    }
    pub fn requeue(&self, task_id: &str) -> Result<()> {
        self.db.requeue_task(
            task_id,
            "Operator manually requeued task after recoverable run failure",
        )?;
        Ok(())
    }
    /// Deliberately acknowledges a persisted non-convergence gate. It does
    /// not change task status or execute a lifecycle stage.
    pub fn unblock_non_convergence(&self, task_id: &str) -> Result<()> {
        self.db
            .acknowledge_non_convergence_replan_required(task_id)?;
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
        self.dispatch_cancellable(
            task_id,
            agent_id,
            &crate::worker::CancellationControl::new(),
        )
    }
    pub fn dispatch_cancellable(
        &self,
        task_id: &str,
        agent_id: Option<&str>,
        cancellation: &crate::worker::CancellationControl,
    ) -> Result<DispatchSummary> {
        let result = agent::dispatch_selected_with_db_and_repo_cancellable(
            &self.db,
            &self.repo_path,
            task_id,
            agent_id,
            None,
            None,
            Some(cancellation),
        )?;
        Ok(result)
    }
    pub fn revise(&self, task_id: &str, feedback: &str, agent_id: &str) -> Result<()> {
        self.revise_with_agent_selection(task_id, feedback, agent_id, true)
    }

    /// Revise with the most recent implementation agent as a constrained
    /// scheduler preference. This is the no-override application path shared
    /// by presentation clients; semantic Review runs are never mistaken for
    /// implementation-agent selection.
    pub fn revise_with_previous_agent(&self, task_id: &str, feedback: &str) -> Result<()> {
        let agent = self
            .db
            .list_agent_runs_for_task(task_id)?
            .into_iter()
            .find(|run| {
                matches!(
                    run.execution_class.as_str(),
                    "coder" | "reviewer" | "architect" | "researcher" | "general"
                )
            })
            .map(|run| run.agent)
            .context("task has no prior implementation agent run for revision")?;
        self.revise_constrained(task_id, feedback, &agent)
    }

    pub(crate) fn revise_constrained(
        &self,
        task_id: &str,
        feedback: &str,
        agent_id: &str,
    ) -> Result<()> {
        self.revise_with_agent_selection(task_id, feedback, agent_id, false)
    }

    fn revise_with_agent_selection(
        &self,
        task_id: &str,
        feedback: &str,
        agent_id: &str,
        operator_agent_override: bool,
    ) -> Result<()> {
        let agent = self.db.get_agent(agent_id)?.context("agent not found")?;
        if agent.execution_mode == registry::MANUAL {
            let task = self.db.get_task(task_id)?.context("task not found")?;
            let decision = crate::scheduler::resolve_task_economy(
                &self.db,
                &task,
                registry::AgentAction::Code,
                crate::scheduler::EconomyOverrides {
                    agent_id: operator_agent_override.then(|| agent_id.into()),
                    ..Default::default()
                },
                Some(registry::MANUAL),
                (!operator_agent_override).then(|| agent_id.into()),
                task.reasoning_effort,
                Some("revision_contract".into()),
                crate::scheduler::TransportEligibility::Strict,
                None,
                "application_manual_revision",
            )?;
            let selected = decision
                .resolution
                .ok_or_else(|| anyhow::anyhow!(decision.schedule.explanation))?
                .agent;
            agent::revise_manual(task_id, feedback, &selected, &self.db, &self.repo_path)?;
        } else {
            agent::revise_with_factory_on_db_as_with_runner(
                task_id,
                feedback,
                &self.db,
                &self.repo_path,
                agent_id,
                &crate::SystemValidationRunner,
                &agent::RevisionExecutionOverrides::default(),
                crate::scheduler::TransportEligibility::Strict,
                operator_agent_override,
                |agent, model, effort| {
                    crate::backend::WorkerFactory::build_with_overrides(agent, model, effort)
                },
            )?;
        }
        Ok(())
    }
    pub fn submit_manual_run(&self, run_id: i64, output: &str) -> Result<String> {
        let result = agent::submit_run_with_runner(
            &self.db,
            run_id,
            output,
            &self.repo_path,
            &crate::validation::SystemValidationRunner,
        )?;
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
            && (agent.execution_mode == registry::MANUAL
                || !crate::backend::provider_supports_execution_options(&agent.backend))
        {
            anyhow::bail!(
                "only automated providers with execution settings support model and reasoning-effort configuration"
            );
        }
        if agent.execution_mode == registry::AUTOMATED
            && !crate::backend::provider_supports_automated_execution(&agent.backend)
        {
            anyhow::bail!("backend '{}' requires --mode manual", agent.backend);
        }
        registry::Agent::from_definition(&agent)
            .map_err(|error| anyhow::anyhow!("invalid global agent: {error}"))?;
        // Keep the legacy registry payload lossless for CLI/read-model
        // consumers; the registry schema defaults every row to the current
        // globally owned model.
        self.db.insert_agent(&agent)?;
        Ok(())
    }

    pub fn inspect_agent_onboarding(
        &self,
        request: &crate::agent_onboarding::AgentOnboardingRequest,
    ) -> Result<crate::agent_onboarding::AgentOnboardingPreview> {
        let inspector = crate::agent_onboarding::SystemProviderOnboarding::new(&self.repo_path);
        self.inspect_agent_onboarding_with(request, &inspector)
    }

    pub fn inspect_agent_onboarding_with(
        &self,
        request: &crate::agent_onboarding::AgentOnboardingRequest,
        inspector: &dyn crate::agent_onboarding::ProviderOnboarding,
    ) -> Result<crate::agent_onboarding::AgentOnboardingPreview> {
        crate::agent_onboarding::preview(request, inspector)
    }

    /// Inspect first and persist only when the operator explicitly approves
    /// the resulting provider capabilities, permissions, and Orc roles.
    pub fn onboard_agent_with(
        &self,
        request: &crate::agent_onboarding::AgentOnboardingRequest,
        approved: bool,
        inspector: &dyn crate::agent_onboarding::ProviderOnboarding,
    ) -> Result<crate::agent_onboarding::AgentOnboardingResult> {
        let preview = self.inspect_agent_onboarding_with(request, inspector)?;
        if !approved {
            return Ok(crate::agent_onboarding::AgentOnboardingResult {
                preview,
                persisted: false,
            });
        }
        crate::agent_onboarding::persist_preview(&self.db, &preview)?;
        Ok(crate::agent_onboarding::AgentOnboardingResult {
            preview,
            persisted: true,
        })
    }

    pub fn onboard_agent(
        &self,
        request: &crate::agent_onboarding::AgentOnboardingRequest,
        approved: bool,
    ) -> Result<crate::agent_onboarding::AgentOnboardingResult> {
        let inspector = crate::agent_onboarding::SystemProviderOnboarding::new(&self.repo_path);
        self.onboard_agent_with(request, approved, &inspector)
    }

    pub fn agent_configuration(
        &self,
        id: &str,
    ) -> Result<crate::agent_onboarding::AgentConfigurationDocument> {
        crate::agent_onboarding::document_from_storage(&self.db, id)
    }

    pub fn import_agent_configuration(
        &self,
        document: &crate::agent_onboarding::AgentConfigurationDocument,
    ) -> Result<()> {
        crate::agent_onboarding::validate_document(document)?;
        self.db.upsert_global_agent_configuration(
            &document.agent,
            &document.permissions,
            &crate::storage::AgentAuthorization {
                authenticated: document.authentication.verified,
                authentication_method: document.authentication.method.clone(),
                authentication_detail: document.authentication.detail.clone(),
            },
        )?;
        Ok(())
    }

    pub fn agent_permissions(&self, id: &str) -> Result<Vec<registry::OperatorPermission>> {
        registry::get_agent(&self.db, id)?;
        Ok(self.db.agent_permissions(id)?)
    }

    pub fn add_agent_permission(
        &self,
        id: &str,
        permission: registry::OperatorPermission,
    ) -> Result<bool> {
        let mut permissions = self.agent_permissions(id)?;
        if !permissions.contains(&permission) {
            permissions.push(permission);
        }
        Ok(self.db.set_agent_permissions(id, &permissions)?)
    }

    pub fn remove_agent_permission(
        &self,
        id: &str,
        permission: &registry::OperatorPermission,
    ) -> Result<bool> {
        let mut permissions = self.agent_permissions(id)?;
        let before = permissions.len();
        permissions.retain(|value| value != permission);
        if before == permissions.len() {
            return Ok(false);
        }
        Ok(self.db.set_agent_permissions(id, &permissions)?)
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
        self.db.purge_task(id, force)?;
        if absolute.exists() && (force || path.is_some()) {
            crate::git::remove_worktree(&self.repo_path, &expected).map_err(|error| {
                anyhow::anyhow!(
                    "task '{}' was purged from persisted state, but worktree cleanup failed: {}",
                    id,
                    error
                )
            })?;
        }
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
        if agent.execution_mode != registry::AUTOMATED
            || !crate::backend::provider_supports_execution_options(&agent.backend)
        {
            anyhow::bail!("agent '{}' does not support model settings", id)
        }
        Ok(self.db.set_agent_model(id, model)?)
    }
    pub fn set_agent_effort(&self, id: &str, effort: registry::ReasoningEffort) -> Result<bool> {
        let agent = registry::get_agent(&self.db, id)?;
        if agent.execution_mode != registry::AUTOMATED
            || !crate::backend::provider_supports_execution_options(&agent.backend)
        {
            anyhow::bail!("agent '{}' does not support reasoning settings", id)
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
        crate::backend::sync_agent_quota(&self.db, &agent).map_err(anyhow::Error::msg)?;
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
    pub fn apply_approved_plan(&self) -> Result<std::collections::BTreeMap<String, String>> {
        let id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        Ok(self.db.apply_approved_plan(id)?)
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
