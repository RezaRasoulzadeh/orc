//! Trusted, supervised execution of an explicitly authorized recovery action.
//!
//! M04-002 may recommend an operation, but only this application-owned seam
//! can turn that recommendation into a one-shot authorization and execute it.
//! Every execution re-inspects the M04-001 legality boundary before delegating
//! to an existing canonical mutation.

use crate::agent;
use crate::app::OrcApp;
use crate::controller_actions::ControllerActionExecutionContext;
use crate::queue::QueueCategory;
use crate::recovery::{RecoveryOperation, RecoveryOperationLegality, RecoveryOperationRejection};
use crate::recovery_controller::{RecoveryRecommendationResult, RecoveryRecommendationValidation};
use crate::task::TaskStatus;
use crate::validation::ValidationRunner;
use crate::worker::Worker;
use serde::{Deserialize, Serialize};

const MAX_RECOVERY_EXECUTION_TASK_ID_BYTES: usize = 256;

/// A bounded recovery execution intent derived only from an actionable
/// M04-002 result. It is intentionally not deserializable and has no free-form
/// or execution-context fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryExecutionIntent {
    task_id: String,
    operation: RecoveryOperation,
}

impl RecoveryExecutionIntent {
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub const fn operation(&self) -> RecoveryOperation {
        self.operation
    }

    fn validate(&self) -> Result<(), RecoveryIntentRejection> {
        if self.task_id.trim().is_empty()
            || self.task_id.len() > MAX_RECOVERY_EXECUTION_TASK_ID_BYTES
        {
            return Err(RecoveryIntentRejection::InvalidTaskIdentity);
        }
        Ok(())
    }
}

/// Bounded outcome of deriving an execution intent. Only the exact actionable
/// validation result can produce `Proposed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryExecutionIntentProposal {
    Proposed { intent: RecoveryExecutionIntent },
    Rejected { reason: RecoveryIntentRejection },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryIntentRejection {
    NotActionable,
    ValidationOperationMismatch,
    InvalidTaskIdentity,
}

/// Derive a recovery intent from M04-002's typed actionable result. Rationale
/// and confidence are deliberately not read.
pub fn propose_recovery_execution(
    result: &RecoveryRecommendationResult,
) -> RecoveryExecutionIntentProposal {
    let RecoveryRecommendationValidation::Actionable { operation } = result.validation else {
        return RecoveryExecutionIntentProposal::Rejected {
            reason: RecoveryIntentRejection::NotActionable,
        };
    };
    if result.recommendation.decision.operation() != Some(operation) {
        return RecoveryExecutionIntentProposal::Rejected {
            reason: RecoveryIntentRejection::ValidationOperationMismatch,
        };
    }
    let intent = RecoveryExecutionIntent {
        task_id: result.inspection.observation.task_id.clone(),
        operation,
    };
    if intent.validate().is_err() {
        return RecoveryExecutionIntentProposal::Rejected {
            reason: RecoveryIntentRejection::InvalidTaskIdentity,
        };
    }
    RecoveryExecutionIntentProposal::Proposed { intent }
}

/// Opaque, non-serializable, non-cloneable, one-shot authorization. Its only
/// constructor is the trusted `OrcApp` method below, and execution consumes it
/// by value while matching both task and operation.
#[derive(Debug, PartialEq, Eq)]
pub struct RecoveryActionAuthorization {
    task_id: String,
    operation: RecoveryOperation,
}

/// Trusted native dependencies for ResumeRevision. No worker, validation
/// runner, model, effort, backend, or override enters model-owned recovery
/// request/intent types.
pub enum RecoveryExecutionContext<'a> {
    Requeue,
    ResumeRevision {
        agent_id: Option<String>,
        overrides: agent::RevisionExecutionOverrides,
        worker: Option<&'a dyn Worker>,
        validation_runner: &'a dyn ValidationRunner,
    },
    AcknowledgeNonConvergence,
}

impl<'a> RecoveryExecutionContext<'a> {
    pub const fn requeue() -> Self {
        Self::Requeue
    }

    pub fn resume_revision() -> Self {
        Self::ResumeRevision {
            agent_id: None,
            overrides: agent::RevisionExecutionOverrides::default(),
            worker: None,
            validation_runner: &crate::validation::SystemValidationRunner,
        }
    }

