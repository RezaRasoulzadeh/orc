//! Deterministic read-only Controller decision evaluation.
//!
//! This module provides a small, bounded scenario corpus and evaluates typed
//! recommendations through [`ControllerStateBuilder`]. It owns no lifecycle
//! or storage mutation path: scenarios are serialized
//! [`ControllerStatePacket`] fixtures and evaluation only observes runtime
//! responses.

use crate::controller::{
    ControllerActivityEvent, ControllerBlockerSummary, ControllerContractSummary,
    ControllerDependency, ControllerEconomySummary, ControllerError, ControllerEvidenceRef,
    ControllerExecutionSummary, ControllerRecommendation, ControllerReviewCriterion,
    ControllerReviewSummary, ControllerSelfHostingState, ControllerStateBuilder,
    ControllerStatePacket, ControllerTaskState, ControllerTaskSummary, ControllerValidationSummary,
};
use crate::local_runtime::{LocalInferenceError, LocalInferenceRuntime};
use crate::operations::{BlockerState, OperationalNextStep, ValidationState};
use crate::protocol::{ReviewCriterionStatus, ReviewEvidenceKind};
use crate::queue::QueueCategory;
use crate::registry::ReasoningEffort;
use crate::self_hosting::SelfHostingReadinessState;
use crate::task::{TaskPriority, TaskStatus};
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Maximum number of scenarios accepted by one evaluation run.
pub const MAX_EVALUATION_SCENARIOS: usize = 16;

const MAX_SCENARIO_TEXT_BYTES: usize = 512;
const MAX_ACCEPTABLE_ALTERNATIVES: usize = 8;

/// Failures constructing or running the evaluation harness.
#[derive(Debug, Error)]
pub enum ControllerEvaluationError {
    #[error("evaluation contains {actual} scenarios; maximum is {max}")]
    TooManyScenarios { actual: usize, max: usize },
    #[error("evaluation scenario metadata exceeds its {field} bound")]
    ScenarioBounds { field: String },
    #[error("evaluation scenario `{scenario_id}` has an invalid packet: {source}")]
    InvalidPacket {
        scenario_id: String,
        #[source]
        source: ControllerError,
    },
}

/// Typed semantic classes used by the scenario corpus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedActionClass {
    Dispatch,
    Accept,
    Revise,
    PreserveRevisionLineage,
    AvoidSemanticRevision,
    SatisfyDependencies,
    OperatorDecision,
}

impl ExpectedActionClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::Accept => "accept",
            Self::Revise => "revise",
            Self::PreserveRevisionLineage => "preserve_revision_lineage",
            Self::AvoidSemanticRevision => "avoid_semantic_revision",
            Self::SatisfyDependencies => "satisfy_dependencies",
            Self::OperatorDecision => "operator_decision",
        }
    }
}

/// Typed semantic interpretation of a Controller recommendation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "next_step")]
pub enum ControllerDecision {
    NextStep(OperationalNextStep),
    OperatorDecision,
    Unspecified,
}

impl ControllerDecision {
    /// Interpret only typed recommendation fields, never natural-language text.
    pub fn from_recommendation(recommendation: &ControllerRecommendation) -> Self {
        if let Some(next_step) = recommendation.suggested_next_step {
            return Self::NextStep(next_step);
        }
        let Some(value) = recommendation.structured_output.as_ref() else {
            return Self::Unspecified;
        };
        let Some(decision_class) = value.get("decision_class") else {
            return Self::Unspecified;
        };
        match serde_json::from_value::<StructuredDecisionClass>(decision_class.clone()) {
            Ok(StructuredDecisionClass::OperatorDecision) => Self::OperatorDecision,
            Ok(StructuredDecisionClass::Action) | Err(_) => Self::Unspecified,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NextStep(next_step) => match next_step {
                OperationalNextStep::Dispatch => "dispatch",
                OperationalNextStep::WaitForExecution => "wait_for_execution",
                OperationalNextStep::RunSemanticReview => "run_semantic_review",
                OperationalNextStep::Revise => "revise",
                OperationalNextStep::Accept => "accept",
                OperationalNextStep::ResolveBlocker => "resolve_blocker",
                OperationalNextStep::SatisfyDependencies => "satisfy_dependencies",
                OperationalNextStep::ConfigureEligibleAgent => "configure_eligible_agent",
                OperationalNextStep::None => "none",
            },
            Self::OperatorDecision => "operator_decision",
            Self::Unspecified => "unspecified",
        }
    }
}

