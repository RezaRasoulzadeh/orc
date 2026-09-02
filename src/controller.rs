//! Read-only Controller state and recommendation boundary.
//!
//! This module projects the canonical [`ProjectOperations`] read surface into
//! a deliberately bounded packet before sending it through the generic local
//! inference runtime. It owns no database handle, lifecycle mutation or model
//! backend detail. The deterministic kernel remains authoritative for every
//! fact, legal transition and eventual execution.

use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::operations::{
    BlockerState, EconomyResolutionSummary, ExecutionConditionSummary, ExecutionSummary,
    OperationalEvent, OperationalNextStep, ProjectOperations, ReviewCriterionSummary,
    ReviewOperationsSummary, TaskOperationsDetail, TaskOperationsSummary, ValidationCommandSummary,
    ValidationSummary,
};
use crate::protocol::{ReviewCriterionStatus, ReviewEvidenceKind};
use crate::queue::{DependencyInfo, QueueCategory};
use crate::registry::{EconomyTier, ReasoningEffort};
use crate::self_hosting::{SelfHostingReadiness, SelfHostingReadinessState};
use crate::task::{TaskContract, TaskPriority, TaskStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Version of the serialized Controller state packet.
pub const CONTROLLER_STATE_PACKET_VERSION: u32 = 1;
/// Maximum serialized size of a Controller state packet.
pub const MAX_CONTROLLER_PACKET_BYTES: usize = 64 * 1024;

const MAX_TEXT_BYTES: usize = 1024;
const MAX_LIST_ITEMS: usize = 16;
const MAX_EXECUTIONS: usize = 4;
const MAX_ACTIVITY: usize = 16;
const MAX_REVIEW_CRITERIA: usize = 16;
const MAX_BLOCKERS: usize = 16;
const MAX_EVIDENCE_PER_CRITERION: usize = 8;
const MAX_RECOMMENDATION_TEXT_BYTES: usize = 16 * 1024;
const MAX_RECOMMENDATION_STRUCTURED_BYTES: usize = 16 * 1024;
const MAX_RECOMMENDATION_RATIONALE_BYTES: usize = 1024;

/// Failures at the read-only Controller boundary.
#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("controller read failed: {0}")]
    Read(#[source] anyhow::Error),
    #[error("controller task was not found: {0}")]
    TaskNotFound(String),
    #[error("controller packet serialization failed: {0}")]
    Serialization(String),
    #[error(
        "controller state packet is too large: {actual_bytes} bytes exceeds {max_bytes}-byte limit"
    )]
    PacketTooLarge {
        actual_bytes: usize,
        max_bytes: usize,
    },
    #[error("controller state packet exceeds its {field} bound")]
    PacketBounds { field: String },
    #[error("controller recommendation is invalid: {0}")]
    InvalidRecommendation(String),
    #[error("local inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
}

/// A bounded, model-independent projection of current Orc project/task facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControllerStatePacket {
    pub packet_version: u32,
    pub project: ControllerProjectState,
    pub task: ControllerTaskState,
}

impl ControllerStatePacket {
    /// Validate the packet's serialized-size boundary.
    pub fn validate(&self) -> Result<(), ControllerError> {
        let value = serde_json::to_value(self)
            .map_err(|error| ControllerError::Serialization(error.to_string()))?;
        let size = serde_json::to_vec(&value)
            .map_err(|error| ControllerError::Serialization(error.to_string()))?
            .len();
        if size > MAX_CONTROLLER_PACKET_BYTES {
            return Err(ControllerError::PacketTooLarge {
                actual_bytes: size,
                max_bytes: MAX_CONTROLLER_PACKET_BYTES,
            });
        }
        validate_packet_value(&value, "packet")?;
        Ok(())
    }
}

/// Project-level facts exposed to the Controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerProjectState {
    pub name: Option<String>,
    pub self_hosting: ControllerSelfHostingState,
}

impl ControllerProjectState {
    fn from_facts(name: Option<String>, readiness: &SelfHostingReadiness) -> Self {
        Self {
            name: name.map(|value| bounded_text(&value)),
            self_hosting: ControllerSelfHostingState {
                recognized: readiness.recognized,
                repository_id: readiness.repository_id.as_deref().map(bounded_text),
                state: readiness.state,
                blocking_guards: readiness
                    .blocking_guards
                    .iter()
                    .take(MAX_LIST_ITEMS)
                    .map(|value| bounded_text(value))
                    .collect(),
            },
        }
    }
}

/// Bounded self-hosting readiness facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerSelfHostingState {
    pub recognized: bool,
    pub repository_id: Option<String>,
    pub state: SelfHostingReadinessState,
    pub blocking_guards: Vec<String>,
}

