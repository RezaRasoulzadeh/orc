//! Read-only structured recovery recommendation.
//!
//! This is the M04-002 boundary between the bounded M04-001 inspection and a
//! trusted caller. It can recommend a repository-defined operation, but it
//! cannot authorize or execute one.

use crate::controller_memory::ControllerMemoryContext;
use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::operations::ValidationState;
use crate::queue::QueueCategory;
use crate::recovery::{
    RecoveryCondition, RecoveryExecutionCondition, RecoveryExecutionConditionKind,
    RecoveryInspection, RecoveryObservation, RecoveryObservationState, RecoveryOperation,
    RecoveryOperationLegality, RecoveryReviewFacts, RecoveryRevisionLineage,
    RecoveryValidationFacts,
};
use crate::task::TaskStatus;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_RECOVERY_RATIONALE_BYTES: usize = 1024;
const MAX_RECOVERY_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RECOVERY_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_EVALUATION_SCENARIOS: usize = 16;

/// The only decisions a recovery model may return. The first three names map
/// one-to-one to existing [`RecoveryOperation`] values. OperatorDecision is a
/// typed non-mutating fallback, not a recovery operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRecommendationDecision {
    Requeue,
    ResumeRevision,
    AcknowledgeNonConvergence,
    OperatorDecision,
}

impl RecoveryRecommendationDecision {
    pub const fn operation(self) -> Option<RecoveryOperation> {
        match self {
            Self::Requeue => Some(RecoveryOperation::Requeue),
            Self::ResumeRevision => Some(RecoveryOperation::ResumeRevision),
            Self::AcknowledgeNonConvergence => Some(RecoveryOperation::AcknowledgeNonConvergence),
            Self::OperatorDecision => None,
        }
    }
}

/// Bounded, model-independent structured output. It intentionally contains no
/// task identity, command, path, SQL, provider payload, runtime handle,
/// authorization, or executable parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRecommendation {
    pub decision: RecoveryRecommendationDecision,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

impl RecoveryRecommendation {
    /// Validate the canonical structured recommendation contract used by the
    /// recovery inference parser. This remains a pure, bounded check and does
    /// not perform legality or execution.
    pub fn validate(&self) -> Result<(), RecoveryControllerError> {
        if self.rationale.is_empty() || self.rationale.len() > MAX_RECOVERY_RATIONALE_BYTES {
            return Err(RecoveryControllerError::MalformedStructuredOutput(
                "rationale must be non-empty and bounded".into(),
            ));
        }
        if self
            .confidence
            .is_some_and(|confidence| !confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
        {
            return Err(RecoveryControllerError::MalformedStructuredOutput(
                "confidence must be finite and between 0 and 1".into(),
            ));
        }
        Ok(())
    }
}

/// The current recovery facts and exact operation-legality results supplied to
/// the recovery capability. Memory is deliberately kept outside this type so
/// the current inspection remains a distinct authority boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInferenceRequest {
    pub observation: RecoveryObservation,
    pub legal_operations: Vec<RecoveryOperationLegality>,
}

/// Capability-local recovery inference input. The current inspection remains
/// authoritative; memory is bounded, typed, and read-only advisory context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInferenceInput {
    pub current_request: RecoveryInferenceRequest,
    pub memory: ControllerMemoryContext,
}

impl RecoveryInferenceInput {
    pub fn from_inspection(
        inspection: &RecoveryInspection,
        memory: ControllerMemoryContext,
    ) -> Self {
        Self {
            current_request: RecoveryInferenceRequest::from_inspection(inspection),
            memory,
        }
    }

    pub fn validate(&self) -> Result<String, RecoveryControllerError> {
        self.current_request.validate()?;
        self.memory
            .validate()
            .map_err(|error| RecoveryControllerError::MemoryContext(error.to_string()))?;
        let serialized = serde_json::to_vec(self)
            .map_err(|error| RecoveryControllerError::Serialization(error.to_string()))?;
        if serialized.len() > MAX_RECOVERY_REQUEST_BYTES {
            return Err(RecoveryControllerError::RequestTooLarge {
                actual: serialized.len(),
                max: MAX_RECOVERY_REQUEST_BYTES,
            });
        }
        String::from_utf8(serialized)
            .map_err(|error| RecoveryControllerError::Serialization(error.to_string()))
    }
}