    pub fn resume_revision_with_worker(
        agent_id: impl Into<String>,
        worker: &'a dyn Worker,
        validation_runner: &'a dyn ValidationRunner,
    ) -> Self {
        Self::ResumeRevision {
            agent_id: Some(agent_id.into()),
            overrides: agent::RevisionExecutionOverrides::default(),
            worker: Some(worker),
            validation_runner,
        }
    }

    pub const fn acknowledge_non_convergence() -> Self {
        Self::AcknowledgeNonConvergence
    }

    fn matches(&self, operation: RecoveryOperation) -> bool {
        matches!(
            (self, operation),
            (Self::Requeue, RecoveryOperation::Requeue)
                | (
                    Self::ResumeRevision { .. },
                    RecoveryOperation::ResumeRevision
                )
                | (
                    Self::AcknowledgeNonConvergence,
                    RecoveryOperation::AcknowledgeNonConvergence
                )
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAuthorizationRejection {
    Missing,
    NotAuthorizedForIntent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryExecutionFailureStage {
    RequestValidation,
    FreshLegalityInspection,
    CanonicalMutation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryExecutionEvidence {
    pub lifecycle: Option<TaskStatus>,
    pub queue_phase: Option<QueueCategory>,
    pub execution_condition_present: bool,
    pub actionable_revision_lineage: bool,
}

/// Bounded result of one attempted supervised recovery execution. Provider
/// output, errors, paths, commands, SQL, credentials, handles and runtime
/// values are intentionally absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum RecoveryExecutionResult {
    AuthorizationRejected {
        task_id: String,
        operation: RecoveryOperation,
        reason: RecoveryAuthorizationRejection,
    },
    InvalidRequest {
        task_id: String,
        operation: RecoveryOperation,
        reason: RecoveryIntentRejection,
    },
    InvalidExecutionContext {
        task_id: String,
        operation: RecoveryOperation,
    },
    FreshLegalityRejected {
        task_id: String,
        operation: RecoveryOperation,
        legality: RecoveryOperationLegality,
    },
    ExecutionFailed {
        task_id: String,
        operation: RecoveryOperation,
        stage: RecoveryExecutionFailureStage,
    },
    Executed {
        task_id: String,
        operation: RecoveryOperation,
        evidence: RecoveryExecutionEvidence,
    },
}

impl OrcApp {
    /// Mint an authorization only from a trusted, non-deserializable intent.
    /// It performs no mutation and does not treat the earlier recommendation
    /// inspection as a durable legality grant.
    pub fn authorize_recovery_action(
        &self,
        intent: &RecoveryExecutionIntent,
    ) -> RecoveryActionAuthorization {
        RecoveryActionAuthorization {
            task_id: intent.task_id.clone(),
            operation: intent.operation,
        }
    }

    /// Consume one trusted authorization, freshly inspect M04-001 legality,
    /// and only then delegate to an existing canonical recovery mutation.
    pub fn execute_authorized_recovery(
        &self,
        intent: &RecoveryExecutionIntent,
        authorization: Option<RecoveryActionAuthorization>,
        context: RecoveryExecutionContext<'_>,
    ) -> RecoveryExecutionResult {
        let task_id = bounded_task_id(intent.task_id());
        let operation = intent.operation();
        if intent.validate().is_err() {
            return RecoveryExecutionResult::InvalidRequest {
                task_id,
                operation,
                reason: RecoveryIntentRejection::InvalidTaskIdentity,
            };
        }
        let Some(authorization) = authorization else {
            return RecoveryExecutionResult::AuthorizationRejected {
                task_id,
                operation,
                reason: RecoveryAuthorizationRejection::Missing,
            };
        };
        if authorization.task_id != intent.task_id()
            || authorization.operation != intent.operation()
        {
            return RecoveryExecutionResult::AuthorizationRejected {
                task_id,
                operation,
                reason: RecoveryAuthorizationRejection::NotAuthorizedForIntent,
            };
        }
        if !context.matches(operation) {
            return RecoveryExecutionResult::InvalidExecutionContext { task_id, operation };
        }

        // This is the only legality source used for execution. It is the last
        // read before entering the canonical mutation path.
        let inspection = match self.inspect_recovery(intent.task_id()) {
            Ok(inspection) => inspection,
            Err(_) => {
                return RecoveryExecutionResult::ExecutionFailed {
                    task_id,
                    operation,
                    stage: RecoveryExecutionFailureStage::FreshLegalityInspection,
                };
            }
        };
        let legality = inspection
            .operations
            .into_iter()
            .find(|legality| legality_operation(*legality) == operation)
            .unwrap_or(RecoveryOperationLegality::Rejected {
                operation,
                reason: RecoveryOperationRejection::CanonicalLegalityRejected,
            });
        if !matches!(
            legality,
            RecoveryOperationLegality::Allowed { operation: allowed } if allowed == operation
        ) {
            return RecoveryExecutionResult::FreshLegalityRejected {
                task_id,
                operation,
                legality,
            };
        }

        if self
            .execute_recovery_canonically(intent.task_id(), operation, context)
            .is_err()
        {
            return RecoveryExecutionResult::ExecutionFailed {
                task_id,
                operation,
                stage: RecoveryExecutionFailureStage::CanonicalMutation,
            };
        }
        RecoveryExecutionResult::Executed {
            task_id: intent.task_id().to_owned(),
            operation,
            evidence: self.recovery_execution_evidence(intent.task_id()),
        }
    }

    fn execute_recovery_canonically(
        &self,
        task_id: &str,
        operation: RecoveryOperation,
        context: RecoveryExecutionContext<'_>,
    ) -> anyhow::Result<()> {
        match (operation, context) {
            (RecoveryOperation::Requeue, RecoveryExecutionContext::Requeue) => {
                self.requeue(task_id)?;
            }
            (
                RecoveryOperation::ResumeRevision,
                RecoveryExecutionContext::ResumeRevision {
                    agent_id,
                    overrides,
                    worker,
                    validation_runner,
                },
            ) => {
                let context = ControllerActionExecutionContext::Revise {
                    agent_id,
                    overrides,
                    worker,
                    validation_runner,
                };
                self.execute_canonical_revision(task_id, context)?;
            }
            (
                RecoveryOperation::AcknowledgeNonConvergence,
                RecoveryExecutionContext::AcknowledgeNonConvergence,
            ) => {
                self.unblock_non_convergence(task_id)?;
            }
            _ => anyhow::bail!("recovery execution context does not match operation"),
        }
        Ok(())
    }

    fn recovery_execution_evidence(&self, task_id: &str) -> RecoveryExecutionEvidence {
        let Ok(Some(detail)) = self.task_operations(task_id) else {
            return RecoveryExecutionEvidence {
                lifecycle: None,
                queue_phase: None,
                execution_condition_present: false,
                actionable_revision_lineage: false,
            };
        };
        RecoveryExecutionEvidence {
            lifecycle: Some(detail.summary.lifecycle),
            queue_phase: Some(detail.summary.phase),
            execution_condition_present: detail.execution_condition.is_some(),
            actionable_revision_lineage: detail
                .revision_lineage
                .is_some_and(|lineage| lineage.actionable_review_run_id.is_some()),
        }
    }
}

fn legality_operation(legality: RecoveryOperationLegality) -> RecoveryOperation {
    match legality {
        RecoveryOperationLegality::Allowed { operation }
        | RecoveryOperationLegality::Rejected { operation, .. } => operation,
    }
}

fn bounded_task_id(task_id: &str) -> String {
    let mut end = task_id.len().min(MAX_RECOVERY_EXECUTION_TASK_ID_BYTES);
    while end > 0 && !task_id.is_char_boundary(end) {
        end -= 1;
    }
    task_id[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automated::ReviewResult;
    use crate::recovery::RecoveryInspection;
    use crate::recovery_controller::RecoveryRecommendation;
    use crate::registry::{self, AgentAction, AgentDefinition};
    use crate::storage::{AgentRunExecution, Database};
    use crate::task::TaskPriority;
    use crate::validation;
    use crate::worker::test_helpers::FakeWorker;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {:?} failed", args);
    }

    fn setup() -> (TempDir, Database, i64, String) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
        git(directory.path(), &["init", "."]);
        git(
            directory.path(),
            &["config", "user.email", "recovery-execution@example.com"],
        );
        git(
            directory.path(),
            &["config", "user.name", "Recovery Execution Test"],
        );
        std::fs::write(directory.path().join("README.md"), "base\n").unwrap();
        git(directory.path(), &["add", "README.md"]);
        git(directory.path(), &["commit", "-m", "base"]);
        let db = Database::init(directory.path().join(".orc/orc.db")).unwrap();
        let project = db.create_project("recovery-execution").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "recovery execution",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        (directory, db, project, task)
    }

    fn create_run(db: &Database, project: i64, task: &str, class: &str, status: &str) -> i64 {
        let run = db
            .create_agent_run_with_execution(
                project,
                task,
                "agent-a",
                registry::AUTOMATED,
                AgentRunExecution {
                    class,
                    model: Some("test-model"),
                    effort: Some(crate::registry::ReasoningEffort::Low),
                    source: "recovery-execution-test",
                },
            )
            .unwrap();
        db.update_agent_run_status(run, status, Some("failed evidence"))
            .unwrap();
        run
    }

    fn agent() -> AgentDefinition {
        AgentDefinition {
            id: "agent-a".into(),
            backend: "codex".into(),
            execution_mode: registry::AUTOMATED.into(),
            display_name: "Recovery test agent".into(),
            enabled: true,
            priority: 1,
            capabilities: vec!["code".into(), "command_execution".into()],
            status: registry::AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: None,
            model: Some("test-model".into()),
            reasoning_effort: Some(crate::registry::ReasoningEffort::Low),
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
            actions: vec![AgentAction::Code],
        }
    }

    fn revision_output() -> String {
        serde_json::to_string(&ReviewResult {
            verdict: "REVISE".into(),
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

    fn result(
        inspection: &RecoveryInspection,
        decision: crate::recovery_controller::RecoveryRecommendationDecision,
    ) -> RecoveryRecommendationResult {
        let recommendation = RecoveryRecommendation {
            decision,
            rationale: "trusted fixture rationale".into(),
            confidence: Some(0.5),
        };
        let validation =
            crate::recovery_controller::validate_recommendation(inspection, &recommendation);
        RecoveryRecommendationResult {
            inspection: inspection.clone(),
            recommendation,
            validation,
        }
    }

    fn intent(result: &RecoveryRecommendationResult) -> RecoveryExecutionIntent {
        match propose_recovery_execution(result) {
            RecoveryExecutionIntentProposal::Proposed { intent } => intent,
            RecoveryExecutionIntentProposal::Rejected { reason } => {
                panic!("expected actionable intent, got {reason:?}")
            }
        }
    }

    #[test]
    fn actionable_result_derives_only_bounded_typed_intent() {
        let scenarios = crate::recovery_controller::representative_recovery_scenarios();
        let result = result(
            &scenarios[0].inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::ResumeRevision,
        );
        let intent = intent(&result);
        assert_eq!(intent.task_id(), "task-blocked-revision");
        assert_eq!(intent.operation(), RecoveryOperation::ResumeRevision);
    }

    #[test]
    fn non_actionable_and_rejected_recommendations_cannot_derive_intent() {
        let scenarios = crate::recovery_controller::representative_recovery_scenarios();
        let operator = result(
            &scenarios[6].inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::OperatorDecision,
        );
        assert!(matches!(
            propose_recovery_execution(&operator),
            RecoveryExecutionIntentProposal::Rejected {
                reason: RecoveryIntentRejection::NotActionable
            }
        ));

        let illegal = result(
            &scenarios[0].inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::Requeue,
        );
        assert!(matches!(
            propose_recovery_execution(&illegal),
            RecoveryExecutionIntentProposal::Rejected {
                reason: RecoveryIntentRejection::NotActionable
            }
        ));
    }

    #[test]
    fn blocked_actionable_revision_cannot_be_generically_requeued() {
        let (directory, db, project, task) = setup();
        let review = db
            .create_agent_run_with_execution(
                project,
                &task,
                "agent-a",
                registry::AUTOMATED,
                AgentRunExecution {
                    class: "review",
                    model: Some("test-model"),
                    effort: Some(crate::registry::ReasoningEffort::Low),
                    source: "recovery-execution-test",
                },
            )
            .unwrap();
        db.update_agent_run_status(review, "completed", Some(&revision_output()))
            .unwrap();
        db.persist_revision_contract(&task, review, "{}").unwrap();
        db.update_task_status(&task, TaskStatus::Blocked).unwrap();
        let inspection = crate::recovery::inspect_recovery(
            &crate::operations::ProjectOperations::new(&db, directory.path()),
            &task,
        )
        .unwrap();
        let result = result(
            &inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::Requeue,
        );
        assert!(matches!(
            result.validation,
            crate::recovery_controller::RecoveryRecommendationValidation::Rejected { .. }
        ));
        assert!(matches!(
            propose_recovery_execution(&result),
            RecoveryExecutionIntentProposal::Rejected { .. }
        ));
        assert_eq!(
            db.get_task(&task).unwrap().unwrap().status,
            TaskStatus::Blocked
        );
    }

    #[test]
    fn missing_authorization_has_zero_mutation() {
        let (directory, db, project, task) = setup();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        create_run(&db, project, &task, "implementation", "failed");
        let inspection = crate::recovery::inspect_recovery(
            &crate::operations::ProjectOperations::new(&db, directory.path()),
            &task,
        )
        .unwrap();
        let intent = intent(&result(
            &inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::Requeue,
        ));
        let before = db.get_task(&task).unwrap();
        drop(db);
        let app = OrcApp::open(directory.path().join(".orc/orc.db"), directory.path()).unwrap();
        let output =
            app.execute_authorized_recovery(&intent, None, RecoveryExecutionContext::requeue());
        assert!(matches!(
            output,
            RecoveryExecutionResult::AuthorizationRejected {
                reason: RecoveryAuthorizationRejection::Missing,
                ..
            }
        ));
        assert_eq!(app.task(&task).unwrap(), before);
    }

    #[test]
    fn wrong_task_and_wrong_operation_authorization_have_zero_mutation() {
        let (directory, db, project, task) = setup();
        let other = db
            .insert_task(
                project,
                "other",
                "other recovery execution",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        for id in [&task, &other] {
            db.update_task_status(id, TaskStatus::Active).unwrap();
            create_run(&db, project, id, "implementation", "failed");
        }
        drop(db);
        let app = OrcApp::open(directory.path().join(".orc/orc.db"), directory.path()).unwrap();
        let first = app.inspect_recovery(&task).unwrap();
        let first_intent = intent(&result(
            &first,
            crate::recovery_controller::RecoveryRecommendationDecision::Requeue,
        ));
        let authorization = app.authorize_recovery_action(&first_intent);
        let wrong_task = RecoveryExecutionIntent {
            task_id: other.clone(),
            operation: RecoveryOperation::Requeue,
        };
        let before = app.task(&other).unwrap();
        let output = app.execute_authorized_recovery(
            &wrong_task,
            Some(authorization),
            RecoveryExecutionContext::requeue(),
        );
        assert!(matches!(
            output,
            RecoveryExecutionResult::AuthorizationRejected {
                reason: RecoveryAuthorizationRejection::NotAuthorizedForIntent,
                ..
            }
        ));
        assert_eq!(app.task(&other).unwrap(), before);

        let authorization = app.authorize_recovery_action(&first_intent);
        let wrong_operation = RecoveryExecutionIntent {
            task_id: task,
            operation: RecoveryOperation::AcknowledgeNonConvergence,
        };
        let before = app.task(wrong_operation.task_id()).unwrap();
        let output = app.execute_authorized_recovery(
            &wrong_operation,
            Some(authorization),
            RecoveryExecutionContext::acknowledge_non_convergence(),
        );
        assert!(matches!(
            output,
            RecoveryExecutionResult::AuthorizationRejected {
                reason: RecoveryAuthorizationRejection::NotAuthorizedForIntent,
                ..
            }
        ));
        assert_eq!(app.task(wrong_operation.task_id()).unwrap(), before);
    }

    #[test]
    fn stale_legality_is_rejected_before_canonical_mutation() {
        let (directory, db, project, task) = setup();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        create_run(&db, project, &task, "implementation", "failed");
        let inspection = crate::recovery::inspect_recovery(
            &crate::operations::ProjectOperations::new(&db, directory.path()),
            &task,
        )
        .unwrap();
        let intent = intent(&result(
            &inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::Requeue,
        ));
        drop(db);
        let app = OrcApp::open(directory.path().join(".orc/orc.db"), directory.path()).unwrap();
        let authorization = app.authorize_recovery_action(&intent);
        app.database()
            .update_task_status(&task, TaskStatus::Ready)
            .unwrap();
        let before = app.task(&task).unwrap();
        let output = app.execute_authorized_recovery(
            &intent,
            Some(authorization),
            RecoveryExecutionContext::requeue(),
        );
        assert!(matches!(
            output,
            RecoveryExecutionResult::FreshLegalityRejected { .. }
        ));
        assert_eq!(app.task(&task).unwrap(), before);
    }

    #[test]
    fn canonical_requeue_executes_and_reports_lifecycle_evidence() {
        let (directory, db, project, task) = setup();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        create_run(&db, project, &task, "implementation", "failed");
        drop(db);
        let app = OrcApp::open(directory.path().join(".orc/orc.db"), directory.path()).unwrap();
        let inspection = app.inspect_recovery(&task).unwrap();
        let intent = intent(&result(
            &inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::Requeue,
        ));
        let authorization = app.authorize_recovery_action(&intent);
        let output = app.execute_authorized_recovery(
            &intent,
            Some(authorization),
            RecoveryExecutionContext::requeue(),
        );
        assert!(matches!(
            output,
            RecoveryExecutionResult::Executed {
                evidence: RecoveryExecutionEvidence {
                    lifecycle: Some(TaskStatus::Backlog),
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn canonical_non_convergence_acknowledgement_executes() {
        let (directory, db, _project, task) = setup();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        db.set_task_execution_condition(
            &task,
            "non_convergence_replan_required",
            "{\"attempts\":3}",
        )
        .unwrap();
        drop(db);
        let app = OrcApp::open(directory.path().join(".orc/orc.db"), directory.path()).unwrap();
        let inspection = app.inspect_recovery(&task).unwrap();
        let intent = intent(&result(
            &inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::AcknowledgeNonConvergence,
        ));
        let authorization = app.authorize_recovery_action(&intent);
        let output = app.execute_authorized_recovery(
            &intent,
            Some(authorization),
            RecoveryExecutionContext::acknowledge_non_convergence(),
        );
        assert!(matches!(output, RecoveryExecutionResult::Executed { .. }));
        assert_eq!(
            app.database()
                .get_task_execution_condition(&task)
                .unwrap()
                .unwrap()
                .kind,
            "non_convergence_replan_acknowledged"
        );
    }

    #[test]
    fn canonical_resume_revision_executes_through_existing_revision_path() {
        let (directory, db, project, task) = setup();
        std::fs::write(directory.path().join(".orc/engineering.md"), "# contract\n").unwrap();
        let (branch, worktree) = crate::git::ensure_worktree(&task, directory.path()).unwrap();
        let implementation = create_run(&db, project, &task, "coder", "completed");
        db.store_worktree_metadata(implementation, &task, &branch, &worktree.to_string_lossy())
            .unwrap();
        let review = db
            .create_agent_run_with_execution(
                project,
                &task,
                "agent-a",
                registry::AUTOMATED,
                AgentRunExecution {
                    class: "review",
                    model: Some("test-model"),
                    effort: Some(crate::registry::ReasoningEffort::Low),
                    source: "recovery-execution-test",
                },
            )
            .unwrap();
        db.update_agent_run_status(review, "completed", Some(&revision_output()))
            .unwrap();
        db.update_task_status(&task, TaskStatus::RevisionRequired)
            .unwrap();
        db.insert_agent(&agent()).unwrap();
        drop(db);
        let app = OrcApp::open(directory.path().join(".orc/orc.db"), directory.path()).unwrap();
        let inspection = app.inspect_recovery(&task).unwrap();
        let intent = intent(&result(
            &inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::ResumeRevision,
        ));
        let authorization = app.authorize_recovery_action(&intent);
        let validation = validation::test_helpers::FakeValidationRunner::success();
        let worker = FakeWorker::new_success(None);
        let output = app.execute_authorized_recovery(
            &intent,
            Some(authorization),
            RecoveryExecutionContext::resume_revision_with_worker("agent-a", &worker, &validation),
        );
        assert!(matches!(
            output,
            RecoveryExecutionResult::Executed {
                evidence: RecoveryExecutionEvidence {
                    lifecycle: Some(TaskStatus::Review),
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn authorization_is_consumed_by_value_and_cannot_be_replayed() {
        let (directory, db, project, task) = setup();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        create_run(&db, project, &task, "implementation", "failed");
        drop(db);
        let app = OrcApp::open(directory.path().join(".orc/orc.db"), directory.path()).unwrap();
        let inspection = app.inspect_recovery(&task).unwrap();
        let intent = intent(&result(
            &inspection,
            crate::recovery_controller::RecoveryRecommendationDecision::Requeue,
        ));
        let authorization = app.authorize_recovery_action(&intent);
        let first = app.execute_authorized_recovery(
            &intent,
            Some(authorization),
            RecoveryExecutionContext::requeue(),
        );
        assert!(matches!(first, RecoveryExecutionResult::Executed { .. }));
        let replay =
            app.execute_authorized_recovery(&intent, None, RecoveryExecutionContext::requeue());
        assert!(matches!(
            replay,
            RecoveryExecutionResult::AuthorizationRejected {
                reason: RecoveryAuthorizationRejection::Missing,
                ..
            }
        ));
    }
}