/// Bounded task state and canonical operational evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControllerTaskState {
    pub summary: ControllerTaskSummary,
    pub contract: ControllerContractSummary,
    pub dependencies: Vec<ControllerDependency>,
    pub waiting_on: Vec<String>,
    pub execution_condition: Option<ControllerExecutionCondition>,
    /// Newest executions first, limited to the most recent relevant runs.
    pub executions: Vec<ControllerExecutionSummary>,
    pub validation: ControllerValidationSummary,
    pub review: ControllerReviewSummary,
    pub blockers: Vec<ControllerBlockerSummary>,
    pub economy: ControllerEconomySummary,
    /// Newest activity first, with event payloads intentionally omitted.
    pub recent_activity: Vec<ControllerActivityEvent>,
}

impl ControllerTaskState {
    fn from_detail(detail: &TaskOperationsDetail) -> Self {
        let dependencies = detail
            .queue
            .as_ref()
            .map(|queue| {
                queue
                    .dependencies
                    .iter()
                    .take(MAX_LIST_ITEMS)
                    .map(ControllerDependency::from_dependency)
                    .collect()
            })
            .unwrap_or_default();
        let waiting_on = detail
            .queue
            .as_ref()
            .map(|queue| {
                queue
                    .waiting_on
                    .iter()
                    .take(MAX_LIST_ITEMS)
                    .map(|value| bounded_text(value))
                    .collect()
            })
            .unwrap_or_default();
        let executions = detail
            .executions
            .iter()
            .take(MAX_EXECUTIONS)
            .map(ControllerExecutionSummary::from_execution)
            .collect();
        let blockers = detail
            .blockers
            .iter()
            .take(MAX_BLOCKERS)
            .map(ControllerBlockerSummary::from_blocker)
            .collect();
        let recent_activity = detail
            .activity
            .iter()
            .rev()
            .take(MAX_ACTIVITY)
            .map(ControllerActivityEvent::from_event)
            .collect();

        Self {
            summary: ControllerTaskSummary::from_summary(&detail.summary),
            contract: ControllerContractSummary::from_contract(&detail.contract),
            dependencies,
            waiting_on,
            execution_condition: detail
                .execution_condition
                .as_ref()
                .map(ControllerExecutionCondition::from_condition),
            executions,
            validation: ControllerValidationSummary::from_validation(&detail.summary.validation),
            review: ControllerReviewSummary::from_review(
                &detail.summary.review,
                &detail.review_criteria,
            ),
            blockers,
            economy: ControllerEconomySummary::from_detail(detail),
            recent_activity,
        }
    }
}

/// The bounded task identity and canonical next-step fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerTaskSummary {
    pub task_id: String,
    pub title: String,
    pub objective: String,
    pub role: String,
    pub priority: TaskPriority,
    pub lifecycle: TaskStatus,
    pub phase: QueueCategory,
    pub next_step: OperationalNextStep,
    pub cancellation_reason: Option<String>,
}

impl ControllerTaskSummary {
    fn from_summary(summary: &TaskOperationsSummary) -> Self {
        Self {
            task_id: bounded_text(&summary.task_id),
            title: bounded_text(&summary.title),
            objective: bounded_text(&summary.objective),
            role: bounded_text(&summary.role),
            priority: summary.priority,
            lifecycle: summary.lifecycle,
            phase: summary.phase,
            next_step: summary.next_step,
            cancellation_reason: summary.cancellation_reason.as_deref().map(bounded_text),
        }
    }
}

/// Bounded task contract context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerContractSummary {
    pub unchanged: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub required_tests: Vec<String>,
    pub validation: Vec<String>,
}

impl ControllerContractSummary {
    fn from_contract(contract: &TaskContract) -> Self {
        Self {
            unchanged: bounded_list(&contract.unchanged),
            acceptance_criteria: bounded_list(&contract.acceptance_criteria),
            required_tests: bounded_list(&contract.required_tests),
            validation: bounded_list(&contract.validation),
        }
    }
}

/// One bounded dependency fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerDependency {
    pub task_id: String,
    pub status: Option<TaskStatus>,
    pub is_done: bool,
}

impl ControllerDependency {
    fn from_dependency(dependency: &DependencyInfo) -> Self {
        Self {
            task_id: bounded_text(&dependency.task_id),
            status: dependency.status,
            is_done: dependency.is_done,
        }
    }
}

/// Bounded execution-condition fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerExecutionCondition {
    pub kind: String,
    pub details: String,
    pub created_at: String,
}

impl ControllerExecutionCondition {
    fn from_condition(condition: &ExecutionConditionSummary) -> Self {
        Self {
            kind: bounded_text(&condition.kind),
            details: bounded_text(&condition.details),
            created_at: bounded_text(&condition.created_at),
        }
    }
}

