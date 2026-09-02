//! Typed, read-only Controller action intents and kernel legality inspection.
//!
//! Controller code can propose one of the small high-level intents below, but
//! it cannot provide commands, persistence handles or execution arguments.
//! Legality is delegated to [`ProjectOperations`], which owns the canonical
//! queue, lifecycle and evidence projections.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::agent;
use crate::app::OrcApp;
use crate::automated::{ActionBackend, ActionOverrides};
use crate::controller::ControllerRecommendation;
use crate::operations::{OperationalAction, OperationalNextStep, ProjectOperations};
use crate::registry::ReasoningEffort;
use crate::validation::ValidationRunner;
use crate::worker::Worker;

pub use crate::operations::{
    OperationalAction as ControllerActionKind,
    OperationalActionLegality as ControllerActionLegality,
    OperationalActionObservation as ControllerActionObservation,
    OperationalActionRejection as ControllerActionRejection,
};

const MAX_CONTROLLER_ACTION_TASK_ID_BYTES: usize = 256;

/// The only action proposals currently inspectable at the Controller boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerActionIntent {
    Dispatch { task_id: String },
    SemanticReview { task_id: String },
    Revise { task_id: String },
    Accept { task_id: String },
}

/// The bounded result of translating one typed Controller recommendation into
/// an action proposal. A proposal is not an authorization and carries no
/// execution context or mutation capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ControllerActionProposal {
    Proposed {
        intent: ControllerActionIntent,
    },
    Unsupported {
        next_step: Option<OperationalNextStep>,
    },
    Invalid {
        reason: ControllerActionProposalRejection,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerActionProposalRejection {
    InvalidTaskId,
}

/// Deterministically map only the four supported typed recommendations to
/// typed action intents. Rationale, confidence, structured JSON and response
/// text are deliberately not consulted.
pub fn propose_controller_action(
    recommendation: &ControllerRecommendation,
) -> ControllerActionProposal {
    let Some(next_step) = recommendation.suggested_next_step else {
        return ControllerActionProposal::Unsupported { next_step: None };
    };
    let intent = match next_step {
        OperationalNextStep::Dispatch => ControllerActionIntent::Dispatch {
            task_id: recommendation.task_id.clone(),
        },
        OperationalNextStep::RunSemanticReview => ControllerActionIntent::SemanticReview {
            task_id: recommendation.task_id.clone(),
        },
        OperationalNextStep::Revise => ControllerActionIntent::Revise {
            task_id: recommendation.task_id.clone(),
        },
        OperationalNextStep::Accept => ControllerActionIntent::Accept {
            task_id: recommendation.task_id.clone(),
        },
        unsupported => {
            return ControllerActionProposal::Unsupported {
                next_step: Some(unsupported),
            };
        }
    };
    if intent.validate().is_err() {
        return ControllerActionProposal::Invalid {
            reason: ControllerActionProposalRejection::InvalidTaskId,
        };
    }
    ControllerActionProposal::Proposed { intent }
}

#[derive(Debug, Error)]
pub enum ControllerActionError {
    #[error(
        "controller action task ID must be non-empty and at most {MAX_CONTROLLER_ACTION_TASK_ID_BYTES} bytes"
    )]
    InvalidTaskId,
    #[error("controller action legality read failed: {0}")]
    Read(#[source] anyhow::Error),
}

/// A one-shot authorization minted by trusted Orc/application code.
///
/// This deliberately has no serde implementation, no public constructor and
/// is consumed by the execution boundary. The intent fingerprint prevents a
/// token authorized for one action from being replayed for another action.
#[derive(Debug, PartialEq, Eq)]
pub struct ControllerActionAuthorization {
    action: OperationalAction,
    task_id: String,
}

/// Native execution dependencies and operator/application configuration. This
/// type is intentionally not serializable: it is not part of the
/// model-owned Controller contract.
pub enum ControllerActionExecutionContext<'a> {
    Dispatch {
        agent_id: Option<String>,
        model_override: Option<String>,
        effort_override: Option<ReasoningEffort>,
        worker: Option<&'a dyn Worker>,
        validation_runner: &'a dyn ValidationRunner,
    },
    SemanticReview {
        overrides: ActionOverrides,
        backend: &'a dyn ActionBackend,
        validation_runner: &'a dyn ValidationRunner,
    },
    Revise {
        agent_id: Option<String>,
        overrides: agent::RevisionExecutionOverrides,
        worker: Option<&'a dyn Worker>,
        validation_runner: &'a dyn ValidationRunner,
    },
    Accept,
}

impl<'a> ControllerActionExecutionContext<'a> {
    /// Use the configured Orc dispatch path. Optional agent/model/effort
    /// values are trusted application/operator overrides, not model fields.
    pub fn dispatch(
        agent_id: Option<String>,
        model_override: Option<String>,
        effort_override: Option<ReasoningEffort>,
    ) -> Self {
        Self::Dispatch {
            agent_id,
            model_override,
            effort_override,
            worker: None,
            validation_runner: &crate::validation::SystemValidationRunner,
        }
    }

    /// Use the canonical worker-backed dispatch path with an injected worker
    /// for deterministic tests or an explicitly trusted application seam.
    pub fn dispatch_with_worker(
        agent_id: impl Into<String>,
        worker: &'a dyn Worker,
        validation_runner: &'a dyn ValidationRunner,
    ) -> Self {
        Self::Dispatch {
            agent_id: Some(agent_id.into()),
            model_override: None,
            effort_override: None,
            worker: Some(worker),
            validation_runner,
        }
    }

    pub fn semantic_review(
        overrides: ActionOverrides,
        backend: &'a dyn ActionBackend,
        validation_runner: &'a dyn ValidationRunner,
    ) -> Self {
        Self::SemanticReview {
            overrides,
            backend,
            validation_runner,
        }
    }

    /// Use Orc's configured previous implementation-agent selection.
    pub fn revise() -> Self {
        Self::Revise {
            agent_id: None,
            overrides: agent::RevisionExecutionOverrides::default(),
            worker: None,
            validation_runner: &crate::validation::SystemValidationRunner,
        }
    }

