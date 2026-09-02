//! Read-only, bounded recovery facts and legal recovery operations.
//!
//! This module projects canonical [`ProjectOperations`] facts into a small
//! model-independent recovery surface. It never executes an operation,
//! selects a preferred operation, or mints Controller authorization.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::operations::{
    BlockerState, OperationalAction, OperationalActionLegality, OperationalRequeueLegality,
    OperationalRequeueRejection, ProjectOperations, RevisionLineageSummary, TaskOperationsDetail,
    ValidationState,
};
use crate::queue::{BlockingReason, QueueCategory};
use crate::scheduler::{CandidateStatus, RejectionReason};
use crate::task::TaskStatus;
use crate::validation::ValidationFailureClassification;

const MAX_RECOVERY_TEXT_BYTES: usize = 256;
const MAX_RECOVERY_ITEMS: usize = 16;

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("recovery task was not found: {0}")]
    TaskNotFound(String),
    #[error("recovery observation read failed: {0}")]
    Read(#[source] anyhow::Error),
}

/// The bounded state classification exposed to later Controller reasoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryObservationState {
    Stable,
    Abnormal,
    Ambiguous,
}

/// Distinct canonical abnormal-state facts. There is intentionally no generic
/// `Blocked` recovery condition: persisted lifecycle blocking is represented by
/// its more specific fact when one is known, or by `Ambiguous` otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryCondition {
    BlockedRevision,
    InfrastructureFailure,
    DependencyBlocked,
    NoEligibleAgent,
    EconomyExhaustion,
    NonConvergenceReplanRequired,
    SemanticRevision,
    ValidationFailure,
    ExecutionFailure,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryExecutionConditionKind {
    NonConvergenceReplanRequired,
    NonConvergenceReplanAcknowledged,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryExecutionCondition {
    pub kind: RecoveryExecutionConditionKind,
    pub details_present: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReviewVerdict {
    Pass,
    Revise,
    Reject,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryValidationFacts {
    pub state: ValidationState,
    pub failure_classification: Option<ValidationFailureClassification>,
    pub is_current: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReviewFacts {
    pub run_id: Option<i64>,
    pub verdict: Option<RecoveryReviewVerdict>,
    pub applies_to_current_change: Option<bool>,
}

/// Only lineage identity is exposed. Review feedback and persisted contract
/// contents remain outside the recovery observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRevisionLineage {
    pub actionable_review_run_id: Option<i64>,
    pub actionable_contract_id: Option<i64>,
    pub contract_source_review_run_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryDependency {
    pub task_id: String,
    pub status: Option<TaskStatus>,
    pub is_done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryBlockerState {
    New,
    Unresolved,
    Regressed,
    Resolved,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryBlockerFacts {
    pub state: RecoveryBlockerState,
    pub actionable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryExecutionClass {
    Implementation,
    Review,
    Revision,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryExecutionStatus {
    Running,
    WaitingExternal,
    Completed,
    Failed,
    Cancelled,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryExecutionFailure {
    Infrastructure,
    Validation,
    Execution,
    Unknown,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryExecutionFacts {
    pub id: i64,
    pub class: RecoveryExecutionClass,
    pub status: RecoveryExecutionStatus,
    pub failure: RecoveryExecutionFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAgentEconomyConstraint {
    Disabled,
    Unavailable,
    UnsupportedBackend,
    UnsupportedMode,
    MissingCapability,
    QuotaExhausted,
    QuotaReserve,
    QuotaRefreshFailed,
    Busy,
    ModeMismatch,
    UnsupportedAction,
    AgentConstraint,
    BelowEscalationTier,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAgentEconomyFacts {
    pub candidate_count: usize,
    pub eligible_count: usize,
    pub constraints: Vec<RecoveryAgentEconomyConstraint>,
}

/// Bounded canonical recovery facts. All strings are task/dependency identity
/// values or bounded flags; provider output, commands, paths and credentials
/// are intentionally omitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryObservation {
    pub task_id: String,
    pub state: RecoveryObservationState,
    pub lifecycle: TaskStatus,
    pub queue_phase: QueueCategory,
    pub conditions: Vec<RecoveryCondition>,
    pub execution_condition: Option<RecoveryExecutionCondition>,
    pub validation: RecoveryValidationFacts,
    pub review: RecoveryReviewFacts,
    pub revision: RecoveryRevisionLineage,
    pub latest_execution: Option<RecoveryExecutionFacts>,
    pub dependencies: Vec<RecoveryDependency>,
    pub blockers: Vec<RecoveryBlockerFacts>,
    pub agent_economy: RecoveryAgentEconomyFacts,
}

/// Recovery operations backed by existing repository behavior. This is a
/// description of an operation, not an executable command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryOperation {
    Requeue,
    ResumeRevision,
    AcknowledgeNonConvergence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryOperationRejection {
    TaskNotRecoverable,
    ActiveExecution,
    NoRecoverableRun,
    RevisionLineageMissing,
    RequeueWouldDiscardRevisionLineage,
    DependenciesBlocking,
    ExecutionConditionPresent,
    NoExecutionCondition,
    UnsupportedExecutionCondition,
    TerminalTask,
    CanonicalLegalityRejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum RecoveryOperationLegality {
    Allowed {
        operation: RecoveryOperation,
    },
    Rejected {
        operation: RecoveryOperation,
        reason: RecoveryOperationRejection,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryInspection {
    pub observation: RecoveryObservation,
    pub operations: Vec<RecoveryOperationLegality>,
}

/// Observe one task through the canonical provider-independent operations
/// surface. This function performs no lifecycle, persistence, worktree or
/// provider mutation.
pub fn observe_recovery(
    operations: &ProjectOperations<'_>,
    task_id: &str,
) -> Result<RecoveryObservation, RecoveryError> {
    let detail = operations
        .task_detail(task_id)
        .map_err(RecoveryError::Read)?
        .ok_or_else(|| RecoveryError::TaskNotFound(bounded_text(task_id)))?;
    Ok(observation_from_detail(&detail))
}

/// Inspect the fixed repository-grounded recovery operation set. The result
/// reports legality for every operation in stable order and never ranks or
/// selects one.
pub fn inspect_recovery(
    operations: &ProjectOperations<'_>,
    task_id: &str,
) -> Result<RecoveryInspection, RecoveryError> {
    let detail = operations
        .task_detail(task_id)
        .map_err(RecoveryError::Read)?
        .ok_or_else(|| RecoveryError::TaskNotFound(bounded_text(task_id)))?;
    let observation = observation_from_detail(&detail);
    let operations = [
        RecoveryOperation::Requeue,
        RecoveryOperation::ResumeRevision,
        RecoveryOperation::AcknowledgeNonConvergence,
    ]
    .into_iter()
    .map(|operation| inspect_operation(operations, task_id, &observation, operation))
    .collect::<Result<Vec<_>, _>>()?;
    Ok(RecoveryInspection {
        observation,
        operations,
    })
}

fn inspect_operation(
    operations: &ProjectOperations<'_>,
    task_id: &str,
    observation: &RecoveryObservation,
    operation: RecoveryOperation,
) -> Result<RecoveryOperationLegality, RecoveryError> {
    let rejected = |reason| RecoveryOperationLegality::Rejected { operation, reason };
    match operation {
        RecoveryOperation::Requeue => {
            if observation.lifecycle == TaskStatus::Blocked
                && observation.revision.actionable_review_run_id.is_some()
            {
                return Ok(rejected(
                    RecoveryOperationRejection::RequeueWouldDiscardRevisionLineage,
                ));
            }
            if !observation.dependencies.is_empty() {
                return Ok(rejected(RecoveryOperationRejection::DependenciesBlocking));
            }
            match operations
                .inspect_requeue(task_id)
                .map_err(RecoveryError::Read)?
            {
                OperationalRequeueLegality::Allowed => {
                    Ok(RecoveryOperationLegality::Allowed { operation })
                }
                OperationalRequeueLegality::Rejected { reason } => Ok(rejected(match reason {
                    OperationalRequeueRejection::ActiveExecution => {
                        RecoveryOperationRejection::ActiveExecution
                    }
                    OperationalRequeueRejection::NoRecoverableRun => {
                        RecoveryOperationRejection::NoRecoverableRun
                    }
                    OperationalRequeueRejection::TaskNotFound
                    | OperationalRequeueRejection::TaskNotActive => {
                        RecoveryOperationRejection::TaskNotRecoverable
                    }
                })),
            }
        }
        RecoveryOperation::ResumeRevision => {
            if !observation.dependencies.is_empty() {
                return Ok(rejected(RecoveryOperationRejection::DependenciesBlocking));
            }
            if observation.execution_condition.is_some() {
                return Ok(rejected(
                    RecoveryOperationRejection::ExecutionConditionPresent,
                ));
            }
            if observation.revision.actionable_review_run_id.is_none() {
                return Ok(rejected(RecoveryOperationRejection::RevisionLineageMissing));
            }
            match operations
                .inspect_action(task_id, OperationalAction::Revise)
                .map_err(RecoveryError::Read)?
            {
                OperationalActionLegality::Allowed { .. } => {
                    Ok(RecoveryOperationLegality::Allowed { operation })
                }
                OperationalActionLegality::Rejected { .. } => Ok(rejected(
                    RecoveryOperationRejection::CanonicalLegalityRejected,
                )),
            }
        }
        RecoveryOperation::AcknowledgeNonConvergence => {
            if observation.lifecycle.is_terminal() {
                return Ok(rejected(RecoveryOperationRejection::TerminalTask));
            }
            match observation
                .execution_condition
                .map(|condition| condition.kind)
            {
                Some(RecoveryExecutionConditionKind::NonConvergenceReplanRequired) => {
                    Ok(RecoveryOperationLegality::Allowed { operation })
                }
                Some(RecoveryExecutionConditionKind::Other)
                | Some(RecoveryExecutionConditionKind::NonConvergenceReplanAcknowledged) => Ok(
                    rejected(RecoveryOperationRejection::UnsupportedExecutionCondition),
                ),
                None => Ok(rejected(RecoveryOperationRejection::NoExecutionCondition)),
            }
        }
    }
}

fn observation_from_detail(detail: &TaskOperationsDetail) -> RecoveryObservation {
    let dependencies = detail
        .queue
        .as_ref()
        .map(|queue| {
            queue
                .dependencies
                .iter()
                .filter(|dependency| !dependency.is_done)
                .take(MAX_RECOVERY_ITEMS)
                .map(|dependency| RecoveryDependency {
                    task_id: bounded_text(&dependency.task_id),
                    status: dependency.status,
                    is_done: dependency.is_done,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let execution_condition =
        detail
            .execution_condition
            .as_ref()
            .map(|condition| RecoveryExecutionCondition {
                kind: match condition.kind.as_str() {
                    "non_convergence_replan_required" => {
                        RecoveryExecutionConditionKind::NonConvergenceReplanRequired
                    }
                    "non_convergence_replan_acknowledged" => {
                        RecoveryExecutionConditionKind::NonConvergenceReplanAcknowledged
                    }
                    _ => RecoveryExecutionConditionKind::Other,
                },
                details_present: !condition.details.is_empty(),
            });
    let validation = RecoveryValidationFacts {
        state: detail.summary.validation.state,
        failure_classification: detail.summary.validation.failure_classification,
        is_current: detail.summary.validation.is_current,
    };
    let review = RecoveryReviewFacts {
        run_id: detail.summary.review.run_id,
        verdict: detail.summary.review.verdict.as_deref().map(review_verdict),
        applies_to_current_change: detail.summary.review.applies_to_current_change,
    };
    let revision =
        detail
            .revision_lineage
            .map(revision_lineage)
            .unwrap_or(RecoveryRevisionLineage {
                actionable_review_run_id: None,
                actionable_contract_id: None,
                contract_source_review_run_id: None,
            });
    let latest_execution = detail
        .executions
        .iter()
        .find(|execution| {
            matches!(
                execution.status.as_str(),
                "running" | "waiting_external" | "failed" | "cancelled"
            )
        })
        .or_else(|| detail.executions.first())
        .map(execution_facts);
    let blockers = detail
        .blockers
        .iter()
        .take(MAX_RECOVERY_ITEMS)
        .map(|blocker| RecoveryBlockerFacts {
            state: blocker_state(blocker.state),
            actionable: blocker.actionable,
        })
        .collect::<Vec<_>>();
    let agent_economy = agent_economy_facts(detail);
    let mut conditions = Vec::new();
    if detail.summary.lifecycle == TaskStatus::Blocked
        && revision.actionable_review_run_id.is_some()
    {
        conditions.push(RecoveryCondition::BlockedRevision);
    } else if detail.summary.lifecycle == TaskStatus::RevisionRequired
        || review.verdict == Some(RecoveryReviewVerdict::Revise)
    {
        conditions.push(RecoveryCondition::SemanticRevision);
    }
    if matches!(validation.state, ValidationState::InfrastructureFailure)
        || validation.failure_classification
            == Some(ValidationFailureClassification::Infrastructure)
    {
        conditions.push(RecoveryCondition::InfrastructureFailure);
    } else if matches!(
        validation.state,
        ValidationState::Failing | ValidationState::Stale
    ) {
        conditions.push(RecoveryCondition::ValidationFailure);
    }
    if !dependencies.is_empty() {
        conditions.push(RecoveryCondition::DependencyBlocked);
    }
    if agent_economy.candidate_count > 0 && agent_economy.eligible_count == 0 {
        if agent_economy.constraints.iter().all(is_economy_constraint) {
            conditions.push(RecoveryCondition::EconomyExhaustion);
        } else {
            conditions.push(RecoveryCondition::NoEligibleAgent);
        }
    } else if detail.queue.as_ref().is_some_and(|queue| {
        queue
            .blocking_reasons
            .iter()
            .any(|reason| matches!(reason, BlockingReason::NoEligibleAgent { .. }))
    }) {
        conditions.push(RecoveryCondition::NoEligibleAgent);
    }
    if let Some(condition) = execution_condition {
        match condition.kind {
            RecoveryExecutionConditionKind::NonConvergenceReplanRequired => {
                conditions.push(RecoveryCondition::NonConvergenceReplanRequired)
            }
            RecoveryExecutionConditionKind::NonConvergenceReplanAcknowledged => {}
            RecoveryExecutionConditionKind::Other => conditions.push(RecoveryCondition::Ambiguous),
        }
    }
    if latest_execution.is_some_and(|execution| {
        matches!(
            execution.status,
            RecoveryExecutionStatus::Failed | RecoveryExecutionStatus::Cancelled
        )
    }) {
        conditions.push(RecoveryCondition::ExecutionFailure);
    }
    if detail.summary.lifecycle == TaskStatus::Blocked && conditions.is_empty() {
        conditions.push(RecoveryCondition::Ambiguous);
    }
    conditions.sort_by_key(|condition| *condition as u8);
    conditions.dedup();
    let state = if conditions.contains(&RecoveryCondition::Ambiguous) {
        RecoveryObservationState::Ambiguous
    } else if conditions.is_empty() {
        RecoveryObservationState::Stable
    } else {
        RecoveryObservationState::Abnormal
    };
    RecoveryObservation {
        task_id: bounded_text(&detail.summary.task_id),
        state,
        lifecycle: detail.summary.lifecycle,
        queue_phase: detail.summary.phase,
        conditions,
        execution_condition,
        validation,
        review,
        revision,
        latest_execution,
        dependencies,
        blockers,
        agent_economy,
    }
}

fn revision_lineage(lineage: RevisionLineageSummary) -> RecoveryRevisionLineage {
    RecoveryRevisionLineage {
        actionable_review_run_id: lineage.actionable_review_run_id,
        actionable_contract_id: lineage.actionable_contract_id,
        contract_source_review_run_id: lineage.contract_source_review_run_id,
    }
}

fn review_verdict(value: &str) -> RecoveryReviewVerdict {
    if value.eq_ignore_ascii_case("pass") {
        RecoveryReviewVerdict::Pass
    } else if value.eq_ignore_ascii_case("revise") {
        RecoveryReviewVerdict::Revise
    } else if value.eq_ignore_ascii_case("reject") {
        RecoveryReviewVerdict::Reject
    } else {
        RecoveryReviewVerdict::Other
    }
}

fn execution_facts(execution: &crate::operations::ExecutionSummary) -> RecoveryExecutionFacts {
    RecoveryExecutionFacts {
        id: execution.id,
        class: match execution.execution_class.as_str() {
            "coder" | "implementation" => RecoveryExecutionClass::Implementation,
            "review" => RecoveryExecutionClass::Review,
            "revision" => RecoveryExecutionClass::Revision,
            _ => RecoveryExecutionClass::Other,
        },
        status: match execution.status.as_str() {
            "running" => RecoveryExecutionStatus::Running,
            "waiting_external" => RecoveryExecutionStatus::WaitingExternal,
            "completed" => RecoveryExecutionStatus::Completed,
            "failed" => RecoveryExecutionStatus::Failed,
            "cancelled" => RecoveryExecutionStatus::Cancelled,
            _ => RecoveryExecutionStatus::Other,
        },
        failure: if execution.failure_category.as_deref() == Some("infrastructure") {
            RecoveryExecutionFailure::Infrastructure
        } else if execution.failure_category.is_some() {
            RecoveryExecutionFailure::Execution
        } else {
            RecoveryExecutionFailure::None
        },
    }
}

fn blocker_state(state: BlockerState) -> RecoveryBlockerState {
    match state {
        BlockerState::New => RecoveryBlockerState::New,
        BlockerState::Unresolved => RecoveryBlockerState::Unresolved,
        BlockerState::Regressed => RecoveryBlockerState::Regressed,
        BlockerState::Resolved => RecoveryBlockerState::Resolved,
        BlockerState::Unknown => RecoveryBlockerState::Unknown,
    }
}

fn agent_economy_facts(detail: &TaskOperationsDetail) -> RecoveryAgentEconomyFacts {
    let Some(decision) = detail
        .queue
        .as_ref()
        .and_then(|queue| queue.schedule_decision.as_ref())
    else {
        return RecoveryAgentEconomyFacts {
            candidate_count: 0,
            eligible_count: 0,
            constraints: Vec::new(),
        };
    };
    let mut constraints = Vec::new();
    for candidate in &decision.candidates {
        if let CandidateStatus::Rejected(reason) = &candidate.status {
            let constraint = rejection_constraint(reason);
            if !constraints.contains(&constraint) {
                constraints.push(constraint);
            }
        }
    }
    RecoveryAgentEconomyFacts {
        candidate_count: decision.candidates.len(),
        eligible_count: decision
            .candidates
            .iter()
            .filter(|candidate| matches!(candidate.status, CandidateStatus::Eligible))
            .count(),
        constraints,
    }
}

fn rejection_constraint(reason: &RejectionReason) -> RecoveryAgentEconomyConstraint {
    match reason {
        RejectionReason::Disabled => RecoveryAgentEconomyConstraint::Disabled,
        RejectionReason::Unavailable { .. } => RecoveryAgentEconomyConstraint::Unavailable,
        RejectionReason::UnsupportedBackend { .. } => {
            RecoveryAgentEconomyConstraint::UnsupportedBackend
        }
        RejectionReason::UnsupportedMode { .. } => RecoveryAgentEconomyConstraint::UnsupportedMode,
        RejectionReason::MissingCapability { .. } => {
            RecoveryAgentEconomyConstraint::MissingCapability
        }
        RejectionReason::QuotaExhausted => RecoveryAgentEconomyConstraint::QuotaExhausted,
        RejectionReason::QuotaReserve { .. } => RecoveryAgentEconomyConstraint::QuotaReserve,
        RejectionReason::QuotaRefreshFailed { .. } => {
            RecoveryAgentEconomyConstraint::QuotaRefreshFailed
        }
        RejectionReason::Busy => RecoveryAgentEconomyConstraint::Busy,
        RejectionReason::ModeMismatch { .. } => RecoveryAgentEconomyConstraint::ModeMismatch,
        RejectionReason::UnsupportedAction { .. } => {
            RecoveryAgentEconomyConstraint::UnsupportedAction
        }
        RejectionReason::AgentConstraint { .. } => RecoveryAgentEconomyConstraint::AgentConstraint,
        RejectionReason::BelowEscalationTier { .. } => {
            RecoveryAgentEconomyConstraint::BelowEscalationTier
        }
    }
}

fn is_economy_constraint(constraint: &RecoveryAgentEconomyConstraint) -> bool {
    matches!(
        constraint,
        RecoveryAgentEconomyConstraint::QuotaExhausted
            | RecoveryAgentEconomyConstraint::QuotaReserve
            | RecoveryAgentEconomyConstraint::QuotaRefreshFailed
            | RecoveryAgentEconomyConstraint::BelowEscalationTier
    )
}

fn bounded_text(value: &str) -> String {
    let mut end = value.len().min(MAX_RECOVERY_TEXT_BYTES);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    use crate::automated::ReviewResult;
    use crate::registry::{self, AgentAction, AgentDefinition, ReasoningEffort};
    use crate::storage::{AgentRunExecution, Database};
    use crate::task::TaskPriority;
    use crate::validation::ValidationReport;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git failed: {:?}", args);
    }

    fn setup() -> (TempDir, Database, i64, String) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
        git(directory.path(), &["init", "."]);
        git(
            directory.path(),
            &["config", "user.email", "recovery@example.com"],
        );
        git(directory.path(), &["config", "user.name", "Recovery Test"]);
        std::fs::write(directory.path().join("README.md"), "base\n").unwrap();
        git(directory.path(), &["add", "README.md"]);
        git(directory.path(), &["commit", "-m", "base"]);
        let db = Database::init(directory.path().join(".orc/orc.db")).unwrap();
        let project = db.create_project("recovery").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "recovery facts",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        (directory, db, project, task)
    }

    fn agent(id: &str, quota: Option<i64>) -> AgentDefinition {
        AgentDefinition {
            id: id.into(),
            backend: "codex".into(),
            execution_mode: registry::AUTOMATED.into(),
            display_name: id.into(),
            enabled: true,
            priority: 1,
            capabilities: vec!["code".into(), "command_execution".into()],
            status: registry::AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: None,
            model: Some("test-model".into()),
            reasoning_effort: Some(ReasoningEffort::Low),
            config_metadata: None,
            quota_remaining_percent: quota,
            quota_reset_at: None,
            quota_checked_at: quota.map(|_| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    .to_string()
            }),
            quota_source: quota.map(|_| "test".into()),
            quota_limits: None,
            actions: vec![AgentAction::Code],
        }
    }

    fn review_output(verdict: &str) -> String {
        serde_json::to_string(&ReviewResult {
            verdict: verdict.into(),
            criterion_results: Vec::new(),
            findings: Vec::new(),
            blocking_findings: Vec::new(),
            non_blocking_findings: Vec::new(),
            severity: None,
            revision_feedback: Some("repair the implementation".into()),
            blockers: Vec::new(),
        })
        .unwrap()
    }

    fn create_run(db: &Database, project: i64, task: &str, class: &str, status: &str) -> i64 {
        let run = db
            .create_agent_run_with_execution(
                project,
                task,
                "agent",
                registry::AUTOMATED,
                AgentRunExecution {
                    class,
                    model: Some("test-model"),
                    effort: Some(ReasoningEffort::Low),
                    source: "recovery-test",
                },
            )
            .unwrap();
        db.update_agent_run_status(run, status, Some("failure evidence"))
            .unwrap();
        run
    }

    fn persist_infrastructure_validation(db: &Database, task: &str, run: i64) {
        let report =
            ValidationReport::infrastructure_failure("check", "validation host unavailable".into());
        db.record_lifecycle_event(
            "validation_result",
            Some(task),
            Some(run),
            Some("agent"),
            Some(&serde_json::to_string(&report).unwrap()),
        )
        .unwrap();
    }

    fn operation(
        inspection: &RecoveryInspection,
        expected: RecoveryOperation,
    ) -> RecoveryOperationLegality {
        inspection
            .operations
            .iter()
            .copied()
            .find(|result| match result {
                RecoveryOperationLegality::Allowed { operation }
                | RecoveryOperationLegality::Rejected { operation, .. } => *operation == expected,
            })
            .unwrap()
    }

    #[test]
    fn blocked_revision_preserves_actionable_lineage_and_disallows_generic_requeue() {
        let (directory, db, project, task) = setup();
        let review = create_run(&db, project, &task, "review", "running");
        db.update_agent_run_status(review, "completed", Some(&review_output("REVISE")))
            .unwrap();
        db.persist_revision_contract(&task, review, "{}").unwrap();
        db.update_task_status(&task, TaskStatus::Blocked).unwrap();
        let inspection =
            inspect_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();

        assert_eq!(
            inspection.observation.state,
            RecoveryObservationState::Abnormal
        );
        assert!(
            inspection
                .observation
                .conditions
                .contains(&RecoveryCondition::BlockedRevision)
        );
        assert!(
            inspection
                .observation
                .revision
                .actionable_review_run_id
                .is_some()
        );
        assert!(
            inspection
                .observation
                .revision
                .actionable_contract_id
                .is_some()
        );
        assert!(matches!(
            operation(&inspection, RecoveryOperation::ResumeRevision),
            RecoveryOperationLegality::Allowed { .. }
        ));
        assert!(matches!(
            operation(&inspection, RecoveryOperation::Requeue),
            RecoveryOperationLegality::Rejected {
                reason: RecoveryOperationRejection::RequeueWouldDiscardRevisionLineage,
                ..
            }
        ));
    }

    #[test]
    fn infrastructure_failure_is_distinct_and_does_not_become_semantic_revision() {
        let (directory, db, project, task) = setup();
        db.insert_agent(&agent("agent", Some(80))).unwrap();
        let run = create_run(&db, project, &task, "implementation", "failed");
        persist_infrastructure_validation(&db, &task, run);
        db.update_task_status(&task, TaskStatus::Blocked).unwrap();
        let observation =
            observe_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();

        assert!(
            observation
                .conditions
                .contains(&RecoveryCondition::InfrastructureFailure)
        );
        assert!(
            !observation
                .conditions
                .contains(&RecoveryCondition::SemanticRevision)
        );
        assert_eq!(
            observation.validation.state,
            ValidationState::InfrastructureFailure
        );
    }

    #[test]
    fn dependency_blocking_has_no_recovery_operation_that_bypasses_dependencies() {
        let (directory, db, project, task) = setup();
        let dependency = db
            .insert_task(
                project,
                "dependency",
                "dependency",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        db.add_task_dependency(&task, &dependency).unwrap();
        let inspection =
            inspect_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();

        assert!(
            inspection
                .observation
                .conditions
                .contains(&RecoveryCondition::DependencyBlocked)
        );
        assert!(
            inspection
                .observation
                .dependencies
                .iter()
                .any(|item| { item.task_id == dependency && !item.is_done })
        );
        assert!(inspection.operations.iter().all(|result| {
            !matches!(
                result,
                RecoveryOperationLegality::Allowed {
                    operation: RecoveryOperation::Requeue | RecoveryOperation::ResumeRevision
                }
            )
        }));
    }

    #[test]
    fn economy_exhaustion_is_distinct_from_generic_agent_ineligibility() {
        let (directory, db, _project, task) = setup();
        db.insert_agent(&agent("quota-agent", Some(0))).unwrap();
        let observation =
            observe_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();
        assert!(
            observation
                .conditions
                .contains(&RecoveryCondition::EconomyExhaustion)
        );
        assert!(
            !observation
                .conditions
                .contains(&RecoveryCondition::NoEligibleAgent)
        );
        assert!(
            observation
                .agent_economy
                .constraints
                .contains(&RecoveryAgentEconomyConstraint::QuotaExhausted)
        );
    }

    #[test]
    fn economy_classification_considers_eligible_candidates_beyond_display_bound() {
        let (directory, db, _project, task) = setup();
        for index in 0..16 {
            let mut unavailable = agent(&format!("unavailable-{index:02}"), Some(80));
            unavailable.status = "unavailable".into();
            db.insert_agent(&unavailable).unwrap();
        }
        db.insert_agent(&agent("eligible-after-bound", Some(80)))
            .unwrap();

        let observation =
            observe_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();

        assert_eq!(observation.agent_economy.candidate_count, 17);
        assert_eq!(observation.agent_economy.eligible_count, 1);
        assert!(
            !observation
                .conditions
                .contains(&RecoveryCondition::NoEligibleAgent)
        );
        assert!(
            !observation
                .conditions
                .contains(&RecoveryCondition::EconomyExhaustion)
        );
    }

    #[test]
    fn latest_relevant_execution_is_selected_beyond_display_bound() {
        let (directory, db, project, task) = setup();
        let failed = create_run(&db, project, &task, "implementation", "failed");
        for _ in 0..4 {
            create_run(&db, project, &task, "implementation", "completed");
        }

        let observation =
            observe_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();

        assert_eq!(
            observation.latest_execution.map(|execution| execution.id),
            Some(failed)
        );
        assert_eq!(
            observation
                .latest_execution
                .map(|execution| execution.status),
            Some(RecoveryExecutionStatus::Failed)
        );
        assert!(
            observation
                .conditions
                .contains(&RecoveryCondition::ExecutionFailure)
        );
    }

    #[test]
    fn ambiguous_blocked_state_exposes_no_safe_default_operation() {
        let (directory, db, _project, task) = setup();
        db.update_task_status(&task, TaskStatus::Blocked).unwrap();
        let inspection =
            inspect_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();

        assert_eq!(
            inspection.observation.state,
            RecoveryObservationState::Ambiguous
        );
        assert!(
            inspection
                .observation
                .conditions
                .contains(&RecoveryCondition::Ambiguous)
        );
        assert!(
            inspection
                .operations
                .iter()
                .all(|result| matches!(result, RecoveryOperationLegality::Rejected { .. }))
        );
    }

    #[test]
    fn non_convergence_operation_is_read_only_and_uses_the_canonical_gate() {
        let (directory, db, _project, task) = setup();
        db.set_task_execution_condition(&task, "non_convergence_replan_required", "{}")
            .unwrap();
        let before_task = db.get_task(&task).unwrap();
        let before_condition = db.get_task_execution_condition(&task).unwrap();
        let inspection =
            inspect_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();

        assert!(
            inspection
                .observation
                .conditions
                .contains(&RecoveryCondition::NonConvergenceReplanRequired)
        );
        assert!(matches!(
            operation(&inspection, RecoveryOperation::AcknowledgeNonConvergence),
            RecoveryOperationLegality::Allowed { .. }
        ));
        assert_eq!(db.get_task(&task).unwrap(), before_task);
        assert_eq!(
            db.get_task_execution_condition(&task).unwrap(),
            before_condition
        );
    }

    #[test]
    fn observation_and_legality_inspection_are_side_effect_free_and_bounded() {
        let (directory, db, _project, task) = setup();
        db.update_task_status(&task, TaskStatus::Blocked).unwrap();
        let before_task = db.get_task(&task).unwrap();
        let before_runs = db.list_agent_runs_for_task(&task).unwrap();
        let before_run_ids = before_runs.iter().map(|run| run.id).collect::<Vec<_>>();
        let before_condition = db.get_task_execution_condition(&task).unwrap();
        let inspection =
            inspect_recovery(&ProjectOperations::new(&db, directory.path()), &task).unwrap();
        let encoded = serde_json::to_vec(&inspection).unwrap();

        assert!(encoded.len() < 64 * 1024);
        assert_eq!(db.get_task(&task).unwrap(), before_task);
        let after_run_ids = db
            .list_agent_runs_for_task(&task)
            .unwrap()
            .iter()
            .map(|run| run.id)
            .collect::<Vec<_>>();
        assert_eq!(after_run_ids, before_run_ids);
        assert_eq!(
            db.get_task_execution_condition(&task).unwrap(),
            before_condition
        );
    }
}