/// Bounded execution/run evidence. Raw run output is intentionally omitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerExecutionSummary {
    pub id: i64,
    pub agent: String,
    pub execution_mode: String,
    pub execution_class: String,
    pub status: String,
    pub phase: Option<String>,
    pub is_active: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub last_activity: String,
    pub outcome: Option<String>,
    pub failure_category: Option<String>,
    pub persisted_model: Option<String>,
    pub persisted_effort: Option<ReasoningEffort>,
    pub persisted_resolution_source: String,
    pub error: Option<String>,
}

impl ControllerExecutionSummary {
    fn from_execution(execution: &ExecutionSummary) -> Self {
        Self {
            id: execution.id,
            agent: bounded_text(&execution.agent),
            execution_mode: bounded_text(&execution.execution_mode),
            execution_class: bounded_text(&execution.execution_class),
            status: bounded_text(&execution.status),
            phase: execution.phase.as_deref().map(bounded_text),
            is_active: execution.is_active,
            started_at: bounded_text(&execution.started_at),
            finished_at: execution.finished_at.as_deref().map(bounded_text),
            last_activity: bounded_text(&execution.last_activity),
            outcome: execution.outcome.as_deref().map(bounded_text),
            failure_category: execution.failure_category.as_deref().map(bounded_text),
            persisted_model: execution.persisted_model.as_deref().map(bounded_text),
            persisted_effort: execution.persisted_effort,
            persisted_resolution_source: bounded_text(&execution.persisted_resolution_source),
            error: execution.error.as_deref().map(bounded_text),
        }
    }
}

/// Bounded validation facts and selected command names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerValidationSummary {
    pub state: crate::operations::ValidationState,
    pub recorded_state: Option<crate::operations::ValidationState>,
    pub run_id: Option<i64>,
    pub timestamp: Option<String>,
    pub latest_passing_run_id: Option<i64>,
    pub latest_passing_timestamp: Option<String>,
    pub is_current: Option<bool>,
    pub worktree_fingerprint: Option<String>,
    pub selected_commands: Vec<ControllerValidationCommand>,
    pub failure_classification: Option<crate::validation::ValidationFailureClassification>,
}

impl ControllerValidationSummary {
    fn from_validation(validation: &ValidationSummary) -> Self {
        Self {
            state: validation.state,
            recorded_state: validation.recorded_state,
            run_id: validation.run_id,
            timestamp: validation.timestamp.as_deref().map(bounded_text),
            latest_passing_run_id: validation.latest_passing_run_id,
            latest_passing_timestamp: validation
                .latest_passing_timestamp
                .as_deref()
                .map(bounded_text),
            is_current: validation.is_current,
            worktree_fingerprint: validation.worktree_fingerprint.as_deref().map(bounded_text),
            selected_commands: validation
                .selected_commands
                .iter()
                .take(MAX_LIST_ITEMS)
                .map(ControllerValidationCommand::from_command)
                .collect(),
            failure_classification: validation.failure_classification,
        }
    }
}

/// One selected validation command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerValidationCommand {
    pub command: String,
    pub passed: Option<bool>,
    pub failure_classification: Option<crate::validation::ValidationFailureClassification>,
}

impl ControllerValidationCommand {
    fn from_command(command: &ValidationCommandSummary) -> Self {
        Self {
            command: bounded_text(&command.command),
            passed: command.passed,
            failure_classification: command.failure_classification,
        }
    }
}

/// Review summary and bounded criterion evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerReviewSummary {
    pub run_id: Option<i64>,
    pub verdict: Option<String>,
    pub timestamp: Option<String>,
    pub applies_to_current_change: Option<bool>,
    pub ready_for_review: bool,
    pub actionable_blockers: usize,
    pub unresolved_blockers: usize,
    pub regressed_blockers: usize,
    pub resolved_blockers: usize,
    pub total_criteria: usize,
    pub satisfied_criteria: usize,
    pub violated_criteria: usize,
    pub insufficient_evidence_criteria: usize,
    pub criteria: Vec<ControllerReviewCriterion>,
}