impl fmt::Display for ControllerDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StructuredDecisionClass {
    Action,
    OperatorDecision,
}

/// One bounded input state and its typed policy expectation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControllerEvaluationScenario {
    pub id: String,
    pub description: String,
    pub packet: ControllerStatePacket,
    pub expected_action_class: ExpectedActionClass,
    pub expected_decision: ControllerDecision,
    pub acceptable_alternatives: Vec<ControllerDecision>,
}

impl ControllerEvaluationScenario {
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        packet: ControllerStatePacket,
        expected_action_class: ExpectedActionClass,
        expected_decision: ControllerDecision,
        acceptable_alternatives: Vec<ControllerDecision>,
    ) -> Result<Self, ControllerEvaluationError> {
        let id = id.into();
        let description = description.into();
        if id.is_empty() || id.len() > MAX_SCENARIO_TEXT_BYTES {
            return Err(ControllerEvaluationError::ScenarioBounds { field: "id".into() });
        }
        if description.len() > MAX_SCENARIO_TEXT_BYTES {
            return Err(ControllerEvaluationError::ScenarioBounds {
                field: "description".into(),
            });
        }
        if acceptable_alternatives.len() > MAX_ACCEPTABLE_ALTERNATIVES {
            return Err(ControllerEvaluationError::ScenarioBounds {
                field: "acceptable_alternatives".into(),
            });
        }
        packet
            .validate()
            .map_err(|source| ControllerEvaluationError::InvalidPacket {
                scenario_id: id.clone(),
                source,
            })?;
        Ok(Self {
            id,
            description,
            packet,
            expected_action_class,
            expected_decision,
            acceptable_alternatives,
        })
    }
}

/// Result for one evaluated scenario.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioResult {
    Pass,
    Fail,
}

/// Evidence retained when an evaluation-only structured-output parser fails.
///
/// This is deliberately scoped to the evaluation harness. It is not part of
/// the model-independent runtime response contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerParseDiagnostic {
    pub raw_model_output: String,
    pub parse_error: String,
}

impl ControllerParseDiagnostic {
    pub fn new(raw_model_output: impl Into<String>, parse_error: impl Into<String>) -> Self {
        Self {
            raw_model_output: raw_model_output.into(),
            parse_error: parse_error.into(),
        }
    }
}

/// Concise typed result for one scenario.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControllerScenarioEvaluation {
    pub scenario_id: String,
    pub expected_action_class: ExpectedActionClass,
    pub expected_decision: ControllerDecision,
    pub acceptable_alternatives: Vec<ControllerDecision>,
    pub observed_decision: ControllerDecision,
    pub rationale: Option<String>,
    pub confidence: Option<f64>,
    pub raw_model_output: Option<String>,
    pub parse_error: Option<String>,
    pub result: ScenarioResult,
    pub error: Option<String>,
}

/// Aggregate evaluation report suitable for deterministic or smoke output.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControllerEvaluationReport {
    pub scenarios: Vec<ControllerScenarioEvaluation>,
    pub passed: usize,
    pub failed: usize,
}

impl ControllerEvaluationReport {
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }

    /// Attach structured-output parse evidence to a failed scenario.
    ///
    /// Keep raw output and parser error visible instead of collapsing the
    /// observation to `Unspecified` without evidence.
    pub fn record_parse_failure(
        &mut self,
        scenario_id: &str,
        diagnostic: ControllerParseDiagnostic,
    ) -> bool {
        let Some(scenario) = self
            .scenarios
            .iter_mut()
            .find(|scenario| scenario.scenario_id == scenario_id)
        else {
            return false;
        };
        scenario.observed_decision = ControllerDecision::Unspecified;
        scenario.rationale = None;
        scenario.confidence = None;
        scenario.raw_model_output = Some(diagnostic.raw_model_output);
        scenario.parse_error = Some(diagnostic.parse_error);
        scenario.result = ScenarioResult::Fail;
        self.passed = self
            .scenarios
            .iter()
            .filter(|scenario| scenario.result == ScenarioResult::Pass)
            .count();
        self.failed = self.scenarios.len() - self.passed;
        true
    }
}