    /// Use the canonical revision worker path with explicitly trusted agent,
    /// worker, and validation seams.
    pub fn revise_with_worker(
        agent_id: impl Into<String>,
        worker: &'a dyn Worker,
        validation_runner: &'a dyn ValidationRunner,
    ) -> Self {
        Self::Revise {
            agent_id: Some(agent_id.into()),
            overrides: agent::RevisionExecutionOverrides::default(),
            worker: Some(worker),
            validation_runner,
        }
    }

    pub const fn accept() -> Self {
        Self::Accept
    }

    fn matches(&self, action: OperationalAction) -> bool {
        matches!(
            (self, action),
            (Self::Dispatch { .. }, OperationalAction::Dispatch)
                | (
                    Self::SemanticReview { .. },
                    OperationalAction::SemanticReview
                )
                | (Self::Revise { .. }, OperationalAction::Revise)
                | (Self::Accept, OperationalAction::Accept)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerActionAuthorizationRejection {
    Missing,
    NotAuthorizedForIntent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerActionExecutionStage {
    RequestValidation,
    ExecutionContext,
    LegalityInspection,
    CanonicalMutation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerActionExecutionEvidence {
    pub lifecycle: Option<crate::task::TaskStatus>,
    pub run_id: Option<i64>,
    pub review_run_id: Option<i64>,
    pub validation_state: Option<crate::operations::ValidationState>,
}

/// Bounded result of one attempted Controller action execution. Provider
/// output, filesystem paths, SQL, handles and runtime objects are excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ControllerActionExecutionResult {
    AuthorizationRejected {
        action: OperationalAction,
        task_id: String,
        reason: ControllerActionAuthorizationRejection,
    },
    FreshLegalityRejected {
        legality: ControllerActionLegality,
    },
    Executed {
        action: OperationalAction,
        task_id: String,
        evidence: ControllerActionExecutionEvidence,
    },
    ExecutionFailed {
        action: OperationalAction,
        task_id: String,
        stage: ControllerActionExecutionStage,
    },
}

impl ControllerActionIntent {
    pub fn action_kind(&self) -> ControllerActionKind {
        match self {
            Self::Dispatch { .. } => OperationalAction::Dispatch,
            Self::SemanticReview { .. } => OperationalAction::SemanticReview,
            Self::Revise { .. } => OperationalAction::Revise,
            Self::Accept { .. } => OperationalAction::Accept,
        }
    }

    pub fn task_id(&self) -> &str {
        match self {
            Self::Dispatch { task_id }
            | Self::SemanticReview { task_id }
            | Self::Revise { task_id }
            | Self::Accept { task_id } => task_id,
        }
    }

    pub fn validate(&self) -> std::result::Result<(), ControllerActionError> {
        let task_id = self.task_id();
        if task_id.trim().is_empty() || task_id.len() > MAX_CONTROLLER_ACTION_TASK_ID_BYTES {
            return Err(ControllerActionError::InvalidTaskId);
        }
        Ok(())
    }

    /// Ask the deterministic kernel whether this intent is legal. This method
    /// only reads canonical state and never executes or persists the intent.
    pub fn inspect(
        &self,
        operations: &ProjectOperations<'_>,
    ) -> std::result::Result<ControllerActionLegality, ControllerActionError> {
        self.validate()?;
        operations
            .inspect_action(self.task_id(), self.action_kind())
            .map_err(ControllerActionError::Read)
    }
}

impl OrcApp {
    /// Mint a one-shot authorization in trusted application code. This method
    /// intentionally does not inspect or persist a grant; execution always
    /// performs its own mutation-boundary legality check.
    pub fn authorize_controller_action(
        &self,
        intent: &ControllerActionIntent,
    ) -> std::result::Result<ControllerActionAuthorization, ControllerActionError> {
        intent.validate()?;
        Ok(ControllerActionAuthorization {
            action: intent.action_kind(),
            task_id: intent.task_id().to_owned(),
        })
    }

    /// Execute one explicitly authorized Controller intent.
    ///
    /// Authorization is consumed, matched to the exact requested intent, and
    /// followed by a fresh canonical legality inspection immediately before
    /// delegating to the existing Orc mutation implementation. A missing or
    /// mismatched authorization cannot reach the mutation path.
    pub fn execute_authorized_controller_action(
        &self,
        intent: &ControllerActionIntent,
        authorization: Option<ControllerActionAuthorization>,
        context: ControllerActionExecutionContext<'_>,
    ) -> ControllerActionExecutionResult {
        let action = intent.action_kind();
        let task_id = bounded_controller_task_id(intent.task_id());
        if intent.validate().is_err() {
            return ControllerActionExecutionResult::ExecutionFailed {
                action,
                task_id,
                stage: ControllerActionExecutionStage::RequestValidation,
            };
        }
        let Some(authorization) = authorization else {
            return ControllerActionExecutionResult::AuthorizationRejected {
                action,
                task_id,
                reason: ControllerActionAuthorizationRejection::Missing,
            };
        };
        if authorization.action != action || authorization.task_id != intent.task_id() {
            return ControllerActionExecutionResult::AuthorizationRejected {
                action,
                task_id,
                reason: ControllerActionAuthorizationRejection::NotAuthorizedForIntent,
            };
        }
        if !context.matches(action) {
            return ControllerActionExecutionResult::ExecutionFailed {
                action,
                task_id,
                stage: ControllerActionExecutionStage::ExecutionContext,
            };
        }

        // This is deliberately the last read in the boundary before the
        // canonical mutation call. Earlier Allowed observations are never
        // consulted and never act as a capability.
        let legality = match intent.inspect(&self.operations()) {
            Ok(legality) => legality,
            Err(_) => {
                return ControllerActionExecutionResult::ExecutionFailed {
                    action,
                    task_id,
                    stage: ControllerActionExecutionStage::LegalityInspection,
                };
            }
        };
        if matches!(legality, ControllerActionLegality::Rejected { .. }) {
            return ControllerActionExecutionResult::FreshLegalityRejected { legality };
        }

        let run_id = match self.execute_controller_action_canonically(intent, context) {
            Ok(run_id) => run_id,
            Err(_) => {
                return ControllerActionExecutionResult::ExecutionFailed {
                    action,
                    task_id,
                    stage: ControllerActionExecutionStage::CanonicalMutation,
                };
            }
        };

        ControllerActionExecutionResult::Executed {
            action,
            task_id,
            evidence: self.controller_execution_evidence(intent.task_id(), run_id),
        }
    }

    fn execute_controller_action_canonically(
        &self,
        intent: &ControllerActionIntent,
        context: ControllerActionExecutionContext<'_>,
    ) -> anyhow::Result<Option<i64>> {
        match (intent.action_kind(), context) {
            (
                OperationalAction::Dispatch,
                ControllerActionExecutionContext::Dispatch {
                    agent_id,
                    model_override,
                    effort_override,
                    worker,
                    validation_runner,
                },
            ) => {
                let summary = match worker {
                    Some(worker) => agent::dispatch_with_worker_on_db(
                        intent.task_id(),
                        worker,
                        self.database(),
                        self.repo_path(),
                        agent_id
                            .as_deref()
                            .context("worker-backed dispatch requires an agent")?,
                        validation_runner,
                    )?,
                    None => agent::dispatch_selected_with_db_and_repo(
                        self.database(),
                        self.repo_path(),
                        intent.task_id(),
                        agent_id.as_deref(),
                        model_override,
                        effort_override,
                    )?,
                };
                Ok(Some(summary.run_id))
            }
            (
                OperationalAction::SemanticReview,
                ControllerActionExecutionContext::SemanticReview {
                    overrides,
                    backend,
                    validation_runner,
                },
            ) => Ok(Some(
                self.automated_review_with_backend(
                    intent.task_id(),
                    &overrides,
                    backend,
                    validation_runner,
                )?
                .0,
            )),
            (
                OperationalAction::Revise,
                ControllerActionExecutionContext::Revise {
                    agent_id,
                    overrides,
                    worker,
                    validation_runner,
                },
            ) => {
                // The revision feedback is canonical persisted review
                // evidence, not a model-owned execution argument.
                let feedback = self
                    .actionable_revision_feedback(intent.task_id())?
                    .context("actionable revision review disappeared")?;
                let summary = match worker {
                    Some(worker) => agent::revise_with_worker_on_db_with_overrides(
                        intent.task_id(),
                        &feedback,
                        worker,
                        self.database(),
                        self.repo_path(),
                        agent_id
                            .as_deref()
                            .context("worker-backed revision requires an agent")?,
                        validation_runner,
                        &overrides,
                    )?,
                    None => {
                        if overrides.model.is_some() || overrides.effort.is_some() {
                            anyhow::bail!(
                                "revision overrides require the worker-backed canonical seam"
                            );
                        }
                        match agent_id {
                            Some(agent_id) => {
                                self.revise(intent.task_id(), &feedback, &agent_id)?;
                            }
                            None => {
                                self.revise_with_previous_agent(intent.task_id(), &feedback)?;
                            }
                        }
                        return Ok(None);
                    }
                };
                Ok(Some(summary.run_id))
            }
            (OperationalAction::Accept, ControllerActionExecutionContext::Accept) => {
                self.accept(intent.task_id())?;
                Ok(None)
            }
            _ => anyhow::bail!("execution context does not match Controller action"),
        }
    }

    /// Shared trusted canonical revision seam for M03 normal actions and M04
    /// recovery. Recovery keeps its own intent and authorization boundary;
    /// revision lifecycle logic remains centralized here.
    pub(crate) fn execute_canonical_revision(
        &self,
        task_id: &str,
        context: ControllerActionExecutionContext<'_>,
    ) -> anyhow::Result<Option<i64>> {
        let intent = ControllerActionIntent::Revise {
            task_id: task_id.to_owned(),
        };
        self.execute_controller_action_canonically(&intent, context)
    }

    fn controller_execution_evidence(
        &self,
        task_id: &str,
        run_id: Option<i64>,
    ) -> ControllerActionExecutionEvidence {
        let Ok(Some(detail)) = self.task_operations(task_id) else {
            return ControllerActionExecutionEvidence {
                lifecycle: None,
                run_id,
                review_run_id: None,
                validation_state: None,
            };
        };
        ControllerActionExecutionEvidence {
            lifecycle: Some(detail.summary.lifecycle),
            run_id,
            review_run_id: detail.summary.review.run_id,
            validation_state: Some(detail.summary.validation.state),
        }
    }
}

fn bounded_controller_task_id(task_id: &str) -> String {
    task_id
        .chars()
        .take(MAX_CONTROLLER_ACTION_TASK_ID_BYTES)
        .collect()
}

/// Convenience function for callers that prefer a free inspection boundary.
pub fn inspect_action(
    intent: &ControllerActionIntent,
    operations: &ProjectOperations<'_>,
) -> std::result::Result<ControllerActionLegality, ControllerActionError> {
    intent.inspect(operations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    use crate::automated::ReviewResult;
    use crate::automated::{ActionBackend, ActionExecution, ActionOverrides};
    use crate::controller::ControllerRecommendation;
    use crate::local_runtime::{
        LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
    };
    use crate::operations::ProjectOperations;
    use crate::registry::{self, AgentAction, AgentDefinition, ReasoningEffort};
    use crate::storage::{AgentRunExecution, Database};
    use crate::task::{CreateTaskInput, TaskPriority, TaskStatus};
    use crate::validation::test_helpers::FakeValidationRunner;
    use crate::validation::{ValidationCategory, ValidationReport, ValidationStepResult};
    use crate::worker::TokenUsage;
    use crate::worker::test_helpers::FakeWorker;
    use tempfile::TempDir;

    struct FakeControllerRuntime {
        response: LocalInferenceResponse,
    }

    impl LocalInferenceRuntime for FakeControllerRuntime {
        fn infer(
            &mut self,
            _request: &LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            Ok(self.response.clone())
        }
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
        run_git(directory.path(), &["init", "."]);
        run_git(
            directory.path(),
            &["config", "user.email", "controller-actions@example.com"],
        );
        run_git(
            directory.path(),
            &["config", "user.name", "Controller Actions Test"],
        );
        std::fs::write(directory.path().join("README.md"), "base\n").unwrap();
        run_git(directory.path(), &["add", "README.md"]);
        run_git(directory.path(), &["commit", "-m", "base"]);
        directory
    }

    fn agent() -> AgentDefinition {
        AgentDefinition {
            id: "agent-a".into(),
            backend: "codex".into(),
            execution_mode: registry::AUTOMATED.into(),
            display_name: "Test agent".into(),
            enabled: true,
            priority: 1,
            capabilities: vec!["code".into(), "command_execution".into(), "review".into()],
            status: registry::AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: None,
            model: Some("test-model".into()),
            reasoning_effort: Some(ReasoningEffort::Low),
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
            actions: vec![AgentAction::Code, AgentAction::Review],
        }
    }

    fn setup() -> (TempDir, Database, i64, String) {
        let repo = repository();
        let db = Database::init(repo.path().join(".orc/orc.db")).unwrap();
        let project = db.create_project("controller-actions").unwrap();
        db.insert_agent(&agent()).unwrap();
        let task = db
            .create_task(
                project,
                &CreateTaskInput {
                    title: "Controller action task".into(),
                    objective: "Inspect action legality".into(),
                    role: "developer".into(),
                    priority: TaskPriority::Normal,
                    required_capabilities: Vec::new(),
                    scope_mode: None,
                    context_files: Vec::new(),
                    expected_changes: Vec::new(),
                    dependencies: Vec::new(),
                },
            )
            .unwrap();
        (repo, db, project, task)
    }

    fn create_run(db: &Database, project: i64, task: &str, class: &str) -> i64 {
        db.create_agent_run_with_execution(
            project,
            task,
            "agent-a",
            registry::AUTOMATED,
            AgentRunExecution {
                class,
                model: Some("test-model"),
                effort: Some(ReasoningEffort::Low),
                source: "controller-actions-test",
            },
        )
        .unwrap()
    }

    fn review_output(verdict: &str) -> String {
        serde_json::to_string(&ReviewResult {
            verdict: verdict.into(),
            criterion_results: Vec::new(),
            findings: Vec::new(),
            blocking_findings: Vec::new(),
            non_blocking_findings: Vec::new(),
            severity: None,
            revision_feedback: Some("test feedback".into()),
            blockers: Vec::new(),
        })
        .unwrap()
    }

    fn passing_validation() -> ValidationReport {
        ValidationReport {
            steps: vec![ValidationStepResult {
                command: "cargo test".into(),
                category: ValidationCategory::Success,
                passed: true,
                stdout: String::new(),
                stderr: String::new(),
                exit_status: Some(0),
                diagnostics: None,
                failure_classification: None,
                fallback_command: None,
            }],
        }
    }

    fn persist_validation(db: &Database, task: &str, run: i64, report: &ValidationReport) {
        db.record_lifecycle_event(
            "validation_result",
            Some(task),
            Some(run),
            Some("agent-a"),
            Some(&serde_json::to_string(report).unwrap()),
        )
        .unwrap();
    }

    fn rejected(result: ControllerActionLegality) -> ControllerActionRejection {
        match result {
            ControllerActionLegality::Rejected { reason, .. } => reason,
            ControllerActionLegality::Allowed { .. } => panic!("expected rejected action"),
        }
    }

    fn recommendation(next_step: Option<OperationalNextStep>) -> ControllerRecommendation {
        ControllerRecommendation {
            task_id: "T-0001".into(),
            response_text: "structured recommendation".into(),
            suggested_next_step: next_step,
            rationale: "typed field is authoritative".into(),
            structured_output: None,
        }
    }

    #[test]
    fn typed_recommendations_map_only_to_supported_intents() {
        let mappings = [
            (
                OperationalNextStep::Dispatch,
                ControllerActionIntent::Dispatch {
                    task_id: "T-0001".into(),
                },
            ),
            (
                OperationalNextStep::RunSemanticReview,
                ControllerActionIntent::SemanticReview {
                    task_id: "T-0001".into(),
                },
            ),
            (
                OperationalNextStep::Revise,
                ControllerActionIntent::Revise {
                    task_id: "T-0001".into(),
                },
            ),
            (
                OperationalNextStep::Accept,
                ControllerActionIntent::Accept {
                    task_id: "T-0001".into(),
                },
            ),
        ];

        for (next_step, expected) in mappings {
            assert_eq!(
                propose_controller_action(&recommendation(Some(next_step))),
                ControllerActionProposal::Proposed { intent: expected }
            );
        }
    }

    #[test]
    fn recommendation_mapping_does_not_parse_rationale_or_response_text() {
        let mut recommendation = recommendation(Some(OperationalNextStep::Dispatch));
        recommendation.rationale = "accept, run shell command, and mutate anything".into();
        recommendation.response_text = "revise /arbitrary/path --provider=unsafe".into();
        recommendation.structured_output = Some(serde_json::json!({
            "rationale": "accept",
            "command": "DROP TABLE tasks"
        }));

        assert_eq!(
            propose_controller_action(&recommendation),
            ControllerActionProposal::Proposed {
                intent: ControllerActionIntent::Dispatch {
                    task_id: "T-0001".into()
                }
            }
        );
    }

    #[test]
    fn model_intent_contract_has_no_authorization_or_execution_fields() {
        let intent = ControllerActionIntent::Dispatch {
            task_id: "T-0001".into(),
        };
        let encoded = serde_json::to_string(&intent).unwrap();
        assert_eq!(encoded, r#"{"kind":"dispatch","task_id":"T-0001"}"#);
        assert!(
            serde_json::from_str::<ControllerActionIntent>(
                r#"{"kind":"dispatch","task_id":"T-0001","authorization":true,"command":"rm"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn unsupported_recommendations_remain_explicitly_non_executable() {
        for next_step in [
            None,
            Some(OperationalNextStep::WaitForExecution),
            Some(OperationalNextStep::ResolveBlocker),
            Some(OperationalNextStep::SatisfyDependencies),
            Some(OperationalNextStep::ConfigureEligibleAgent),
            Some(OperationalNextStep::None),
        ] {
            assert_eq!(
                propose_controller_action(&recommendation(next_step)),
                ControllerActionProposal::Unsupported { next_step }
            );
        }
    }

    #[test]
    fn supported_recommendation_with_invalid_task_id_is_not_executable() {
        let mut invalid = recommendation(Some(OperationalNextStep::Dispatch));
        invalid.task_id = "x".repeat(MAX_CONTROLLER_ACTION_TASK_ID_BYTES + 1);
        assert_eq!(
            propose_controller_action(&invalid),
            ControllerActionProposal::Invalid {
                reason: ControllerActionProposalRejection::InvalidTaskId
            }
        );
    }

    #[test]
    fn supervised_recommendation_path_is_read_only_until_trusted_authorization() {
        let (repo, db, _project, task) = setup();
        std::fs::write(repo.path().join(".orc/engineering.md"), "# contract\n").unwrap();
        db.update_task_status(&task, TaskStatus::Ready).unwrap();
        drop(db);
        let app = OrcApp::open(repo.path().join(".orc/orc.db"), repo.path()).unwrap();
        let mut runtime = FakeControllerRuntime {
            response: LocalInferenceResponse::structured(
                "ignored rationale",
                serde_json::json!({
                    "suggested_next_step": "dispatch",
                    "decision_class": "action",
                    "rationale": "dispatch is structurally recommended",
                    "confidence": 1.0
                }),
            ),
        };

        let proposal = app
            .propose_controller_action(&task, &mut runtime)
            .expect("proposal");
        let intent = match proposal {
            ControllerActionProposal::Proposed { intent } => intent,
            other => panic!("expected executable proposal, got {other:?}"),
        };
        assert_eq!(app.task(&task).unwrap().unwrap().status, TaskStatus::Ready);
        assert!(
            app.database()
                .list_agent_runs_for_task(&task)
                .unwrap()
                .is_empty()
        );

        let worker = FakeWorker::new_success(None);
        let validation = FakeValidationRunner::success();
        let unauthorized = app.execute_authorized_controller_action(
            &intent,
            None,
            ControllerActionExecutionContext::dispatch_with_worker("agent-a", &worker, &validation),
        );
        assert!(matches!(
            unauthorized,
            ControllerActionExecutionResult::AuthorizationRejected {
                reason: ControllerActionAuthorizationRejection::Missing,
                ..
            }
        ));
        assert_eq!(app.task(&task).unwrap().unwrap().status, TaskStatus::Ready);
        assert!(
            app.database()
                .list_agent_runs_for_task(&task)
                .unwrap()
                .is_empty()
        );

        let authorization = app
            .authorize_controller_action(&intent)
            .expect("trusted authorization");
        let executed = app.execute_authorized_controller_action(
            &intent,
            Some(authorization),
            ControllerActionExecutionContext::dispatch_with_worker("agent-a", &worker, &validation),
        );
        assert!(matches!(
            executed,
            ControllerActionExecutionResult::Executed {
                evidence: ControllerActionExecutionEvidence {
                    lifecycle: Some(TaskStatus::Review),
                    run_id: Some(_),
                    ..
                },
                ..
            }
        ));
        assert_eq!(app.task(&task).unwrap().unwrap().status, TaskStatus::Review);
        assert_eq!(
            app.database()
                .list_agent_runs_for_task(&task)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn action_intents_are_serializable_and_typed() {
        let intents = [
            ControllerActionIntent::Dispatch {
                task_id: "T-0001".into(),
            },
            ControllerActionIntent::SemanticReview {
                task_id: "T-0001".into(),
            },
            ControllerActionIntent::Revise {
                task_id: "T-0001".into(),
            },
            ControllerActionIntent::Accept {
                task_id: "T-0001".into(),
            },
        ];
        for intent in intents {
            let encoded = serde_json::to_string(&intent).unwrap();
            let decoded: ControllerActionIntent = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, intent);
            assert_eq!(decoded.task_id(), "T-0001");
            assert!(decoded.validate().is_ok());
        }
    }

    #[test]
    fn action_intent_rejects_unbounded_or_blank_task_ids() {
        let blank = ControllerActionIntent::Dispatch {
            task_id: "   ".into(),
        };
        assert!(matches!(
            blank.validate(),
            Err(ControllerActionError::InvalidTaskId)
        ));
        let oversized = ControllerActionIntent::Accept {
            task_id: "x".repeat(MAX_CONTROLLER_ACTION_TASK_ID_BYTES + 1),
        };
        assert!(matches!(
            oversized.validate(),
            Err(ControllerActionError::InvalidTaskId)
        ));
        assert!(
            serde_json::from_str::<ControllerActionIntent>(
                r#"{"kind":"dispatch","task_id":"T-0001","command":"rm -rf"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn dispatch_legality_uses_canonical_queue_and_dependencies() {
        let (repo, db, _project, task) = setup();
        db.update_task_status(&task, TaskStatus::Ready).unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        let dispatch = ControllerActionIntent::Dispatch {
            task_id: task.clone(),
        }
        .inspect(&operations)
        .unwrap();
        assert!(matches!(dispatch, ControllerActionLegality::Allowed { .. }));
        let encoded = serde_json::to_string(&dispatch).unwrap();
        let decoded: ControllerActionLegality = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, dispatch);

        let (repo, db, project, dependent) = setup();
        let dependency = db
            .create_task(
                project,
                &CreateTaskInput {
                    title: "Dependency".into(),
                    objective: "Dependency".into(),
                    role: "developer".into(),
                    priority: TaskPriority::Normal,
                    required_capabilities: Vec::new(),
                    scope_mode: None,
                    context_files: Vec::new(),
                    expected_changes: Vec::new(),
                    dependencies: Vec::new(),
                },
            )
            .unwrap();
        db.add_task_dependency(&dependent, &dependency).unwrap();
        db.update_task_status(&dependent, TaskStatus::Ready)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        let blocked = ControllerActionIntent::Dispatch { task_id: dependent }
            .inspect(&operations)
            .unwrap();
        let encoded = serde_json::to_string(&blocked).unwrap();
        let decoded: ControllerActionLegality = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, blocked);
        assert!(matches!(
            rejected(blocked),
            ControllerActionRejection::DependenciesIncomplete { .. }
        ));

        let (repo, db, _project, no_agent) = setup();
        db.set_task_required_capabilities(&no_agent, &["unavailable-capability".into()])
            .unwrap();
        db.update_task_status(&no_agent, TaskStatus::Ready).unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::Dispatch { task_id: no_agent }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::NoEligibleAgent
        ));
    }

    #[test]
    fn semantic_review_legality_requires_current_passing_validation() {
        let (repo, db, project, task) = setup();
        db.update_task_status(&task, TaskStatus::Review).unwrap();
        let run = create_run(&db, project, &task, "coder");
        db.update_agent_run_status(run, "completed", Some("implementation"))
            .unwrap();
        persist_validation(&db, &task, run, &passing_validation());
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            ControllerActionIntent::SemanticReview {
                task_id: task.clone()
            }
            .inspect(&operations)
            .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));

        let (repo, db, project, missing) = setup();
        db.update_task_status(&missing, TaskStatus::Review).unwrap();
        let run = create_run(&db, project, &missing, "coder");
        db.store_worktree_metadata(run, &missing, "branch", ".")
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::SemanticReview { task_id: missing }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ValidationMissing
        ));

        let (repo, db, project, stale) = setup();
        db.update_task_status(&stale, TaskStatus::Review).unwrap();
        let run = create_run(&db, project, &stale, "coder");
        db.update_agent_run_status(run, "completed", Some("implementation"))
            .unwrap();
        db.store_worktree_metadata(run, &stale, "branch", ".")
            .unwrap();
        persist_validation(&db, &stale, run, &passing_validation());
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::SemanticReview { task_id: stale }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ValidationStale
        ));

        let (repo, db, project, active) = setup();
        db.update_task_status(&active, TaskStatus::Active).unwrap();
        let run = create_run(&db, project, &active, "coder");
        db.update_agent_run_status(run, "completed", Some("implementation"))
            .unwrap();
        persist_validation(&db, &active, run, &passing_validation());
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            ControllerActionIntent::SemanticReview { task_id: active }
                .inspect(&operations)
                .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));