impl ControllerReviewSummary {
    fn from_review(review: &ReviewOperationsSummary, criteria: &[ReviewCriterionSummary]) -> Self {
        Self {
            run_id: review.run_id,
            verdict: review.verdict.as_deref().map(bounded_text),
            timestamp: review.timestamp.as_deref().map(bounded_text),
            applies_to_current_change: review.applies_to_current_change,
            ready_for_review: review.ready_for_review,
            actionable_blockers: review.actionable_blockers,
            unresolved_blockers: review.unresolved_blockers,
            regressed_blockers: review.regressed_blockers,
            resolved_blockers: review.resolved_blockers,
            total_criteria: review.total_criteria,
            satisfied_criteria: review.satisfied_criteria,
            violated_criteria: review.violated_criteria,
            insufficient_evidence_criteria: review.insufficient_evidence_criteria,
            criteria: criteria
                .iter()
                .take(MAX_REVIEW_CRITERIA)
                .map(ControllerReviewCriterion::from_criterion)
                .collect(),
        }
    }
}

/// One criterion-level Review judgment and bounded evidence references.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerReviewCriterion {
    pub criterion_id: String,
    pub criterion: String,
    pub status: ReviewCriterionStatus,
    pub evidence: Vec<ControllerEvidenceRef>,
    pub rationale: String,
}

impl ControllerReviewCriterion {
    fn from_criterion(criterion: &ReviewCriterionSummary) -> Self {
        Self {
            criterion_id: bounded_text(&criterion.criterion_id),
            criterion: bounded_text(&criterion.criterion),
            status: criterion.status,
            evidence: criterion
                .evidence
                .iter()
                .take(MAX_EVIDENCE_PER_CRITERION)
                .map(ControllerEvidenceRef::from_evidence)
                .collect(),
            rationale: bounded_text(&criterion.rationale),
        }
    }
}

/// A bounded evidence reference with no raw transcript or payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerEvidenceRef {
    pub kind: ReviewEvidenceKind,
    pub reference: String,
    pub explanation: String,
}

impl ControllerEvidenceRef {
    fn from_evidence(evidence: &crate::protocol::ReviewEvidenceRef) -> Self {
        Self {
            kind: evidence.kind,
            reference: bounded_text(&evidence.reference),
            explanation: bounded_text(&evidence.explanation),
        }
    }
}

/// Bounded blocker information useful for a read-only recommendation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerBlockerSummary {
    pub id: String,
    pub key: String,
    pub state: BlockerState,
    pub actionable: bool,
    pub summary: String,
    pub requirement: String,
    pub evidence: String,
    pub severity: String,
    pub acceptance_condition: String,
    pub originating_review_run_id: i64,
}

impl ControllerBlockerSummary {
    fn from_blocker(blocker: &crate::operations::BlockerSummary) -> Self {
        Self {
            id: bounded_text(&blocker.id),
            key: bounded_text(&blocker.key),
            state: blocker.state,
            actionable: blocker.actionable,
            summary: bounded_text(&blocker.summary),
            requirement: bounded_text(&blocker.requirement),
            evidence: bounded_text(&blocker.evidence),
            severity: bounded_text(&blocker.severity),
            acceptance_condition: bounded_text(&blocker.acceptance_condition),
            originating_review_run_id: blocker.originating_review_run_id,
        }
    }
}

/// Economy/model resolution facts, without provider internals or raw context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControllerEconomySummary {
    pub invocation_count: usize,
    pub escalation_count: usize,
    pub latest_resolution: Option<ControllerResolutionSummary>,
}

impl ControllerEconomySummary {
    fn from_detail(detail: &TaskOperationsDetail) -> Self {
        Self {
            invocation_count: detail.resolutions.len(),
            escalation_count: detail.escalations.len(),
            latest_resolution: detail
                .summary
                .latest_resolution
                .as_ref()
                .map(ControllerResolutionSummary::from_resolution),
        }
    }
}

/// Latest selected model/tier/quota facts for this task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControllerResolutionSummary {
    pub invocation_id: i64,
    pub purpose: String,
    pub action: Option<String>,
    pub attempt: usize,
    pub outcome: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
    pub tier: EconomyTier,
    pub selection_reason: Option<String>,
    pub operator_override: bool,
    pub escalation_reason: Option<String>,
    pub quota: Option<ControllerQuotaSummary>,
}

impl ControllerResolutionSummary {
    fn from_resolution(resolution: &EconomyResolutionSummary) -> Self {
        Self {
            invocation_id: resolution.invocation_id,
            purpose: bounded_text(&resolution.purpose),
            action: resolution.action.as_deref().map(bounded_text),
            attempt: resolution.attempt,
            outcome: bounded_text(&resolution.outcome),
            agent: resolution.agent.as_deref().map(bounded_text),
            model: resolution.model.as_deref().map(bounded_text),
            effort: resolution.effort,
            tier: resolution.tier,
            selection_reason: resolution.selection_reason.as_deref().map(bounded_text),
            operator_override: resolution.operator_override,
            escalation_reason: resolution.escalation_reason.as_deref().map(bounded_text),
            quota: resolution
                .quota
                .as_ref()
                .map(ControllerQuotaSummary::from_quota),
        }
    }
}