/// Parse exactly one JSON object from the structured-output contract.
///
/// Full-input parsing is intentional: trailing prose, repeated objects and
/// other extra model output are protocol failures, never extraction hints.
pub fn parse_structured_output(text: &str) -> Result<serde_json::Value, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("model output was empty".into());
    }
    let value = serde_json::from_str::<serde_json::Value>(trimmed)
        .map_err(|error| format!("invalid JSON object: {error}"))?;
    value
        .is_object()
        .then_some(value)
        .ok_or_else(|| "structured output must be a JSON object".into())
}

/// Evaluate bounded scenarios through the same packet recommendation path used
/// by the Controller. Runtime failures are recorded as failed scenarios so the
/// aggregate report remains complete.
pub fn evaluate_scenarios(
    scenarios: &[ControllerEvaluationScenario],
    runtime: &mut dyn LocalInferenceRuntime,
) -> Result<ControllerEvaluationReport, ControllerEvaluationError> {
    if scenarios.len() > MAX_EVALUATION_SCENARIOS {
        return Err(ControllerEvaluationError::TooManyScenarios {
            actual: scenarios.len(),
            max: MAX_EVALUATION_SCENARIOS,
        });
    }

    let builder = ControllerStateBuilder::new();
    let mut evaluations = Vec::with_capacity(scenarios.len());
    for scenario in scenarios {
        scenario
            .packet
            .validate()
            .map_err(|source| ControllerEvaluationError::InvalidPacket {
                scenario_id: scenario.id.clone(),
                source,
            })?;
        let recommendation = builder.recommend_packet(&scenario.packet, runtime);
        let (observed_decision, rationale, confidence, raw_model_output, parse_error, error) =
            match recommendation {
                Ok(recommendation) => (
                    ControllerDecision::from_recommendation(&recommendation),
                    Some(recommendation.rationale.clone()),
                    recommendation_confidence(&recommendation),
                    None,
                    None,
                    None,
                ),
                Err(error) => match error {
                    ControllerError::Inference(LocalInferenceError::InvalidStructuredOutput {
                        raw_output,
                        parse_error,
                    }) => (
                        ControllerDecision::Unspecified,
                        None,
                        None,
                        Some(raw_output),
                        Some(parse_error),
                        Some("local inference produced invalid structured output".into()),
                    ),
                    error => (
                        ControllerDecision::Unspecified,
                        None,
                        None,
                        None,
                        None,
                        Some(error.to_string()),
                    ),
                },
            };
        let accepted = observed_decision == scenario.expected_decision
            || scenario
                .acceptable_alternatives
                .contains(&observed_decision);
        evaluations.push(ControllerScenarioEvaluation {
            scenario_id: scenario.id.clone(),
            expected_action_class: scenario.expected_action_class,
            expected_decision: scenario.expected_decision,
            acceptable_alternatives: scenario.acceptable_alternatives.clone(),
            observed_decision,
            rationale,
            confidence,
            raw_model_output,
            parse_error,
            result: if accepted {
                ScenarioResult::Pass
            } else {
                ScenarioResult::Fail
            },
            error,
        });
    }
    let passed = evaluations
        .iter()
        .filter(|evaluation| evaluation.result == ScenarioResult::Pass)
        .count();
    let failed = evaluations.len() - passed;
    Ok(ControllerEvaluationReport {
        scenarios: evaluations,
        passed,
        failed,
    })
}

fn recommendation_confidence(recommendation: &ControllerRecommendation) -> Option<f64> {
    recommendation
        .structured_output
        .as_ref()?
        .get("confidence")?
        .as_f64()
}