        let (repo, db, _project, terminal) = setup();
        db.update_task_status(&terminal, TaskStatus::Done).unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::SemanticReview { task_id: terminal }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::WrongLifecycle { .. }
        ));
    }

    #[test]
    fn revise_legality_requires_actionable_review_and_no_condition() {
        let (repo, db, project, task) = setup();
        db.update_task_status(&task, TaskStatus::RevisionRequired)
            .unwrap();
        let review = create_run(&db, project, &task, "review");
        db.update_agent_run_status(review, "completed", Some(&review_output("REVISE")))
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            ControllerActionIntent::Revise {
                task_id: task.clone()
            }
            .inspect(&operations)
            .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));
        db.set_task_execution_condition(&task, "operator_gate", "needs decision")
            .unwrap();
        assert!(matches!(
            rejected(
                ControllerActionIntent::Revise { task_id: task }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ExecutionConditionPresent
        ));

        let (repo, db, _project, missing) = setup();
        db.update_task_status(&missing, TaskStatus::RevisionRequired)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::Revise { task_id: missing }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::RevisionEvidenceMissing
        ));
    }

    #[test]
    fn accept_legality_requires_current_pass_review_evidence() {
        let (repo, db, project, task) = setup();
        let implementation = create_run(&db, project, &task, "coder");
        db.store_worktree_metadata(implementation, &task, "branch", ".")
            .unwrap();
        std::fs::write(repo.path().join("accepted.txt"), "change\n").unwrap();
        let changes = crate::git::inspect_worktree(repo.path(), repo.path()).unwrap();
        db.update_agent_run_status(implementation, "completed", Some("implementation"))
            .unwrap();
        let review = create_run(&db, project, &task, "review");
        db.update_agent_run_status(review, "completed", Some(&review_output("PASS")))
            .unwrap();
        db.store_change_evidence(review, &changes).unwrap();
        db.update_task_status(&task, TaskStatus::AcceptanceReady)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            ControllerActionIntent::Accept {
                task_id: task.clone()
            }
            .inspect(&operations)
            .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));

        let (repo, db, _project, missing) = setup();
        db.update_task_status(&missing, TaskStatus::AcceptanceReady)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::Accept { task_id: missing }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ReviewMissing
        ));

        let (repo, db, project, stale) = setup();
        let review = create_run(&db, project, &stale, "review");
        db.update_agent_run_status(review, "completed", Some(&review_output("PASS")))
            .unwrap();
        db.update_task_status(&stale, TaskStatus::AcceptanceReady)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::Accept { task_id: stale }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ReviewStale
        ));
    }

    #[test]
    fn action_inspection_is_side_effect_free() {
        let (repo, db, project, task) = setup();
        let implementation = create_run(&db, project, &task, "coder");
        db.store_worktree_metadata(implementation, &task, "branch", ".")
            .unwrap();
        std::fs::write(repo.path().join("unchanged.txt"), "change\n").unwrap();
        let changes = crate::git::inspect_worktree(repo.path(), repo.path()).unwrap();
        db.update_agent_run_status(implementation, "completed", Some("implementation"))
            .unwrap();
        let review = create_run(&db, project, &task, "review");
        db.update_agent_run_status(review, "completed", Some(&review_output("PASS")))
            .unwrap();
        db.store_change_evidence(review, &changes).unwrap();
        db.update_task_status(&task, TaskStatus::AcceptanceReady)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        let before_detail = operations.task_detail(&task).unwrap();
        let before_task = db.get_task(&task).unwrap();
        let before_runs =
            serde_json::to_value(db.list_agent_runs_for_task(&task).unwrap()).unwrap();
        let before_worktree = db.get_worktree_metadata(&task).unwrap();
        let before_evidence = db.get_change_evidence(review).unwrap();

        let _ = ControllerActionIntent::Accept {
            task_id: task.clone(),
        }
        .inspect(&operations)
        .unwrap();

        assert_eq!(operations.task_detail(&task).unwrap(), before_detail);
        assert_eq!(db.get_task(&task).unwrap(), before_task);
        assert_eq!(
            serde_json::to_value(db.list_agent_runs_for_task(&task).unwrap()).unwrap(),
            before_runs
        );
        assert_eq!(db.get_worktree_metadata(&task).unwrap(), before_worktree);
        assert_eq!(db.get_change_evidence(review).unwrap(), before_evidence);
    }

    #[test]
    fn execution_requires_one_shot_trusted_authorization_and_does_not_self_authorize() {
        let (repo, db, _project, task) = setup();
        db.update_task_status(&task, TaskStatus::Ready).unwrap();
        let intent = ControllerActionIntent::Dispatch {
            task_id: task.clone(),
        };
        let before_runs = db.list_agent_runs_for_task(&task).unwrap();
        drop(db);
        let app = OrcApp::open(repo.path().join(".orc/orc.db"), repo.path()).unwrap();

        let missing = app.execute_authorized_controller_action(
            &intent,
            None,
            ControllerActionExecutionContext::dispatch_with_worker(
                "agent-a",
                &FakeWorker::new_success(None),
                &FakeValidationRunner::success(),
            ),
        );
        assert!(matches!(
            missing,
            ControllerActionExecutionResult::AuthorizationRejected {
                reason: ControllerActionAuthorizationRejection::Missing,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(app.database().list_agent_runs_for_task(&task).unwrap()).unwrap(),
            serde_json::to_value(before_runs).unwrap()
        );
        assert_eq!(app.task(&task).unwrap().unwrap().status, TaskStatus::Ready);

        let authorization = app.authorize_controller_action(&intent).unwrap();
        let different_intent = ControllerActionIntent::Accept {
            task_id: task.clone(),
        };
        let replay = app.execute_authorized_controller_action(
            &different_intent,
            Some(authorization),
            ControllerActionExecutionContext::accept(),
        );
        assert!(matches!(
            replay,
            ControllerActionExecutionResult::AuthorizationRejected {
                reason: ControllerActionAuthorizationRejection::NotAuthorizedForIntent,
                ..
            }
        ));
        assert_eq!(app.task(&task).unwrap().unwrap().status, TaskStatus::Ready);
    }

    #[test]
    fn previously_allowed_intent_is_rechecked_and_rejected_without_mutation() {
        let (repo, db, _project, task) = setup();
        db.update_task_status(&task, TaskStatus::Ready).unwrap();
        let intent = ControllerActionIntent::Dispatch {
            task_id: task.clone(),
        };
        assert!(matches!(
            intent
                .inspect(&ProjectOperations::new(&db, repo.path()))
                .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));
        drop(db);
        let app = OrcApp::open(repo.path().join(".orc/orc.db"), repo.path()).unwrap();
        let authorization = app.authorize_controller_action(&intent).unwrap();
        app.database()
            .update_task_status(&task, TaskStatus::Active)
            .unwrap();
        let before_runs = app.database().list_agent_runs_for_task(&task).unwrap();
        let result = app.execute_authorized_controller_action(
            &intent,
            Some(authorization),
            ControllerActionExecutionContext::dispatch_with_worker(
                "agent-a",
                &FakeWorker::new_success(None),
                &FakeValidationRunner::success(),
            ),
        );
        assert!(matches!(
            result,
            ControllerActionExecutionResult::FreshLegalityRejected {
                legality: ControllerActionLegality::Rejected {
                    reason: ControllerActionRejection::WrongPhase { .. },
                    ..
                }
            }
        ));
        assert_eq!(
            serde_json::to_value(app.database().list_agent_runs_for_task(&task).unwrap()).unwrap(),
            serde_json::to_value(before_runs).unwrap()
        );
        assert_eq!(app.task(&task).unwrap().unwrap().status, TaskStatus::Active);
    }

    struct ControllerReviewBackend {
        output: String,
    }

    impl ActionBackend for ControllerReviewBackend {
        fn invoke(
            &self,
            _agent: &AgentDefinition,
            action: AgentAction,
            _input: &str,
            _model: Option<&str>,
            _effort: Option<ReasoningEffort>,
        ) -> anyhow::Result<ActionExecution> {
            assert_eq!(action, AgentAction::Review);
            Ok(ActionExecution {
                output: self.output.clone(),
                token_usage: Some(TokenUsage {
                    total_tokens: 1,
                    input_tokens: Some(1),
                    output_tokens: Some(1),
                    cached_input_tokens: None,
                }),
            })
        }
    }

    fn revising_review_output() -> String {
        serde_json::json!({
            "verdict": "REVISE",
            "criterion_results": [{
                "criterion_id": "acceptance-criterion-1",
                "status": "insufficient_evidence",
                "evidence": [{
                    "kind": "task_contract",
                    "reference": "task_contract.objective",
                    "explanation": "The objective needs implementation evidence."
                }],
                "rationale": "Implementation evidence is still required."
            }],
            "findings": ["implementation evidence is required"],
            "blocking_findings": ["implementation evidence is required"],
            "non_blocking_findings": [],
            "severity": "medium",
            "revision_feedback": "provide implementation evidence",
            "blockers": []
        })
        .to_string()
    }

    #[test]
    fn all_authorized_actions_use_canonical_lifecycle_paths() {
        // Dispatch: the injected Worker is only a trusted test seam; all
        // lifecycle and persistence changes come from agent dispatch.
        let (repo, db, _project, dispatch_task) = setup();
        std::fs::write(repo.path().join(".orc/engineering.md"), "# contract\n").unwrap();
        db.update_task_status(&dispatch_task, TaskStatus::Ready)
            .unwrap();
        drop(db);
        let app = OrcApp::open(repo.path().join(".orc/orc.db"), repo.path()).unwrap();
        let dispatch = ControllerActionIntent::Dispatch {
            task_id: dispatch_task.clone(),
        };
        let dispatch_auth = app.authorize_controller_action(&dispatch).unwrap();
        let dispatch_result = app.execute_authorized_controller_action(
            &dispatch,
            Some(dispatch_auth),
            ControllerActionExecutionContext::dispatch_with_worker(
                "agent-a",
                &FakeWorker::new_success(None),
                &FakeValidationRunner::success(),
            ),
        );
        assert!(matches!(
            dispatch_result,
            ControllerActionExecutionResult::Executed {
                evidence: ControllerActionExecutionEvidence {
                    lifecycle: Some(TaskStatus::Review),
                    run_id: Some(_),
                    ..
                },
                ..
            }
        ));

        // Semantic Review: the backend is injected through the trusted
        // application context and run_review owns the review transaction.
        let (repo, db, _project, review_task) = setup();
        db.update_task_status(&review_task, TaskStatus::Review)
            .unwrap();
        drop(db);
        let app = OrcApp::open(repo.path().join(".orc/orc.db"), repo.path()).unwrap();
        let review = ControllerActionIntent::SemanticReview {
            task_id: review_task.clone(),
        };
        let review_auth = app.authorize_controller_action(&review).unwrap();
        let review_result = app.execute_authorized_controller_action(
            &review,
            Some(review_auth),
            ControllerActionExecutionContext::semantic_review(
                ActionOverrides::default(),
                &ControllerReviewBackend {
                    output: revising_review_output(),
                },
                &FakeValidationRunner::success(),
            ),
        );
        assert!(matches!(
            review_result,
            ControllerActionExecutionResult::Executed {
                evidence: ControllerActionExecutionEvidence {
                    lifecycle: Some(TaskStatus::RevisionRequired),
                    review_run_id: Some(_),
                    ..
                },
                ..
            }
        ));

        // Revise: create canonical actionable review evidence and let the
        // existing worker-backed revision path consume it.
        let (repo, db, project, revise_task) = setup();
        std::fs::write(repo.path().join(".orc/engineering.md"), "# contract\n").unwrap();
        let (branch, worktree) = crate::git::ensure_worktree(&revise_task, repo.path()).unwrap();
        let implementation = create_run(&db, project, &revise_task, "coder");
        db.store_worktree_metadata(
            implementation,
            &revise_task,
            &branch,
            &worktree.to_string_lossy(),
        )
        .unwrap();
        db.update_agent_run_status(implementation, "completed", Some("implementation"))
            .unwrap();
        let review_run = create_run(&db, project, &revise_task, "review");
        db.update_agent_run_status(review_run, "completed", Some(&review_output("REVISE")))
            .unwrap();
        db.update_task_status(&revise_task, TaskStatus::RevisionRequired)
            .unwrap();
        drop(db);
        let app = OrcApp::open(repo.path().join(".orc/orc.db"), repo.path()).unwrap();
        let revise = ControllerActionIntent::Revise {
            task_id: revise_task.clone(),
        };
        let revise_auth = app.authorize_controller_action(&revise).unwrap();
        let revise_result = app.execute_authorized_controller_action(
            &revise,
            Some(revise_auth),
            ControllerActionExecutionContext::revise_with_worker(
                "agent-a",
                &FakeWorker::new_success(None),
                &FakeValidationRunner::success(),
            ),
        );
        assert!(matches!(
            revise_result,
            ControllerActionExecutionResult::Executed {
                evidence: ControllerActionExecutionEvidence {
                    lifecycle: Some(TaskStatus::Review),
                    run_id: Some(_),
                    ..
                },
                ..
            }
        ));

        // Accept: use the canonical worktree/evidence/merge path.
        let (repo, db, project, accept_task) = setup();
        let (branch, worktree) = crate::git::ensure_worktree(&accept_task, repo.path()).unwrap();
        let worktree_absolute = repo.path().join(&worktree);
        std::fs::write(worktree_absolute.join("accepted.txt"), "accepted\n").unwrap();
        let changes = crate::git::inspect_worktree(&worktree_absolute, repo.path()).unwrap();
        let implementation = create_run(&db, project, &accept_task, "coder");
        db.store_worktree_metadata(
            implementation,
            &accept_task,
            &branch,
            &worktree.to_string_lossy(),
        )
        .unwrap();
        db.update_agent_run_status(implementation, "completed", Some("implementation"))
            .unwrap();
        let review_run = create_run(&db, project, &accept_task, "review");
        db.update_agent_run_status(review_run, "completed", Some(&review_output("PASS")))
            .unwrap();
        db.store_change_evidence(review_run, &changes).unwrap();
        db.update_task_status(&accept_task, TaskStatus::AcceptanceReady)
            .unwrap();
        drop(db);
        let app = OrcApp::open(repo.path().join(".orc/orc.db"), repo.path()).unwrap();
        let accept = ControllerActionIntent::Accept {
            task_id: accept_task.clone(),
        };
        let accept_auth = app.authorize_controller_action(&accept).unwrap();
        let accept_result = app.execute_authorized_controller_action(
            &accept,
            Some(accept_auth),
            ControllerActionExecutionContext::accept(),
        );
        assert!(matches!(
            accept_result,
            ControllerActionExecutionResult::Executed {
                evidence: ControllerActionExecutionEvidence {
                    lifecycle: Some(TaskStatus::Done),
                    ..
                },
                ..
            }
        ));
        assert!(!worktree.exists());
    }
}