/// Bounded quota observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerQuotaSummary {
    pub remaining_percent: Option<i64>,
    pub reset_at: Option<String>,
    pub checked_at: Option<String>,
    pub source: Option<String>,
    pub freshness: Option<String>,
    pub reserve_percent: Option<i64>,
}

impl ControllerQuotaSummary {
    fn from_quota(quota: &crate::operations::QuotaObservationSummary) -> Self {
        Self {
            remaining_percent: quota.remaining_percent,
            reset_at: quota.reset_at.as_deref().map(bounded_text),
            checked_at: quota.checked_at.as_deref().map(bounded_text),
            source: quota.source.as_deref().map(bounded_text),
            freshness: quota.freshness.as_deref().map(bounded_text),
            reserve_percent: quota.reserve_percent,
        }
    }
}

/// Recent lifecycle/activity fact without its unbounded payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerActivityEvent {
    pub id: i64,
    pub timestamp: String,
    pub kind: String,
    pub run_id: Option<i64>,
    pub agent_id: Option<String>,
}

impl ControllerActivityEvent {
    fn from_event(event: &OperationalEvent) -> Self {
        Self {
            id: event.id,
            timestamp: bounded_text(&event.timestamp),
            kind: bounded_text(&event.kind),
            run_id: event.run_id,
            agent_id: event.agent_id.as_deref().map(bounded_text),
        }
    }
}

/// Typed advisory output from the Controller. It grants no execution rights.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControllerRecommendation {
    pub task_id: String,
    pub response_text: String,
    pub suggested_next_step: Option<OperationalNextStep>,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
}

impl ControllerRecommendation {
    fn from_response(
        task_id: &str,
        response: LocalInferenceResponse,
    ) -> Result<Self, ControllerError> {
        if response.text.len() > MAX_RECOMMENDATION_TEXT_BYTES {
            return Err(ControllerError::InvalidRecommendation(format!(
                "response text exceeds the {MAX_RECOMMENDATION_TEXT_BYTES}-byte limit"
            )));
        }
        let value = response.structured_output.as_ref().ok_or_else(|| {
            ControllerError::InvalidRecommendation(
                "structured response is missing its JSON object".into(),
            )
        })?;
        let size = serde_json::to_vec(value)
            .map_err(|error| ControllerError::InvalidRecommendation(error.to_string()))?
            .len();
        if size > MAX_RECOMMENDATION_STRUCTURED_BYTES {
            return Err(ControllerError::InvalidRecommendation(format!(
                "structured output exceeds the {MAX_RECOMMENDATION_STRUCTURED_BYTES}-byte limit"
            )));
        }
        let (suggested_next_step, rationale) = validate_recommendation_value(value)?;
        if rationale.len() > MAX_RECOMMENDATION_TEXT_BYTES {
            return Err(ControllerError::InvalidRecommendation(format!(
                "rationale exceeds the {MAX_RECOMMENDATION_TEXT_BYTES}-byte limit"
            )));
        }
        if response.text.trim().is_empty() && rationale.trim().is_empty() {
            return Err(ControllerError::InvalidRecommendation(
                "response text and rationale must not both be empty".into(),
            ));
        }
        Ok(Self {
            task_id: bounded_text(task_id),
            response_text: response.text,
            suggested_next_step,
            rationale: bounded_text(&rationale),
            structured_output: response.structured_output,
        })
    }
}