/// Curated states used by M02-002 deterministic and opt-in evaluations.
pub fn representative_scenarios()
-> Result<Vec<ControllerEvaluationScenario>, ControllerEvaluationError> {
    let mut scenarios = Vec::with_capacity(7);

    scenarios.push(ControllerEvaluationScenario::new(
        "ready-dispatch",
        "Ready task with passing prerequisites should be dispatched.",
        base_packet(
            "eval-ready",
            "Ready implementation task",
            TaskStatus::Ready,
            QueueCategory::Ready,
            OperationalNextStep::Dispatch,
        ),
        ExpectedActionClass::Dispatch,
        ControllerDecision::NextStep(OperationalNextStep::Dispatch),
        vec![],
    )?);

    let mut packet = base_packet(
        "eval-accept",
        "Acceptance-ready task",
        TaskStatus::AcceptanceReady,
        QueueCategory::AcceptanceReady,
        OperationalNextStep::Accept,
    );
    packet.task.review = review_summary("PASS", 1, 1, 0, true);
    packet.task.review.criteria = vec![review_criterion(
        "criterion-1",
        ReviewCriterionStatus::Satisfied,
        1,
        "The implementation meets the criterion.",
    )];
    scenarios.push(ControllerEvaluationScenario::new(
        "review-pass-accept",
        "A current semantic Review PASS is ready for explicit acceptance.",
        packet,
        ExpectedActionClass::Accept,
        ControllerDecision::NextStep(OperationalNextStep::Accept),
        vec![],
    )?);

    let mut packet = base_packet(
        "eval-revise",
        "Revision-required task",
        TaskStatus::RevisionRequired,
        QueueCategory::RevisionRequired,
        OperationalNextStep::Revise,
    );
    packet.task.review = review_summary("REVISE", 7, 0, 1, true);
    packet.task.review.criteria = vec![review_criterion(
        "criterion-1",
        ReviewCriterionStatus::Violated,
        7,
        "The implementation requires a bounded revision.",
    )];
    packet.task.executions = vec![execution(6, "completed", "completed")];
    scenarios.push(ControllerEvaluationScenario::new(
        "review-revise",
        "A valid Review REVISE with lineage should return to explicit revision.",
        packet,
        ExpectedActionClass::Revise,
        ControllerDecision::NextStep(OperationalNextStep::Revise),
        vec![],
    )?);

    let mut packet = base_packet(
        "eval-revision-exhausted",
        "Validation-repair exhaustion",
        TaskStatus::Blocked,
        QueueCategory::Blocked,
        OperationalNextStep::ResolveBlocker,
    );
    packet.task.validation = validation_summary(
        ValidationState::Failing,
        Some(ValidationState::Failing),
        Some(8),
        Some(crate::validation::ValidationFailureClassification::Implementation),
    );
    packet.task.review = review_summary("REVISE", 7, 0, 1, true);
    packet.task.blockers = vec![blocker(
        "blocker-1",
        "validation_repair_exhausted",
        "Validation repair attempts are exhausted; preserve the review lineage.",
        7,
    )];
    packet.task.executions = vec![execution(8, "failed", "failed")];
    scenarios.push(ControllerEvaluationScenario::new(
        "revision-validation-exhausted",
        "Blocked validation exhaustion must preserve revision lineage, not generic-requeue.",
        packet,
        ExpectedActionClass::PreserveRevisionLineage,
        ControllerDecision::NextStep(OperationalNextStep::ResolveBlocker),
        vec![ControllerDecision::NextStep(OperationalNextStep::Revise)],
    )?);

    let mut packet = base_packet(
        "eval-infrastructure",
        "Infrastructure validation failure",
        TaskStatus::Blocked,
        QueueCategory::Blocked,
        OperationalNextStep::ResolveBlocker,
    );
    packet.task.validation = validation_summary(
        ValidationState::InfrastructureFailure,
        Some(ValidationState::InfrastructureFailure),
        Some(9),
        Some(crate::validation::ValidationFailureClassification::Infrastructure),
    );
    packet.task.blockers = vec![blocker(
        "blocker-2",
        "validation_infrastructure_failure",
        "The validation environment failed before semantic evidence was available.",
        0,
    )];
    scenarios.push(ControllerEvaluationScenario::new(
        "infrastructure-no-revise",
        "Infrastructure failure must not be misclassified as semantic revision.",
        packet,
        ExpectedActionClass::AvoidSemanticRevision,
        ControllerDecision::NextStep(OperationalNextStep::ResolveBlocker),
        vec![ControllerDecision::OperatorDecision],
    )?);

    let mut packet = base_packet(
        "eval-dependency",
        "Dependency-blocked task",
        TaskStatus::Backlog,
        QueueCategory::Backlog,
        OperationalNextStep::SatisfyDependencies,
    );
    packet.task.dependencies = vec![ControllerDependency {
        task_id: "dependency-1".into(),
        status: Some(TaskStatus::Active),
        is_done: false,
    }];
    packet.task.waiting_on = vec!["dependency-1".into()];
    scenarios.push(ControllerEvaluationScenario::new(
        "dependency-blocked",
        "An incomplete dependency must prevent dispatch.",
        packet,
        ExpectedActionClass::SatisfyDependencies,
        ControllerDecision::NextStep(OperationalNextStep::SatisfyDependencies),
        vec![],
    )?);

    let mut packet = base_packet(
        "eval-ambiguous",
        "Inconsistent operational state",
        TaskStatus::AcceptanceReady,
        QueueCategory::Ready,
        OperationalNextStep::Dispatch,
    );
    packet.task.review = review_summary("PASS", 3, 1, 0, true);
    packet.task.validation = validation_summary(
        ValidationState::Passing,
        Some(ValidationState::Passing),
        Some(3),
        None,
    );
    scenarios.push(ControllerEvaluationScenario::new(
        "ambiguous-operator-decision",
        "Conflicting lifecycle and queue facts should request an operator decision.",
        packet,
        ExpectedActionClass::OperatorDecision,
        ControllerDecision::OperatorDecision,
        vec![],
    )?);

    Ok(scenarios)
}