impl RecoveryInferenceRequest {
    pub fn from_inspection(inspection: &RecoveryInspection) -> Self {
        Self {
            observation: inspection.observation.clone(),
            legal_operations: inspection.operations.clone(),
        }
    }

    fn validate(&self) -> Result<String, RecoveryControllerError> {
        let serialized = serde_json::to_vec(self)
            .map_err(|error| RecoveryControllerError::Serialization(error.to_string()))?;
        if serialized.len() > MAX_RECOVERY_REQUEST_BYTES {
            return Err(RecoveryControllerError::RequestTooLarge {
                actual: serialized.len(),
                max: MAX_RECOVERY_REQUEST_BYTES,
            });
        }
        String::from_utf8(serialized)
            .map_err(|error| RecoveryControllerError::Serialization(error.to_string()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRecommendationRejection {
    OperationNotCurrentlyLegal,
}

/// Deterministic result of checking a recommendation against the exact
/// legality set supplied by M04-001.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum RecoveryRecommendationValidation {
    Actionable {
        operation: RecoveryOperation,
    },
    OperatorDecision,
    Rejected {
        operation: RecoveryOperation,
        reason: RecoveryRecommendationRejection,
    },
}

impl RecoveryRecommendationValidation {
    pub const fn is_actionable(self) -> bool {
        matches!(self, Self::Actionable { .. })
    }
}

/// A read-only recommendation plus its exact typed validation result. No
/// authorization is present and no method on this result mutates state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecoveryRecommendationResult {
    pub inspection: RecoveryInspection,
    pub recommendation: RecoveryRecommendation,
    pub validation: RecoveryRecommendationValidation,
}

#[derive(Debug, Error)]
pub enum RecoveryControllerError {
    #[error("recovery inspection failed: {0}")]
    Inspection(#[source] crate::recovery::RecoveryError),
    #[error("recovery inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
    #[error("recovery recommendation serialization failed: {0}")]
    Serialization(String),
    #[error("recovery memory context failed: {0}")]
    MemoryContext(String),
    #[error("recovery inference request is {actual} bytes; maximum is {max}")]
    RequestTooLarge { actual: usize, max: usize },
    #[error("malformed recovery structured output: {0}")]
    MalformedStructuredOutput(String),
}

/// Trusted application-owned builder for a read-only recovery proposal.
#[derive(Clone, Copy, Debug, Default)]
pub struct RecoveryRecommendationBuilder;

impl RecoveryRecommendationBuilder {
    pub const fn new() -> Self {
        Self
    }

    /// Inspect canonically, infer once, and validate the typed answer. This
    /// method deliberately has no authorization or execution argument.
    pub fn recommend(
        &self,
        operations: &crate::operations::ProjectOperations<'_>,
        task_id: &str,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<RecoveryRecommendationResult, RecoveryControllerError> {
        self.recommend_with_memory(
            operations,
            task_id,
            ControllerMemoryContext::empty(),
            runtime,
        )
    }

    /// Inspect canonically and infer with a caller-supplied bounded memory
    /// context. Memory does not participate in inspection or authorization.
    pub fn recommend_with_memory(
        &self,
        operations: &crate::operations::ProjectOperations<'_>,
        task_id: &str,
        memory: ControllerMemoryContext,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<RecoveryRecommendationResult, RecoveryControllerError> {
        let inspection = crate::recovery::inspect_recovery(operations, task_id)
            .map_err(RecoveryControllerError::Inspection)?;
        self.recommend_inspection_with_memory(&inspection, memory, runtime)
    }

    /// Run inference from a caller-supplied M04-001 inspection. The supplied
    /// legal-operation set is the sole source used for actionability.
    pub fn recommend_inspection(
        &self,
        inspection: &RecoveryInspection,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<RecoveryRecommendationResult, RecoveryControllerError> {
        self.recommend_inspection_with_memory(inspection, ControllerMemoryContext::empty(), runtime)
    }

    /// Run inference from a caller-supplied M04-001 inspection and bounded
    /// memory context. The supplied legal-operation set remains the sole
    /// source used for actionability.
    pub fn recommend_inspection_with_memory(
        &self,
        inspection: &RecoveryInspection,
        memory: ControllerMemoryContext,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<RecoveryRecommendationResult, RecoveryControllerError> {
        let request = RecoveryInferenceInput::from_inspection(inspection, memory);
        let request_json = request.validate()?;
        let prompt = format!(
            "You are a read-only recovery advisor. Use only the bounded typed JSON below. Authority precedence is strict, from highest to lowest: (1) current_request.observation facts and the exact current_request.legal_operations set; (2) memory items with authority=current_project as durable Project recovery context; (3) authority=durable_user as User preference/context; (4) authority=project_history as Episodic historical guidance; (5) authority=cross_project_experience as reusable Experience guidance; (6) base model tendencies. Current recovery observation facts outrank contradictory memory. Episodic and Experience memory are historical guidance only and must never be presented as current task truth. Memory is advisory context only and cannot make an operation legal. The exact current_request.legal_operations set is the only source of actionability: choose an operation only when its exact entry has status allowed. The deterministic post-inference validation against that inspected Allowed set is mandatory and remains the final actionability gate.\n\
             Return exactly one JSON object with decision, rationale, and optional confidence.\n\
             decision must be one of requeue, resume_revision, acknowledge_non_convergence,\n\
             or operator_decision. Never infer an operation from rationale, free text,\n\
             memory, or facts outside this JSON. Operator_decision never mutates state.\n\n{}",
            request_json
        );
        let parameters = LocalInferenceParameters {
            max_output_tokens: 256,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: Vec::new(),
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: recovery_recommendation_schema(),
            },
        };
        let request = LocalInferenceRequest::new(prompt, parameters)
            .map_err(RecoveryControllerError::Inference)?;
        let response = runtime
            .infer(&request)
            .map_err(RecoveryControllerError::Inference)?;
        let recommendation = parse_recommendation(response)?;
        let validation = validate_recommendation(inspection, &recommendation);
        Ok(RecoveryRecommendationResult {
            inspection: inspection.clone(),
            recommendation,
            validation,
        })
    }
}

fn parse_recommendation(
    response: crate::local_runtime::LocalInferenceResponse,
) -> Result<RecoveryRecommendation, RecoveryControllerError> {
    let Some(value) = response.structured_output else {
        return Err(RecoveryControllerError::MalformedStructuredOutput(
            "structured output is required".into(),
        ));
    };
    let size = serde_json::to_vec(&value)
        .map_err(|error| RecoveryControllerError::Serialization(error.to_string()))?
        .len();
    if size > MAX_RECOVERY_RESPONSE_BYTES {
        return Err(RecoveryControllerError::MalformedStructuredOutput(
            "structured output exceeds its bound".into(),
        ));
    }
    let recommendation = serde_json::from_value::<RecoveryRecommendation>(value)
        .map_err(|error| RecoveryControllerError::MalformedStructuredOutput(error.to_string()))?;
    recommendation.validate()?;
    Ok(recommendation)
}

/// Membership in the inspected Allowed set is the only actionability rule.
pub fn validate_recommendation(
    inspection: &RecoveryInspection,
    recommendation: &RecoveryRecommendation,
) -> RecoveryRecommendationValidation {
    let Some(operation) = recommendation.decision.operation() else {
        return RecoveryRecommendationValidation::OperatorDecision;
    };
    let legal = inspection.operations.iter().any(|legality| {
        matches!(
            legality,
            RecoveryOperationLegality::Allowed { operation: allowed } if *allowed == operation
        )
    });
    if legal {
        RecoveryRecommendationValidation::Actionable { operation }
    } else {
        RecoveryRecommendationValidation::Rejected {
            operation,
            reason: RecoveryRecommendationRejection::OperationNotCurrentlyLegal,
        }
    }
}

/// The strict schema used by every recovery recommendation request.
pub fn recovery_recommendation_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "decision": {
                "type": "string",
                "enum": ["requeue", "resume_revision", "acknowledge_non_convergence", "operator_decision"]
            },
            "rationale": {"type": "string", "minLength": 1, "maxLength": MAX_RECOVERY_RATIONALE_BYTES},
            "confidence": {"type": ["number", "null"], "minimum": 0, "maximum": 1}
        },
        "required": ["decision", "rationale"],
        "additionalProperties": false
    })
}

/// A small deterministic decision-quality corpus. Its inspections are typed
/// fixtures of the canonical M04-001 output; no policy is added here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecoveryEvaluationScenario {
    pub id: String,
    pub inspection: RecoveryInspection,
    pub expected: RecoveryRecommendationDecision,
}

/// A recovery evaluation scenario with explicit bounded advisory memory.
/// Existing empty-memory evaluation scenarios remain source-compatible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecoveryMemoryEvaluationScenario {
    pub scenario: RecoveryEvaluationScenario,
    pub memory: ControllerMemoryContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryScenarioResult {
    Pass,
    Fail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryScenarioEvaluation {
    pub scenario_id: String,
    pub expected: RecoveryRecommendationDecision,
    pub observed: Option<RecoveryRecommendationDecision>,
    pub strict_contract: bool,
    pub validation: Option<RecoveryRecommendationValidation>,
    pub result: RecoveryScenarioResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryEvaluationReport {
    pub scenarios: Vec<RecoveryScenarioEvaluation>,
    pub strict_passed: usize,
    pub strict_failed: usize,
    pub semantic_passed: usize,
    pub semantic_failed: usize,
}

impl RecoveryEvaluationReport {
    pub const fn is_success(&self) -> bool {
        self.strict_failed == 0 && self.semantic_failed == 0
    }
}

pub fn evaluate_recovery_scenarios(
    scenarios: &[RecoveryEvaluationScenario],
    runtime: &mut dyn LocalInferenceRuntime,
) -> Result<RecoveryEvaluationReport, RecoveryControllerError> {
    let scenarios = scenarios
        .iter()
        .cloned()
        .map(|scenario| RecoveryMemoryEvaluationScenario {
            scenario,
            memory: ControllerMemoryContext::empty(),
        })
        .collect::<Vec<_>>();
    evaluate_recovery_scenarios_with_memory(&scenarios, runtime)
}

pub fn evaluate_recovery_scenarios_with_memory(
    scenarios: &[RecoveryMemoryEvaluationScenario],
    runtime: &mut dyn LocalInferenceRuntime,
) -> Result<RecoveryEvaluationReport, RecoveryControllerError> {
    if scenarios.len() > MAX_EVALUATION_SCENARIOS {
        return Err(RecoveryControllerError::RequestTooLarge {
            actual: scenarios.len(),
            max: MAX_EVALUATION_SCENARIOS,
        });
    }
    let builder = RecoveryRecommendationBuilder::new();
    let mut evaluations = Vec::with_capacity(scenarios.len());
    for scenario in scenarios {
        match builder.recommend_inspection_with_memory(
            &scenario.scenario.inspection,
            scenario.memory.clone(),
            runtime,
        ) {
            Ok(result) => {
                let observed = result.recommendation.decision;
                let pass = observed == scenario.scenario.expected
                    && matches!(
                        (&scenario.scenario.expected, result.validation),
                        (
                            RecoveryRecommendationDecision::OperatorDecision,
                            RecoveryRecommendationValidation::OperatorDecision
                        ) | (
                            RecoveryRecommendationDecision::Requeue,
                            RecoveryRecommendationValidation::Actionable {
                                operation: RecoveryOperation::Requeue
                            }
                        ) | (
                            RecoveryRecommendationDecision::ResumeRevision,
                            RecoveryRecommendationValidation::Actionable {
                                operation: RecoveryOperation::ResumeRevision
                            }
                        ) | (
                            RecoveryRecommendationDecision::AcknowledgeNonConvergence,
                            RecoveryRecommendationValidation::Actionable {
                                operation: RecoveryOperation::AcknowledgeNonConvergence
                            }
                        )
                    );
                evaluations.push(RecoveryScenarioEvaluation {
                    scenario_id: scenario.scenario.id.clone(),
                    expected: scenario.scenario.expected,
                    observed: Some(observed),
                    strict_contract: true,
                    validation: Some(result.validation),
                    result: if pass {
                        RecoveryScenarioResult::Pass
                    } else {
                        RecoveryScenarioResult::Fail
                    },
                });
            }
            Err(_) => evaluations.push(RecoveryScenarioEvaluation {
                scenario_id: scenario.scenario.id.clone(),
                expected: scenario.scenario.expected,
                observed: None,
                strict_contract: false,
                validation: None,
                result: RecoveryScenarioResult::Fail,
            }),
        }
    }
    let strict_passed = evaluations
        .iter()
        .filter(|item| item.strict_contract)
        .count();
    let semantic_passed = evaluations
        .iter()
        .filter(|item| item.result == RecoveryScenarioResult::Pass)
        .count();
    Ok(RecoveryEvaluationReport {
        strict_failed: evaluations.len() - strict_passed,
        semantic_failed: evaluations.len() - semantic_passed,
        scenarios: evaluations,
        strict_passed,
        semantic_passed,
    })
}

fn allowed(operation: RecoveryOperation) -> RecoveryOperationLegality {
    RecoveryOperationLegality::Allowed { operation }
}

fn rejected(operation: RecoveryOperation) -> RecoveryOperationLegality {
    RecoveryOperationLegality::Rejected {
        operation,
        reason: crate::recovery::RecoveryOperationRejection::CanonicalLegalityRejected,
    }
}

fn scenario(
    id: &str,
    condition: RecoveryCondition,
    expected: RecoveryRecommendationDecision,
    legal: Option<RecoveryOperation>,
) -> RecoveryEvaluationScenario {
    let operations = [
        RecoveryOperation::Requeue,
        RecoveryOperation::ResumeRevision,
        RecoveryOperation::AcknowledgeNonConvergence,
    ]
    .into_iter()
    .map(|operation| {
        if Some(operation) == legal {
            allowed(operation)
        } else {
            rejected(operation)
        }
    })
    .collect();
    let execution_condition = if condition == RecoveryCondition::NonConvergenceReplanRequired {
        Some(RecoveryExecutionCondition {
            kind: RecoveryExecutionConditionKind::NonConvergenceReplanRequired,
            details_present: true,
        })
    } else {
        None
    };
    let revision = if condition == RecoveryCondition::BlockedRevision {
        RecoveryRevisionLineage {
            actionable_review_run_id: Some(1),
            actionable_contract_id: Some(1),
            contract_source_review_run_id: Some(1),
        }
    } else {
        RecoveryRevisionLineage {
            actionable_review_run_id: None,
            actionable_contract_id: None,
            contract_source_review_run_id: None,
        }
    };
    RecoveryEvaluationScenario {
        id: id.into(),
        inspection: RecoveryInspection {
            observation: RecoveryObservation {
                task_id: format!("task-{id}"),
                state: if condition == RecoveryCondition::Ambiguous {
                    RecoveryObservationState::Ambiguous
                } else {
                    RecoveryObservationState::Abnormal
                },
                lifecycle: if condition == RecoveryCondition::BlockedRevision {
                    TaskStatus::Blocked
                } else {
                    TaskStatus::Active
                },
                queue_phase: if condition == RecoveryCondition::DependencyBlocked {
                    QueueCategory::Blocked
                } else {
                    QueueCategory::Ready
                },
                conditions: vec![condition],
                execution_condition,
                validation: RecoveryValidationFacts {
                    state: ValidationState::None,
                    failure_classification: None,
                    is_current: None,
                },
                review: RecoveryReviewFacts {
                    run_id: None,
                    verdict: None,
                    applies_to_current_change: None,
                },
                revision,
                latest_execution: None,
                dependencies: Vec::new(),
                blockers: Vec::new(),
                agent_economy: crate::recovery::RecoveryAgentEconomyFacts {
                    candidate_count: 0,
                    eligible_count: 0,
                    constraints: Vec::new(),
                },
            },
            operations,
        },
        expected,
    }
}

pub fn representative_recovery_scenarios() -> Vec<RecoveryEvaluationScenario> {
    vec![
        scenario(
            "blocked-revision",
            RecoveryCondition::BlockedRevision,
            RecoveryRecommendationDecision::ResumeRevision,
            Some(RecoveryOperation::ResumeRevision),
        ),
        scenario(
            "non-convergence",
            RecoveryCondition::NonConvergenceReplanRequired,
            RecoveryRecommendationDecision::AcknowledgeNonConvergence,
            Some(RecoveryOperation::AcknowledgeNonConvergence),
        ),
        scenario(
            "requeueable-failure",
            RecoveryCondition::ExecutionFailure,
            RecoveryRecommendationDecision::Requeue,
            Some(RecoveryOperation::Requeue),
        ),
        scenario(
            "dependency-blocked",
            RecoveryCondition::DependencyBlocked,
            RecoveryRecommendationDecision::OperatorDecision,
            None,
        ),
        scenario(
            "infrastructure-failure",
            RecoveryCondition::InfrastructureFailure,
            RecoveryRecommendationDecision::OperatorDecision,
            None,
        ),
        scenario(
            "economy-exhaustion",
            RecoveryCondition::EconomyExhaustion,
            RecoveryRecommendationDecision::OperatorDecision,
            None,
        ),
        scenario(
            "ambiguous-no-legal-operation",
            RecoveryCondition::Ambiguous,
            RecoveryRecommendationDecision::OperatorDecision,
            None,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_memory::{
        CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryItem,
        MAX_CONTROLLER_MEMORY_CONTENT_BYTES,
    };
    use crate::local_runtime::{LocalInferenceRequest, LocalInferenceResponse};
    use crate::memory::{
        MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
    };
    use crate::storage::Database;
    use crate::task::TaskPriority;
    use std::collections::VecDeque;

    struct FakeRuntime {
        responses: VecDeque<Result<LocalInferenceResponse, LocalInferenceError>>,
        requests: Vec<LocalInferenceRequest>,
    }

    impl FakeRuntime {
        fn new(responses: Vec<Result<LocalInferenceResponse, LocalInferenceError>>) -> Self {
            Self {
                responses: responses.into(),
                requests: Vec::new(),
            }
        }
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            request: &LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            request.validate()?;
            self.requests.push(request.clone());
            self.responses
                .pop_front()
                .unwrap_or_else(|| Err(LocalInferenceError::Backend("no fixture".into())))
        }
    }

    fn response(decision: RecoveryRecommendationDecision) -> LocalInferenceResponse {
        LocalInferenceResponse::structured(
            "ignored free-form output that cannot select an action",
            serde_json::json!({
                "decision": decision,
                "rationale": "bounded typed fixture rationale",
                "confidence": 0.75
            }),
        )
    }

    fn memory_item(
        id: MemoryId,
        kind: MemoryKind,
        scope: MemoryScope,
        authority: ControllerMemoryAuthority,
        subject: &str,
        content: &str,
        provenance: MemoryProvenanceKind,
    ) -> ControllerMemoryItem {
        ControllerMemoryItem {
            id,
            kind,
            scope,
            authority,
            subject: subject.into(),
            content: content.into(),
            provenance: MemoryProvenance {
                kind: provenance,
                source_reference: Some("recovery-controller-test".into()),
            },
            confidence: Some(0.8),
            lifecycle: MemoryLifecycle::Active,
            supersedes: None,
        }
    }

    #[test]
    fn representative_fake_runtime_covers_all_required_decisions() {
        let scenarios = representative_recovery_scenarios();
        let responses = scenarios
            .iter()
            .map(|scenario| Ok(response(scenario.expected)))
            .collect();
        let mut runtime = FakeRuntime::new(responses);
        let report = evaluate_recovery_scenarios(&scenarios, &mut runtime).unwrap();
        assert_eq!(report.strict_passed, 7);
        assert_eq!(report.semantic_passed, 7);
        assert!(report.is_success());
    }

    #[test]
    fn valid_choice_is_actionable_only_for_exact_allowed_membership() {
        let scenario = representative_recovery_scenarios().remove(0);
        let mut runtime = FakeRuntime::new(vec![Ok(response(
            RecoveryRecommendationDecision::ResumeRevision,
        ))]);
        let result = RecoveryRecommendationBuilder::new()
            .recommend_inspection(&scenario.inspection, &mut runtime)
            .unwrap();
        assert_eq!(
            result.validation,
            RecoveryRecommendationValidation::Actionable {
                operation: RecoveryOperation::ResumeRevision
            }
        );
    }

    #[test]
    fn illegal_choice_is_rejected_without_fallback_to_mutation() {
        let mut scenario = representative_recovery_scenarios().remove(0);
        scenario.inspection.operations = scenario
            .inspection
            .operations
            .into_iter()
            .map(|legality| match legality {
                RecoveryOperationLegality::Allowed { operation } => rejected(operation),
                other => other,
            })
            .collect();
        let mut runtime = FakeRuntime::new(vec![Ok(response(
            RecoveryRecommendationDecision::ResumeRevision,
        ))]);
        let result = RecoveryRecommendationBuilder::new()
            .recommend_inspection(&scenario.inspection, &mut runtime)
            .unwrap();
        assert!(matches!(
            result.validation,
            RecoveryRecommendationValidation::Rejected { .. }
        ));
        assert!(!result.validation.is_actionable());
    }

    #[test]
    fn malformed_or_model_invented_structured_output_is_rejected() {
        let scenario = representative_recovery_scenarios().remove(0);
        let malformed = LocalInferenceResponse::structured(
            "ignored",
            serde_json::json!({
                "decision": "retry",
                "rationale": "try arbitrary behavior",
            }),
        );
        let mut runtime = FakeRuntime::new(vec![Ok(malformed)]);
        assert!(matches!(
            RecoveryRecommendationBuilder::new()
                .recommend_inspection(&scenario.inspection, &mut runtime),
            Err(RecoveryControllerError::MalformedStructuredOutput(_))
        ));
    }

    #[test]
    fn operator_decision_is_typed_non_mutating_fallback() {
        let scenario = representative_recovery_scenarios().pop().unwrap();
        let before = serde_json::to_vec(&scenario.inspection).unwrap();
        let mut runtime = FakeRuntime::new(vec![Ok(response(
            RecoveryRecommendationDecision::OperatorDecision,
        ))]);
        let result = RecoveryRecommendationBuilder::new()
            .recommend_inspection(&scenario.inspection, &mut runtime)
            .unwrap();
        assert_eq!(
            result.validation,
            RecoveryRecommendationValidation::OperatorDecision
        );
        assert_eq!(serde_json::to_vec(&scenario.inspection).unwrap(), before);
    }

    #[test]
    fn request_contains_only_bounded_observation_and_legality() {
        let scenario = representative_recovery_scenarios().remove(0);
        let request = RecoveryInferenceRequest::from_inspection(&scenario.inspection);
        let value = serde_json::to_value(request).unwrap();
        assert!(
            value
                .as_object()
                .unwrap()
                .keys()
                .all(|key| key == "observation" || key == "legal_operations")
        );
        assert!(value.get("task_id").is_none());
        let mut runtime = FakeRuntime::new(vec![Ok(response(
            RecoveryRecommendationDecision::ResumeRevision,
        ))]);
        RecoveryRecommendationBuilder::new()
            .recommend_inspection(&scenario.inspection, &mut runtime)
            .unwrap();
        let prompt = &runtime.requests[0].prompt;
        assert!(prompt.contains("legal_operations"));
        assert!(prompt.contains("\"memory\":{\"context_version\":1,\"items\":[]}"));
        assert!(!prompt.contains("provider_payload"));
    }

    #[test]
    fn memory_input_preserves_typed_metadata_and_explicit_precedence() {
        let scenario = representative_recovery_scenarios().remove(0);
        let memory = ControllerMemoryContext {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: vec![
                memory_item(
                    MemoryId::Project {
                        project_id: 1,
                        id: 1,
                    },
                    MemoryKind::Project,
                    MemoryScope::Project { project_id: 1 },
                    ControllerMemoryAuthority::CurrentProject,
                    "recovery-context",
                    "The prior recovery needed a focused retry.",
                    MemoryProvenanceKind::ProjectFact,
                ),
                memory_item(
                    MemoryId::Global(2),
                    MemoryKind::Experience,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "history",
                    "Past recovery attempts preferred requeue.",
                    MemoryProvenanceKind::ControllerApproved,
                ),
            ],
        };
        let input = RecoveryInferenceInput::from_inspection(&scenario.inspection, memory);
        let serialized = serde_json::to_value(&input).unwrap();
        assert!(serialized["current_request"]["observation"].is_object());
        assert_eq!(serialized["memory"]["items"][0]["kind"], "project");
        assert_eq!(
            serialized["memory"]["items"][0]["scope"]["Project"]["project_id"],
            1
        );
        assert_eq!(
            serialized["memory"]["items"][0]["authority"],
            "current_project"
        );
        assert_eq!(
            serialized["memory"]["items"][0]["provenance"]["kind"],
            "project_fact"
        );
        assert_eq!(
            serialized["memory"]["items"][0]["provenance"]["source_reference"],
            "recovery-controller-test"
        );

        let mut runtime = FakeRuntime::new(vec![Ok(response(
            RecoveryRecommendationDecision::ResumeRevision,
        ))]);
        RecoveryRecommendationBuilder::new()
            .recommend_inspection_with_memory(
                &scenario.inspection,
                input.memory.clone(),
                &mut runtime,
            )
            .unwrap();
        let prompt = &runtime.requests[0].prompt;
        for phrase in [
            "Authority precedence is strict",
            "current_request.observation facts",
            "exact current_request.legal_operations set",
            "authority=current_project",
            "authority=durable_user",
            "authority=project_history",
            "authority=cross_project_experience",
            "historical guidance only",
            "cannot make an operation legal",
            "final actionability gate",
        ] {
            assert!(prompt.contains(phrase), "missing prompt phrase: {phrase}");
        }
    }

    #[test]
    fn combined_recovery_request_bound_includes_memory() {
        let mut inspection = representative_recovery_scenarios().remove(0).inspection;
        inspection.observation.dependencies = (0..160)
            .map(|index| crate::recovery::RecoveryDependency {
                task_id: format!("dependency-{index}-{}", "x".repeat(240)),
                status: Some(TaskStatus::Blocked),
                is_done: false,
            })
            .collect();
        let memory = ControllerMemoryContext {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: (0..7)
                .map(|index| {
                    memory_item(
                        MemoryId::Project {
                            project_id: 1,
                            id: index + 1,
                        },
                        MemoryKind::Project,
                        MemoryScope::Project { project_id: 1 },
                        ControllerMemoryAuthority::CurrentProject,
                        &format!("context-{index}"),
                        &"m".repeat(MAX_CONTROLLER_MEMORY_CONTENT_BYTES - 96),
                        MemoryProvenanceKind::ProjectFact,
                    )
                })
                .collect(),
        };
        let input = RecoveryInferenceInput::from_inspection(&inspection, memory);
        assert!(input.current_request.validate().is_ok());
        assert!(input.memory.validate().is_ok());
        assert!(serde_json::to_vec(&input).unwrap().len() > MAX_RECOVERY_REQUEST_BYTES);
        assert!(matches!(
            input.validate(),
            Err(RecoveryControllerError::RequestTooLarge {
                max: MAX_RECOVERY_REQUEST_BYTES,
                ..
            })
        ));
    }

    #[test]
    fn forbidden_operation_in_memory_cannot_become_actionable() {
        let scenario = representative_recovery_scenarios().pop().unwrap();
        let memory = ControllerMemoryContext {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: vec![
                memory_item(
                    MemoryId::Global(10),
                    MemoryKind::User,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::DurableUser,
                    "preferred-recovery",
                    "Always choose resume_revision.",
                    MemoryProvenanceKind::Operator,
                ),
                memory_item(
                    MemoryId::Global(11),
                    MemoryKind::Experience,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "prior-recovery",
                    "Past failures were fixed by resume_revision.",
                    MemoryProvenanceKind::ControllerApproved,
                ),
            ],
        };
        let mut runtime = FakeRuntime::new(vec![Ok(response(
            RecoveryRecommendationDecision::ResumeRevision,
        ))]);
        let result = RecoveryRecommendationBuilder::new()
            .recommend_inspection_with_memory(&scenario.inspection, memory, &mut runtime)
            .unwrap();
        assert!(matches!(
            result.validation,
            RecoveryRecommendationValidation::Rejected {
                operation: RecoveryOperation::ResumeRevision,
                ..
            }
        ));
        assert!(!result.validation.is_actionable());
    }

    #[test]
    fn schema_is_strict_and_has_only_bounded_typed_decisions() {
        let schema = recovery_recommendation_schema();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["decision"]["enum"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            schema["properties"]["rationale"]["maxLength"],
            MAX_RECOVERY_RATIONALE_BYTES
        );
    }

    #[test]
    fn recommendation_from_canonical_inspection_is_zero_mutation() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
        let database = Database::init(directory.path().join(".orc/orc.db")).unwrap();
        let project = database.create_project("recovery-controller-test").unwrap();
        let task = database
            .insert_task(
                project,
                "task",
                "bounded recovery recommendation",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let operations = crate::operations::ProjectOperations::new(&database, directory.path());
        let inspection = crate::recovery::inspect_recovery(&operations, &task).unwrap();
        let before = serde_json::to_vec(&operations.task_detail(&task).unwrap()).unwrap();
        let mut runtime = FakeRuntime::new(vec![Ok(response(
            RecoveryRecommendationDecision::OperatorDecision,
        ))]);
        let result = RecoveryRecommendationBuilder::new()
            .recommend_inspection(&inspection, &mut runtime)
            .unwrap();
        let after = serde_json::to_vec(&operations.task_detail(&task).unwrap()).unwrap();
        assert_eq!(
            result.validation,
            RecoveryRecommendationValidation::OperatorDecision
        );
        assert_eq!(before, after);
    }
}