fn validate_recommendation_value(
    value: &Value,
) -> Result<(Option<OperationalNextStep>, String), ControllerError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerError::InvalidRecommendation("structured response must be a JSON object".into())
    })?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "suggested_next_step" | "decision_class" | "rationale" | "confidence"
        ) {
            return Err(ControllerError::InvalidRecommendation(format!(
                "structured response contains unsupported field `{key}`"
            )));
        }
    }
    let step_value = object.get("suggested_next_step").ok_or_else(|| {
        ControllerError::InvalidRecommendation("suggested_next_step is required".into())
    })?;
    let suggested_next_step = if step_value.is_null() {
        None
    } else {
        Some(serde_json::from_value(step_value.clone()).map_err(|error| {
            ControllerError::InvalidRecommendation(format!(
                "suggested_next_step is invalid: {error}"
            ))
        })?)
    };
    let decision_class = object
        .get("decision_class")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ControllerError::InvalidRecommendation(
                "decision_class must be `action` or `operator_decision`".into(),
            )
        })?;
    match decision_class {
        "action" if suggested_next_step.is_some() => {}
        "operator_decision" if suggested_next_step.is_none() => {}
        "action" => {
            return Err(ControllerError::InvalidRecommendation(
                "action recommendations require suggested_next_step".into(),
            ));
        }
        "operator_decision" => {
            return Err(ControllerError::InvalidRecommendation(
                "operator_decision recommendations require a null suggested_next_step".into(),
            ));
        }
        _ => {
            return Err(ControllerError::InvalidRecommendation(
                "decision_class must be `action` or `operator_decision`".into(),
            ));
        }
    }
    let rationale = object
        .get("rationale")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ControllerError::InvalidRecommendation("rationale must be a string".into())
        })?;
    if rationale.is_empty() || rationale.len() > MAX_RECOMMENDATION_RATIONALE_BYTES {
        return Err(ControllerError::InvalidRecommendation(format!(
            "rationale must be non-empty and at most {MAX_RECOMMENDATION_RATIONALE_BYTES} bytes"
        )));
    }
    if let Some(confidence) = object.get("confidence") {
        let confidence = confidence.as_f64().ok_or_else(|| {
            ControllerError::InvalidRecommendation("confidence must be a number from 0 to 1".into())
        })?;
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(ControllerError::InvalidRecommendation(
                "confidence must be a number from 0 to 1".into(),
            ));
        }
    }
    Ok((suggested_next_step, rationale.to_owned()))
}

/// The model-independent JSON contract requested for Controller advice.
///
/// The local runtime adapter may translate this schema into a native grammar,
/// but Controller types never depend on that representation.
fn recommendation_response_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "suggested_next_step": {
                "anyOf": [
                    {"type": "string", "enum": [
                        "dispatch", "wait_for_execution", "run_semantic_review",
                        "revise", "accept", "resolve_blocker", "satisfy_dependencies",
                        "configure_eligible_agent", "none"
                    ]},
                    {"type": "null"}
                ]
            },
            "decision_class": {
                "type": "string",
                "enum": ["action", "operator_decision"]
            },
            "rationale": {"type": "string", "minLength": 1, "maxLength": 1024},
            "confidence": {"type": "number", "minimum": 0, "maximum": 1}
        },
        "required": ["suggested_next_step", "decision_class", "rationale"],
        "additionalProperties": false
    })
}

/// Builds bounded Controller state and obtains advisory recommendations.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerStateBuilder;

impl ControllerStateBuilder {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Build a packet from the provider-independent, non-mutating read seam.
    pub fn build(
        &self,
        operations: &ProjectOperations<'_>,
        task_id: &str,
    ) -> Result<ControllerStatePacket, ControllerError> {
        if task_id.trim().is_empty() {
            return Err(ControllerError::TaskNotFound(task_id.to_owned()));
        }
        let name = operations.project_name().map_err(ControllerError::Read)?;
        let readiness = operations.self_hosting_readiness();
        let detail = operations
            .task_detail(task_id)
            .map_err(ControllerError::Read)?
            .ok_or_else(|| ControllerError::TaskNotFound(task_id.to_owned()))?;
        let packet = ControllerStatePacket {
            packet_version: CONTROLLER_STATE_PACKET_VERSION,
            project: ControllerProjectState::from_facts(name, &readiness),
            task: ControllerTaskState::from_detail(&detail),
        };
        packet.validate()?;
        Ok(packet)
    }