fn base_packet(
    task_id: &str,
    title: &str,
    lifecycle: TaskStatus,
    phase: QueueCategory,
    next_step: OperationalNextStep,
) -> ControllerStatePacket {
    ControllerStatePacket {
        packet_version: crate::controller::CONTROLLER_STATE_PACKET_VERSION,
        project: crate::controller::ControllerProjectState {
            name: Some("M02 decision evaluation".into()),
            self_hosting: ControllerSelfHostingState {
                recognized: false,
                repository_id: None,
                state: SelfHostingReadinessState::NotApplicable,
                blocking_guards: vec![],
            },
        },
        task: ControllerTaskState {
            summary: ControllerTaskSummary {
                task_id: task_id.into(),
                title: title.into(),
                objective: "Select the next legal operation from canonical facts.".into(),
                role: "developer".into(),
                priority: TaskPriority::Normal,
                lifecycle,
                phase,
                next_step,
                cancellation_reason: None,
            },
            contract: ControllerContractSummary {
                unchanged: vec!["kernel legality and lineage".into()],
                acceptance_criteria: vec!["recommendation remains advisory".into()],
                required_tests: vec!["deterministic evaluation".into()],
                validation: vec!["configured validation evidence".into()],
            },
            dependencies: vec![],
            waiting_on: vec![],
            execution_condition: None,
            executions: vec![],
            validation: validation_summary(
                ValidationState::Passing,
                Some(ValidationState::Passing),
                None,
                None,
            ),
            review: review_summary("", 0, 0, 0, false),
            blockers: vec![],
            economy: ControllerEconomySummary {
                invocation_count: 0,
                escalation_count: 0,
                latest_resolution: None,
            },
            recent_activity: vec![] as Vec<ControllerActivityEvent>,
        },
    }
}

fn validation_summary(
    state: ValidationState,
    recorded_state: Option<ValidationState>,
    run_id: Option<i64>,
    failure_classification: Option<crate::validation::ValidationFailureClassification>,
) -> ControllerValidationSummary {
    ControllerValidationSummary {
        state,
        recorded_state,
        run_id,
        timestamp: run_id.map(|_| "2026-09-02T00:00:00Z".into()),
        latest_passing_run_id: (state == ValidationState::Passing).then_some(1),
        latest_passing_timestamp: (state == ValidationState::Passing)
            .then_some("2026-09-02T00:00:00Z".into()),
        is_current: Some(state == ValidationState::Passing),
        worktree_fingerprint: Some("fixture-fingerprint".into()),
        selected_commands: vec![],
        failure_classification,
    }
}

fn review_summary(
    verdict: &str,
    run_id: i64,
    satisfied_criteria: usize,
    violated_criteria: usize,
    applies_to_current_change: bool,
) -> ControllerReviewSummary {
    let total_criteria = satisfied_criteria + violated_criteria;
    ControllerReviewSummary {
        run_id: (run_id > 0).then_some(run_id),
        verdict: (!verdict.is_empty()).then_some(verdict.into()),
        timestamp: (run_id > 0).then_some("2026-09-02T00:00:00Z".into()),
        applies_to_current_change: (run_id > 0).then_some(applies_to_current_change),
        ready_for_review: false,
        actionable_blockers: 0,
        unresolved_blockers: 0,
        regressed_blockers: 0,
        resolved_blockers: 0,
        total_criteria,
        satisfied_criteria,
        violated_criteria,
        insufficient_evidence_criteria: 0,
        criteria: vec![],
    }
}