    /// Build state and obtain a typed advisory recommendation through the
    /// model-independent runtime. This method cannot mutate `OrcApp` or the
    /// canonical database.
    pub fn recommend(
        &self,
        operations: &ProjectOperations<'_>,
        task_id: &str,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerRecommendation, ControllerError> {
        let packet = self.build(operations, task_id)?;
        self.recommend_packet(&packet, runtime)
    }

    /// Obtain an advisory recommendation from an already-built packet.
    ///
    /// This packet-level entry point lets deterministic evaluation and later
    /// read-only callers reuse the exact recommendation path without gaining
    /// access to `OrcApp`, storage or lifecycle mutation.
    pub fn recommend_packet(
        &self,
        packet: &ControllerStatePacket,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerRecommendation, ControllerError> {
        packet.validate()?;
        let task_id = &packet.task.summary.task_id;
        let packet_json = serde_json::to_string(&packet)
            .map_err(|error| ControllerError::Serialization(error.to_string()))?;
        let prompt = format!(
            "Return exactly one JSON object with `suggested_next_step` (a canonical next-step value or null), `decision_class` (action or operator_decision), and a short `rationale`; optionally include numeric `confidence` from 0 to 1. Do not include prose before or after the object.\n\
Do not claim to have executed an action. Use the canonical state below:\n\n\
{packet_json}"
        );
        let parameters = LocalInferenceParameters {
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: recommendation_response_schema(),
            },
            ..Default::default()
        };
        let request =
            LocalInferenceRequest::new(prompt, parameters).map_err(ControllerError::Inference)?;
        let response = runtime
            .infer(&request)
            .map_err(ControllerError::Inference)?;
        ControllerRecommendation::from_response(task_id, response)
    }
}

fn bounded_text(value: &str) -> String {
    let mut end = value.len().min(MAX_TEXT_BYTES);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn bounded_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .take(MAX_LIST_ITEMS)
        .map(|value| bounded_text(value))
        .collect()
}

fn validate_packet_value(value: &Value, field: &str) -> Result<(), ControllerError> {
    match value {
        Value::String(value) if value.len() > MAX_TEXT_BYTES => {
            Err(ControllerError::PacketBounds {
                field: field.to_owned(),
            })
        }
        Value::Array(values) if values.len() > MAX_LIST_ITEMS => {
            Err(ControllerError::PacketBounds {
                field: field.to_owned(),
            })
        }
        Value::Object(values) => values
            .iter()
            .try_for_each(|(name, value)| validate_packet_value(value, &format!("{field}.{name}"))),
        Value::Array(values) => values.iter().enumerate().try_for_each(|(index, value)| {
            validate_packet_value(value, &format!("{field}[{index}]"))
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::ProjectOperations;
    use crate::storage::Database;
    use crate::task::{CreateTaskInput, TaskScopeMode};
    use tempfile::{TempDir, tempdir};

    struct FakeRuntime {
        result: Result<LocalInferenceResponse, LocalInferenceError>,
        requests: Vec<LocalInferenceRequest>,
    }

    impl FakeRuntime {
        fn responding(response: LocalInferenceResponse) -> Self {
            Self {
                result: Ok(response),
                requests: Vec::new(),
            }
        }

        fn failing(error: LocalInferenceError) -> Self {
            Self {
                result: Err(error),
                requests: Vec::new(),
            }
        }
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            request: &LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            self.requests.push(request.clone());
            self.result.clone()
        }
    }

    fn project_with_task(title: &str) -> (TempDir, Database, String) {
        let directory = tempdir().expect("temporary project directory");
        let database = Database::init(directory.path().join("orc.db")).expect("database");
        let project = database.create_project("controller-test").expect("project");
        let task = database
            .create_task(
                project,
                &CreateTaskInput {
                    title: title.into(),
                    objective: "inspect the current project state".into(),
                    role: "developer".into(),
                    priority: TaskPriority::Normal,
                    required_capabilities: vec![],
                    scope_mode: Some(TaskScopeMode::Focused),
                    context_files: vec![],
                    expected_changes: vec![],
                    dependencies: vec![],
                },
            )
            .expect("task");
        (directory, database, task)
    }

    #[test]
    fn packet_uses_canonical_read_facts_and_omits_unbounded_run_output() {
        let (directory, database, task_id) = project_with_task(&"x".repeat(MAX_TEXT_BYTES + 100));
        let operations = ProjectOperations::new(&database, directory.path());
        let packet = ControllerStateBuilder::new()
            .build(&operations, &task_id)
            .expect("bounded packet");

        assert_eq!(packet.packet_version, CONTROLLER_STATE_PACKET_VERSION);
        assert_eq!(packet.task.summary.task_id, task_id);
        assert_eq!(packet.task.summary.title.len(), MAX_TEXT_BYTES);
        assert!(packet.validate().is_ok());
        let serialized = serde_json::to_string(&packet).expect("packet serialization");
        assert!(!serialized.contains("llama"));
        assert!(!serialized.contains("gguf"));
        assert!(!serialized.contains("\"output\""));
    }

    #[test]
    fn packet_rejects_oversized_serialized_state() {
        let packet = ControllerStatePacket {
            packet_version: CONTROLLER_STATE_PACKET_VERSION,
            project: ControllerProjectState {
                name: Some("x".repeat(MAX_CONTROLLER_PACKET_BYTES)),
                self_hosting: ControllerSelfHostingState {
                    recognized: false,
                    repository_id: None,
                    state: SelfHostingReadinessState::NotApplicable,
                    blocking_guards: vec![],
                },
            },
            task: ControllerTaskState {
                summary: ControllerTaskSummary {
                    task_id: "task".into(),
                    title: "title".into(),
                    objective: "objective".into(),
                    role: "developer".into(),
                    priority: TaskPriority::Normal,
                    lifecycle: TaskStatus::Backlog,
                    phase: QueueCategory::Backlog,
                    next_step: OperationalNextStep::ConfigureEligibleAgent,
                    cancellation_reason: None,
                },
                contract: ControllerContractSummary {
                    unchanged: vec![],
                    acceptance_criteria: vec![],
                    required_tests: vec![],
                    validation: vec![],
                },
                dependencies: vec![],
                waiting_on: vec![],
                execution_condition: None,
                executions: vec![],
                validation: ControllerValidationSummary {
                    state: crate::operations::ValidationState::None,
                    recorded_state: None,
                    run_id: None,
                    timestamp: None,
                    latest_passing_run_id: None,
                    latest_passing_timestamp: None,
                    is_current: None,
                    worktree_fingerprint: None,
                    selected_commands: vec![],
                    failure_classification: None,
                },
                review: ControllerReviewSummary {
                    run_id: None,
                    verdict: None,
                    timestamp: None,
                    applies_to_current_change: None,
                    ready_for_review: false,
                    actionable_blockers: 0,
                    unresolved_blockers: 0,
                    regressed_blockers: 0,
                    resolved_blockers: 0,
                    total_criteria: 0,
                    satisfied_criteria: 0,
                    violated_criteria: 0,
                    insufficient_evidence_criteria: 0,
                    criteria: vec![],
                },
                blockers: vec![],
                economy: ControllerEconomySummary {
                    invocation_count: 0,
                    escalation_count: 0,
                    latest_resolution: None,
                },
                recent_activity: vec![],
            },
        };
        assert!(matches!(
            packet.validate(),
            Err(ControllerError::PacketTooLarge { .. })
        ));
    }

    #[test]
    fn recommendation_propagates_typed_fields_without_mutating_state() {
        let (directory, database, task_id) = project_with_task("recommend");
        let operations = ProjectOperations::new(&database, directory.path());
        let before = operations
            .task_detail(&task_id)
            .expect("task detail before")
            .expect("task before");
        let response = LocalInferenceResponse {
            text: "Inspect the task before dispatch.".into(),
            structured_output: Some(serde_json::json!({
                "suggested_next_step": "dispatch",
                "decision_class": "action",
                "rationale": "The task is ready for an eligible worker."
            })),
        };
        let mut runtime = FakeRuntime::responding(response);
        let recommendation = ControllerStateBuilder::new()
            .recommend(&operations, &task_id, &mut runtime)
            .expect("recommendation");

        assert_eq!(recommendation.task_id, task_id);
        assert_eq!(
            recommendation.suggested_next_step,
            Some(OperationalNextStep::Dispatch)
        );
        assert_eq!(
            recommendation.rationale,
            "The task is ready for an eligible worker."
        );
        assert_eq!(runtime.requests.len(), 1);
        assert!(runtime.requests[0].prompt.contains("recommend"));
        assert!(!runtime.requests[0].prompt.contains("llama"));
        assert!(matches!(
            runtime.requests[0].parameters.response_format,
            LocalInferenceResponseFormat::JsonSchema { .. }
        ));
        let after = operations
            .task_detail(&task_id)
            .expect("task detail after")
            .expect("task after");
        assert_eq!(after, before);
    }

    #[test]
    fn recommendation_propagates_typed_runtime_errors() {
        let (directory, database, task_id) = project_with_task("failure");
        let operations = ProjectOperations::new(&database, directory.path());
        let error = LocalInferenceError::Backend("fake failure".into());
        let mut runtime = FakeRuntime::failing(error.clone());

        assert!(matches!(
            ControllerStateBuilder::new().recommend(&operations, &task_id, &mut runtime),
            Err(ControllerError::Inference(LocalInferenceError::Backend(message)))
                if message == "fake failure"
        ));
    }

    #[test]
    fn recommendation_rejects_unknown_structured_fields() {
        let (directory, database, task_id) = project_with_task("invalid-structured");
        let operations = ProjectOperations::new(&database, directory.path());
        let response = LocalInferenceResponse::structured(
            "unused",
            serde_json::json!({
                "suggested_next_step": "dispatch",
                "decision_class": "action",
                "rationale": "The task is ready.",
                "unexpected": true
            }),
        );
        let mut runtime = FakeRuntime::responding(response);

        assert!(matches!(
            ControllerStateBuilder::new().recommend(&operations, &task_id, &mut runtime),
            Err(ControllerError::InvalidRecommendation(message))
                if message.contains("unsupported field")
        ));
    }

    #[test]
    fn missing_task_is_reported_without_inference() {
        let (directory, database, _) = project_with_task("existing");
        let operations = ProjectOperations::new(&database, directory.path());
        let mut runtime = FakeRuntime::responding(LocalInferenceResponse::text("unused"));

        assert!(matches!(
            ControllerStateBuilder::new().recommend(&operations, "missing", &mut runtime),
            Err(ControllerError::TaskNotFound(task)) if task == "missing"
        ));
        assert!(runtime.requests.is_empty());
    }
}