fn review_criterion(
    criterion_id: &str,
    status: ReviewCriterionStatus,
    run_id: i64,
    rationale: &str,
) -> ControllerReviewCriterion {
    ControllerReviewCriterion {
        criterion_id: criterion_id.into(),
        criterion: "The implementation satisfies the bounded criterion.".into(),
        status,
        evidence: vec![ControllerEvidenceRef {
            kind: ReviewEvidenceKind::Validation,
            reference: format!("validation-run-{run_id}"),
            explanation: "Bounded validation evidence reference.".into(),
        }],
        rationale: rationale.into(),
    }
}

fn execution(id: i64, status: &str, outcome: &str) -> ControllerExecutionSummary {
    ControllerExecutionSummary {
        id,
        agent: "fixture-agent".into(),
        execution_mode: "automated".into(),
        execution_class: "code".into(),
        status: status.into(),
        phase: Some("implementation".into()),
        is_active: false,
        started_at: "2026-09-02T00:00:00Z".into(),
        finished_at: Some("2026-09-02T00:00:01Z".into()),
        last_activity: "2026-09-02T00:00:01Z".into(),
        outcome: Some(outcome.into()),
        failure_category: None,
        persisted_model: Some("fixture-model".into()),
        persisted_effort: Some(ReasoningEffort::Medium),
        persisted_resolution_source: "fixture".into(),
        error: None,
    }
}

fn blocker(id: &str, key: &str, summary: &str, review_run_id: i64) -> ControllerBlockerSummary {
    ControllerBlockerSummary {
        id: id.into(),
        key: key.into(),
        state: BlockerState::Unresolved,
        actionable: true,
        summary: summary.into(),
        requirement: "Resolve the recorded blocker without discarding lineage.".into(),
        evidence: "bounded-evidence-ref".into(),
        severity: "high".into(),
        acceptance_condition: "A deterministic kernel operation resolves the blocker.".into(),
        originating_review_run_id: review_run_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_runtime::{
        LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    };
    use serde_json::Value;
    use std::collections::VecDeque;

    struct FakeRuntime {
        responses: VecDeque<Result<LocalInferenceResponse, LocalInferenceError>>,
    }

    impl FakeRuntime {
        fn new(responses: Vec<Result<LocalInferenceResponse, LocalInferenceError>>) -> Self {
            Self {
                responses: responses.into(),
            }
        }
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            _request: &LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            self.responses
                .pop_front()
                .unwrap_or_else(|| Err(LocalInferenceError::Backend("no fixture response".into())))
        }
    }

    fn response(decision: ControllerDecision) -> LocalInferenceResponse {
        let mut structured = serde_json::Map::new();
        match decision {
            ControllerDecision::NextStep(next_step) => {
                structured.insert("suggested_next_step".into(), serde_json::json!(next_step));
                structured.insert("decision_class".into(), serde_json::json!("action"));
            }
            ControllerDecision::OperatorDecision => {
                structured.insert("suggested_next_step".into(), Value::Null);
                structured.insert(
                    "decision_class".into(),
                    serde_json::json!("operator_decision"),
                );
            }
            ControllerDecision::Unspecified => {}
        }
        structured.insert(
            "rationale".into(),
            serde_json::json!("typed fixture rationale"),
        );
        structured.insert("confidence".into(), serde_json::json!(0.75));
        LocalInferenceResponse::structured("typed fixture", Value::Object(structured))
    }

    #[test]
    fn representative_scenarios_are_bounded_and_typed() {
        let scenarios = representative_scenarios().expect("scenario corpus");
        assert_eq!(scenarios.len(), 7);
        for scenario in scenarios {
            assert!(scenario.packet.validate().is_ok());
            assert!(scenario.id.len() <= MAX_SCENARIO_TEXT_BYTES);
            assert!(scenario.description.len() <= MAX_SCENARIO_TEXT_BYTES);
            assert!(scenario.acceptable_alternatives.len() <= MAX_ACCEPTABLE_ALTERNATIVES);
        }
    }

    #[test]
    fn fake_runtime_evaluation_matches_typed_semantics() {
        let scenarios = representative_scenarios().expect("scenario corpus");
        let responses = scenarios
            .iter()
            .map(|scenario| Ok(response(scenario.expected_decision)))
            .collect();
        let mut runtime = FakeRuntime::new(responses);
        let report = evaluate_scenarios(&scenarios, &mut runtime).expect("evaluation report");
        assert_eq!(report.passed, 7);
        assert_eq!(report.failed, 0);
        assert!(report.is_success());
        assert_eq!(
            report.scenarios[0].rationale.as_deref(),
            Some("typed fixture rationale")
        );
        assert_eq!(report.scenarios[0].confidence, Some(0.75));
    }

    #[test]
    fn acceptable_alternative_is_counted_as_pass() {
        let scenarios = representative_scenarios().expect("scenario corpus");
        let responses = scenarios
            .iter()
            .enumerate()
            .map(|(index, scenario)| {
                let decision = if index == 3 {
                    ControllerDecision::NextStep(OperationalNextStep::Revise)
                } else {
                    scenario.expected_decision
                };
                Ok(response(decision))
            })
            .collect();
        let mut runtime = FakeRuntime::new(responses);
        let report = evaluate_scenarios(&scenarios, &mut runtime).expect("evaluation report");
        assert_eq!(report.passed, 7);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn incorrect_typed_decision_fails_without_prose_matching() {
        let scenarios = representative_scenarios().expect("scenario corpus");
        let responses = scenarios
            .iter()
            .map(|_| {
                Ok(response(ControllerDecision::NextStep(
                    OperationalNextStep::Dispatch,
                )))
            })
            .collect();
        let mut runtime = FakeRuntime::new(responses);
        let report = evaluate_scenarios(&scenarios, &mut runtime).expect("evaluation report");
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 6);
        assert_eq!(
            report.scenarios[1].observed_decision,
            ControllerDecision::NextStep(OperationalNextStep::Dispatch)
        );
    }

    #[test]
    fn runtime_error_is_recorded_as_failed_scenario() {
        let scenarios = representative_scenarios().expect("scenario corpus");
        let mut runtime = FakeRuntime::new(vec![Err(LocalInferenceError::Backend(
            "fixture failure".into(),
        ))]);
        let report = evaluate_scenarios(&scenarios[..1], &mut runtime).expect("evaluation report");
        assert_eq!(report.passed, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(
            report.scenarios[0].observed_decision,
            ControllerDecision::Unspecified
        );
        assert!(
            report.scenarios[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("fixture failure"))
        );
    }

    #[test]
    fn malformed_structured_output_diagnostics_are_retained() {
        let scenarios = representative_scenarios().expect("scenario corpus");
        let raw = "not valid JSON";
        let parse_error = parse_structured_output(raw).expect_err("malformed output");
        let mut runtime = FakeRuntime::new(vec![Ok(LocalInferenceResponse::text(raw))]);
        let mut report = evaluate_scenarios(&scenarios[..1], &mut runtime).expect("report");

        assert_eq!(
            report.scenarios[0].observed_decision,
            ControllerDecision::Unspecified
        );
        assert!(report.record_parse_failure(
            "ready-dispatch",
            ControllerParseDiagnostic::new(raw, &parse_error),
        ));
        let evaluation = &report.scenarios[0];
        assert_eq!(
            evaluation.expected_action_class,
            ExpectedActionClass::Dispatch
        );
        assert_eq!(evaluation.raw_model_output.as_deref(), Some(raw));
        assert_eq!(
            evaluation.parse_error.as_deref(),
            Some(parse_error.as_str())
        );
        assert_eq!(evaluation.result, ScenarioResult::Fail);
        assert_eq!(report.passed, 0);
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn runtime_structured_parse_failures_retain_raw_evidence() {
        let scenarios = representative_scenarios().expect("scenario corpus");
        let raw = r#"{"suggested_next_step":"dispatch"} trailing"#;
        let mut runtime =
            FakeRuntime::new(vec![Err(LocalInferenceError::InvalidStructuredOutput {
                raw_output: raw.into(),
                parse_error: "trailing output".into(),
            })]);
        let report = evaluate_scenarios(&scenarios[..1], &mut runtime).expect("report");

        let evaluation = &report.scenarios[0];
        assert_eq!(evaluation.raw_model_output.as_deref(), Some(raw));
        assert_eq!(evaluation.parse_error.as_deref(), Some("trailing output"));
        assert_eq!(evaluation.result, ScenarioResult::Fail);
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn strict_parser_rejects_trailing_or_repeated_json() {
        assert!(parse_structured_output(r#"{"suggested_next_step":"dispatch"}"#).is_ok());
        assert!(
            parse_structured_output(r#"{"suggested_next_step":"dispatch"} trailing prose"#)
                .is_err()
        );
        assert!(
            parse_structured_output(r#"{"suggested_next_step":"dispatch"}{"repeat":true}"#)
                .is_err()
        );
    }
}
