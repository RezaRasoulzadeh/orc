//! Deterministic, model-independent evaluation of the complete Controller
//! judgment surface.
//!
//! This is an evaluation boundary, not a curation or execution boundary. It
//! contains explicit typed fixtures, calls the existing capability builders,
//! and records each scenario failure so one bad response cannot hide the
//! remaining results.

use crate::controller::{
    ControllerError, ControllerRecommendation, ControllerRecommendationInput,
    ControllerStateBuilder,
};
use crate::controller_evaluation::ControllerDecision;
use crate::controller_experience_intake::CONTROLLER_WORKFLOW_INTAKE_EXPERIENCE_CAPABILITY;
use crate::controller_experience_memory_capture::CONTROLLER_MEMORY_CAPTURE_EXPERIENCE_CAPABILITY;
use crate::controller_experience_memory_maintenance::CONTROLLER_MEMORY_MAINTENANCE_EXPERIENCE_CAPABILITY;
use crate::controller_experience_memory_selection::CONTROLLER_MEMORY_SELECTION_EXPERIENCE_CAPABILITY;
use crate::controller_experience_plan_review::CONTROLLER_PLAN_REVIEW_EXPERIENCE_CAPABILITY;
use crate::controller_experience_plan_revision::CONTROLLER_PLAN_REVISION_EXPERIENCE_CAPABILITY;
use crate::controller_experience_planning::CONTROLLER_PLAN_GENERATION_EXPERIENCE_CAPABILITY;
use crate::controller_experience_recommendation::CONTROLLER_RECOMMENDATION_EXPERIENCE_CAPABILITY;
use crate::controller_experience_recovery::CONTROLLER_RECOVERY_RECOMMENDATION_EXPERIENCE_CAPABILITY;
use crate::controller_intake::{
    ControllerIntakeBuilder, ControllerIntakeDecision, ControllerIntakeError,
    ControllerIntakeInput, ControllerIntakeRequest, ControllerIntakeResult, ControllerIntakeState,
};
use crate::controller_memory::ControllerMemoryContext;
use crate::controller_memory_capture::{
    ControllerMemoryCaptureBuilder, ControllerMemoryCaptureCandidate, ControllerMemoryCaptureError,
    ControllerMemoryCaptureInput, ControllerMemoryCaptureRequest, ControllerMemoryCaptureResult,
};
use crate::controller_memory_maintenance::{
    ControllerMemoryMaintenanceBuilder, ControllerMemoryMaintenanceError,
    ControllerMemoryMaintenanceInput, ControllerMemoryMaintenanceRequest,
    ControllerMemoryMaintenanceResult,
};
use crate::controller_memory_mutation::ControllerMemoryMutationIntent;
use crate::controller_memory_selection::{
    ControllerMemorySelectionBuilder, ControllerMemorySelectionCandidate,
    ControllerMemorySelectionError, ControllerMemorySelectionInput,
    ControllerMemorySelectionRequest, ControllerMemorySelectionResult,
};
use crate::controller_plan_review::{
    ControllerPlanReviewBuilder, ControllerPlanReviewDecision, ControllerPlanReviewError,
    ControllerPlanReviewInput, ControllerPlanReviewRequest, ControllerPlanReviewResult,
    ControllerPlanReviewState,
};
use crate::controller_plan_revision::{
    ControllerPlanRevisionBuilder, ControllerPlanRevisionError, ControllerPlanRevisionInput,
    ControllerPlanRevisionRequest,
};
use crate::controller_planning::{
    ControllerPlanResult, ControllerPlanningError, ControllerPlanningInput,
    ControllerPlanningRequest,
};
use crate::local_runtime::{LocalInferenceResponse, LocalInferenceRuntime};
use crate::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
    MemoryRecord, MemoryScope,
};
use crate::protocol::{
    ExecutionHints, PROTOCOL_VERSION, PlanResponse, PlanResponseSchema, TaskProposal,
};
use crate::recovery::RecoveryInspection;
use crate::recovery::{RecoveryOperation, RecoveryOperationLegality};
use crate::recovery_controller::{
    RecoveryControllerError, RecoveryInferenceInput, RecoveryRecommendation,
    RecoveryRecommendationBuilder, RecoveryRecommendationDecision,
    RecoveryRecommendationValidation, representative_recovery_scenarios, validate_recommendation,
};
use crate::storage::db::{PlanOrigin, PlanStatus};
use crate::task::TaskPriority;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION: u32 = 1;
pub const MAX_CONTROLLER_SPECIALIZATION_SCENARIOS: usize = 64;
pub const MAX_CONTROLLER_SPECIALIZATION_ALTERNATIVES: usize = 8;
const MAX_SCENARIO_ID_BYTES: usize = 256;
const MAX_SCENARIO_DESCRIPTION_BYTES: usize = 1024;

pub const CONTROLLER_SPECIALIZATION_CAPABILITIES: [&str; 9] = [
    CONTROLLER_RECOMMENDATION_EXPERIENCE_CAPABILITY,
    CONTROLLER_RECOVERY_RECOMMENDATION_EXPERIENCE_CAPABILITY,
    CONTROLLER_PLAN_GENERATION_EXPERIENCE_CAPABILITY,
    CONTROLLER_WORKFLOW_INTAKE_EXPERIENCE_CAPABILITY,
    CONTROLLER_PLAN_REVIEW_EXPERIENCE_CAPABILITY,
    CONTROLLER_PLAN_REVISION_EXPERIENCE_CAPABILITY,
    CONTROLLER_MEMORY_CAPTURE_EXPERIENCE_CAPABILITY,
    CONTROLLER_MEMORY_MAINTENANCE_EXPERIENCE_CAPABILITY,
    CONTROLLER_MEMORY_SELECTION_EXPERIENCE_CAPABILITY,
];

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "input", deny_unknown_fields)]
pub enum ControllerSpecializationInput {
    TaskRecommendation(ControllerRecommendationInput),
    Recovery(RecoveryInferenceInput),
    PlanGeneration(ControllerPlanningInput),
    WorkflowIntake(ControllerIntakeInput),
    PlanReview(ControllerPlanReviewInput),
    PlanRevision(ControllerPlanRevisionInput),
    MemoryCapture(ControllerMemoryCaptureInput),
    MemoryMaintenance(ControllerMemoryMaintenanceInput),
    MemorySelection(ControllerMemorySelectionInput),
}

impl ControllerSpecializationInput {
    pub fn capability(&self) -> &'static str {
        match self {
            Self::TaskRecommendation(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[0],
            Self::Recovery(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[1],
            Self::PlanGeneration(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[2],
            Self::WorkflowIntake(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[3],
            Self::PlanReview(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[4],
            Self::PlanRevision(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[5],
            Self::MemoryCapture(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[6],
            Self::MemoryMaintenance(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[7],
            Self::MemorySelection(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[8],
        }
    }

    fn validate(&self) -> Result<(), ControllerSpecializationEvaluationError> {
        match self {
            Self::TaskRecommendation(input) => input
                .validate()
                .map_err(|error| invalid_input(self.capability(), error)),
            Self::Recovery(input) => input
                .validate()
                .map(|_| ())
                .map_err(|error| invalid_input(self.capability(), error)),
            Self::PlanGeneration(input) => input
                .validate()
                .map_err(|error| invalid_input(self.capability(), error)),
            Self::WorkflowIntake(input) => input
                .validate()
                .map_err(|error| invalid_input(self.capability(), error)),
            Self::PlanReview(input) => input
                .validate()
                .map_err(|error| invalid_input(self.capability(), error)),
            Self::PlanRevision(input) => input
                .validate()
                .map_err(|error| invalid_input(self.capability(), error)),
            Self::MemoryCapture(input) => input
                .validate()
                .map_err(|error| invalid_input(self.capability(), error)),
            Self::MemoryMaintenance(input) => input
                .validate()
                .map_err(|error| invalid_input(self.capability(), error)),
            Self::MemorySelection(input) => input
                .validate()
                .map_err(|error| invalid_input(self.capability(), error)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "output", deny_unknown_fields)]
pub enum ControllerSpecializationOutput {
    TaskRecommendation(ControllerRecommendation),
    Recovery(RecoveryRecommendation),
    PlanGeneration(ControllerPlanResult),
    WorkflowIntake(ControllerIntakeResult),
    PlanReview(ControllerPlanReviewResult),
    PlanRevision(PlanResponse),
    MemoryCapture(ControllerMemoryCaptureResult),
    MemoryMaintenance(ControllerMemoryMaintenanceResult),
    MemorySelection(ControllerMemorySelectionResult),
}

impl ControllerSpecializationOutput {
    fn capability(&self) -> &'static str {
        match self {
            Self::TaskRecommendation(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[0],
            Self::Recovery(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[1],
            Self::PlanGeneration(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[2],
            Self::WorkflowIntake(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[3],
            Self::PlanReview(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[4],
            Self::PlanRevision(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[5],
            Self::MemoryCapture(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[6],
            Self::MemoryMaintenance(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[7],
            Self::MemorySelection(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[8],
        }
    }

    fn validate(
        &self,
        input: &ControllerSpecializationInput,
    ) -> Result<(), ControllerSpecializationEvaluationError> {
        match (input, self) {
            (
                ControllerSpecializationInput::TaskRecommendation(_),
                Self::TaskRecommendation(output),
            ) => output
                .validate()
                .map_err(|error| invalid_output(self.capability(), error)),
            (ControllerSpecializationInput::Recovery(_), Self::Recovery(output)) => output
                .validate()
                .map_err(|error| invalid_output(self.capability(), error)),
            (ControllerSpecializationInput::PlanGeneration(_), Self::PlanGeneration(output)) => {
                output
                    .validate()
                    .map_err(|error| invalid_output(self.capability(), error))
            }
            (ControllerSpecializationInput::WorkflowIntake(_), Self::WorkflowIntake(output)) => {
                output
                    .validate()
                    .map_err(|error| invalid_output(self.capability(), error))
            }
            (ControllerSpecializationInput::PlanReview(_), Self::PlanReview(output)) => output
                .validate()
                .map_err(|error| invalid_output(self.capability(), error)),
            (ControllerSpecializationInput::PlanRevision(_), Self::PlanRevision(output)) => output
                .validate()
                .map_err(|error| invalid_output(self.capability(), error)),
            (ControllerSpecializationInput::MemoryCapture(input), Self::MemoryCapture(output)) => {
                output
                    .validate(&input.current_request.candidate)
                    .map_err(|error| invalid_output(self.capability(), error))
            }
            (
                ControllerSpecializationInput::MemoryMaintenance(input),
                Self::MemoryMaintenance(output),
            ) => output
                .validate(input)
                .map_err(|error| invalid_output(self.capability(), error)),
            (
                ControllerSpecializationInput::MemorySelection(input),
                Self::MemorySelection(output),
            ) => output
                .validate(input)
                .map_err(|error| invalid_output(self.capability(), error)),
            _ => Err(ControllerSpecializationEvaluationError::VariantMismatch {
                capability: input.capability().into(),
                field: "output".into(),
            }),
        }
    }

    /// Convert a typed fixture into the structured response consumed by the
    /// existing production parser. This is a test/runtime seam only.
    pub fn fake_runtime_response(
        &self,
    ) -> Result<LocalInferenceResponse, ControllerSpecializationEvaluationError> {
        let value = match self {
            Self::TaskRecommendation(output) => {
                output.structured_output.clone().ok_or_else(|| {
                    ControllerSpecializationEvaluationError::InvalidExpected {
                        capability: self.capability().into(),
                        reason: "recommendation has no structured output".into(),
                    }
                })?
            }
            _ => self.output_value(),
        };
        let text = serde_json::to_string(&value).map_err(|error| {
            ControllerSpecializationEvaluationError::Projection(error.to_string())
        })?;
        Ok(LocalInferenceResponse::structured(text, value))
    }

    fn output_value(&self) -> serde_json::Value {
        match self {
            Self::TaskRecommendation(output) => serde_json::to_value(output).unwrap_or_default(),
            Self::Recovery(output) => serde_json::to_value(output).unwrap_or_default(),
            Self::PlanGeneration(output) => serde_json::to_value(output).unwrap_or_default(),
            Self::WorkflowIntake(output) => serde_json::to_value(output).unwrap_or_default(),
            Self::PlanReview(output) => serde_json::to_value(output).unwrap_or_default(),
            Self::PlanRevision(output) => serde_json::to_value(output).unwrap_or_default(),
            Self::MemoryCapture(output) => serde_json::to_value(output).unwrap_or_default(),
            Self::MemoryMaintenance(output) => serde_json::to_value(output).unwrap_or_default(),
            Self::MemorySelection(output) => serde_json::to_value(output).unwrap_or_default(),
        }
    }

    fn semantic(
        &self,
        input: &ControllerSpecializationInput,
    ) -> Result<ControllerSpecializationSemanticResult, ControllerSpecializationEvaluationError>
    {
        self.validate(input)?;
        match (input, self) {
            (
                ControllerSpecializationInput::TaskRecommendation(_),
                Self::TaskRecommendation(output),
            ) => Ok(ControllerSpecializationSemanticResult::TaskRecommendation {
                decision: ControllerDecision::from_recommendation(output),
            }),
            (ControllerSpecializationInput::Recovery(input), Self::Recovery(output)) => {
                Ok(ControllerSpecializationSemanticResult::Recovery {
                    decision: output.decision,
                    validation: validate_recommendation(&recovery_inspection(input), output),
                })
            }
            (ControllerSpecializationInput::PlanGeneration(_), Self::PlanGeneration(output)) => {
                Ok(ControllerSpecializationSemanticResult::PlanGeneration {
                    plan: output.plan.clone(),
                })
            }
            (ControllerSpecializationInput::WorkflowIntake(_), Self::WorkflowIntake(output)) => {
                Ok(ControllerSpecializationSemanticResult::WorkflowIntake {
                    decision: output.decision,
                    direct_tasks: output.direct_tasks.clone(),
                })
            }
            (ControllerSpecializationInput::PlanReview(_), Self::PlanReview(output)) => {
                Ok(ControllerSpecializationSemanticResult::PlanReview {
                    decision: output.decision,
                    revision_feedback: output.revision_feedback.clone(),
                })
            }
            (ControllerSpecializationInput::PlanRevision(_), Self::PlanRevision(output)) => {
                Ok(ControllerSpecializationSemanticResult::PlanRevision {
                    plan: output.clone(),
                })
            }
            (ControllerSpecializationInput::MemoryCapture(_), Self::MemoryCapture(output)) => Ok(
                ControllerSpecializationSemanticResult::MemoryCapture(output.clone()),
            ),
            (
                ControllerSpecializationInput::MemoryMaintenance(_),
                Self::MemoryMaintenance(output),
            ) => Ok(ControllerSpecializationSemanticResult::MemoryMaintenance(
                output.clone(),
            )),
            (ControllerSpecializationInput::MemorySelection(_), Self::MemorySelection(output)) => {
                Ok(ControllerSpecializationSemanticResult::MemorySelection(
                    output.clone(),
                ))
            }
            _ => Err(ControllerSpecializationEvaluationError::VariantMismatch {
                capability: input.capability().into(),
                field: "output".into(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ControllerSpecializationSemanticResult {
    TaskRecommendation {
        decision: ControllerDecision,
    },
    Recovery {
        decision: RecoveryRecommendationDecision,
        validation: RecoveryRecommendationValidation,
    },
    PlanGeneration {
        plan: PlanResponse,
    },
    WorkflowIntake {
        decision: ControllerIntakeDecision,
        direct_tasks: Vec<TaskProposal>,
    },
    PlanReview {
        decision: ControllerPlanReviewDecision,
        revision_feedback: Option<String>,
    },
    PlanRevision {
        plan: PlanResponse,
    },
    MemoryCapture(ControllerMemoryCaptureResult),
    MemoryMaintenance(ControllerMemoryMaintenanceResult),
    MemorySelection(ControllerMemorySelectionResult),
}

impl ControllerSpecializationSemanticResult {
    fn capability(&self) -> &'static str {
        match self {
            Self::TaskRecommendation { .. } => CONTROLLER_SPECIALIZATION_CAPABILITIES[0],
            Self::Recovery { .. } => CONTROLLER_SPECIALIZATION_CAPABILITIES[1],
            Self::PlanGeneration { .. } => CONTROLLER_SPECIALIZATION_CAPABILITIES[2],
            Self::WorkflowIntake { .. } => CONTROLLER_SPECIALIZATION_CAPABILITIES[3],
            Self::PlanReview { .. } => CONTROLLER_SPECIALIZATION_CAPABILITIES[4],
            Self::PlanRevision { .. } => CONTROLLER_SPECIALIZATION_CAPABILITIES[5],
            Self::MemoryCapture(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[6],
            Self::MemoryMaintenance(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[7],
            Self::MemorySelection(_) => CONTROLLER_SPECIALIZATION_CAPABILITIES[8],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationScenario {
    pub id: String,
    pub description: String,
    pub capability: String,
    pub input: ControllerSpecializationInput,
    pub expected: ControllerSpecializationOutput,
    pub acceptable_alternatives: Vec<ControllerSpecializationOutput>,
}

impl ControllerSpecializationScenario {
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        capability: impl Into<String>,
        input: ControllerSpecializationInput,
        expected: ControllerSpecializationOutput,
        acceptable_alternatives: Vec<ControllerSpecializationOutput>,
    ) -> Result<Self, ControllerSpecializationEvaluationError> {
        let scenario = Self {
            id: id.into(),
            description: description.into(),
            capability: capability.into(),
            input,
            expected,
            acceptable_alternatives,
        };
        scenario.validate()?;
        Ok(scenario)
    }

    pub fn validate(&self) -> Result<(), ControllerSpecializationEvaluationError> {
        if self.id.is_empty() || self.id.len() > MAX_SCENARIO_ID_BYTES {
            return Err(ControllerSpecializationEvaluationError::ScenarioBounds(
                "id".into(),
            ));
        }
        if self.description.len() > MAX_SCENARIO_DESCRIPTION_BYTES {
            return Err(ControllerSpecializationEvaluationError::ScenarioBounds(
                "description".into(),
            ));
        }
        if self.acceptable_alternatives.len() > MAX_CONTROLLER_SPECIALIZATION_ALTERNATIVES {
            return Err(ControllerSpecializationEvaluationError::ScenarioBounds(
                "acceptable_alternatives".into(),
            ));
        }
        if !CONTROLLER_SPECIALIZATION_CAPABILITIES.contains(&self.capability.as_str()) {
            return Err(ControllerSpecializationEvaluationError::UnknownCapability(
                self.capability.clone(),
            ));
        }
        if self.input.capability() != self.capability {
            return Err(
                ControllerSpecializationEvaluationError::CapabilityMismatch {
                    declared: self.capability.clone(),
                    actual: self.input.capability().into(),
                },
            );
        }
        self.input.validate()?;
        self.validate_output(&self.expected, "expected")?;
        for alternative in &self.acceptable_alternatives {
            self.validate_output(alternative, "acceptable_alternative")?;
        }
        Ok(())
    }

    fn validate_output(
        &self,
        output: &ControllerSpecializationOutput,
        field: &str,
    ) -> Result<(), ControllerSpecializationEvaluationError> {
        if output.capability() != self.capability {
            return Err(ControllerSpecializationEvaluationError::VariantMismatch {
                capability: self.capability.clone(),
                field: field.into(),
            });
        }
        output.validate(&self.input).map_err(|error| {
            if field == "expected" {
                ControllerSpecializationEvaluationError::InvalidExpected {
                    capability: self.capability.clone(),
                    reason: error.to_string(),
                }
            } else {
                ControllerSpecializationEvaluationError::InvalidAlternative {
                    capability: self.capability.clone(),
                    reason: error.to_string(),
                }
            }
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationSuite {
    pub schema_version: u32,
    pub scenarios: Vec<ControllerSpecializationScenario>,
}

pub type ControllerSpecializationEvaluationSuite = ControllerSpecializationSuite;

impl ControllerSpecializationSuite {
    pub fn new(
        mut scenarios: Vec<ControllerSpecializationScenario>,
    ) -> Result<Self, ControllerSpecializationEvaluationError> {
        scenarios.sort_by(|left, right| left.id.cmp(&right.id));
        let suite = Self {
            schema_version: CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION,
            scenarios,
        };
        suite.validate()?;
        Ok(suite)
    }

    pub fn validate(&self) -> Result<(), ControllerSpecializationEvaluationError> {
        if self.schema_version != CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION {
            return Err(
                ControllerSpecializationEvaluationError::UnsupportedSchemaVersion(
                    self.schema_version,
                ),
            );
        }
        if self.scenarios.len() > MAX_CONTROLLER_SPECIALIZATION_SCENARIOS {
            return Err(ControllerSpecializationEvaluationError::TooManyScenarios {
                actual: self.scenarios.len(),
                max: MAX_CONTROLLER_SPECIALIZATION_SCENARIOS,
            });
        }
        for window in self.scenarios.windows(2) {
            if window[0].id >= window[1].id {
                return Err(
                    ControllerSpecializationEvaluationError::DuplicateOrUnorderedScenario(
                        window[1].id.clone(),
                    ),
                );
            }
        }
        for scenario in &self.scenarios {
            scenario.validate()?;
        }
        Ok(())
    }

    pub fn representative_suite() -> Result<Self, ControllerSpecializationEvaluationError> {
        let recovery_scenarios = representative_recovery_scenarios();
        let recovery_scenario = recovery_scenarios.first().cloned().ok_or_else(|| {
            ControllerSpecializationEvaluationError::InvalidExpected {
                capability: CONTROLLER_SPECIALIZATION_CAPABILITIES[1].into(),
                reason: "the production recovery corpus is empty".into(),
            }
        })?;
        let additional_recovery_scenario = recovery_scenarios.get(1).cloned().ok_or_else(|| {
            ControllerSpecializationEvaluationError::InvalidExpected {
                capability: CONTROLLER_SPECIALIZATION_CAPABILITIES[1].into(),
                reason: "the production recovery corpus has no additional decision scenario".into(),
            }
        })?;
        let mut multi_legal_recovery = additional_recovery_scenario.inspection.clone();
        multi_legal_recovery.operations = vec![
            RecoveryOperationLegality::Allowed {
                operation: RecoveryOperation::AcknowledgeNonConvergence,
            },
            RecoveryOperationLegality::Allowed {
                operation: RecoveryOperation::Requeue,
            },
        ];
        let recovery_input = RecoveryInferenceInput::from_inspection(
            &recovery_scenario.inspection,
            ControllerMemoryContext::empty(),
        );
        let multi_legal_recovery_input = RecoveryInferenceInput::from_inspection(
            &multi_legal_recovery,
            ControllerMemoryContext::empty(),
        );
        let recovery_output = RecoveryRecommendation {
            decision: recovery_scenario.expected,
            rationale: "explicit evaluation fixture".into(),
            confidence: Some(1.0),
        };
        let packet_scenarios =
            crate::controller_evaluation::representative_scenarios().map_err(|error| {
                ControllerSpecializationEvaluationError::InvalidExpected {
                    capability: CONTROLLER_SPECIALIZATION_CAPABILITIES[0].into(),
                    reason: error.to_string(),
                }
            })?;
        let packet_scenario = packet_scenarios.first().ok_or_else(|| {
            ControllerSpecializationEvaluationError::InvalidExpected {
                capability: CONTROLLER_SPECIALIZATION_CAPABILITIES[0].into(),
                reason: "the production M02 corpus is empty".into(),
            }
        })?;
        let second_packet_scenario = packet_scenarios.get(1).ok_or_else(|| {
            ControllerSpecializationEvaluationError::InvalidExpected {
                capability: CONTROLLER_SPECIALIZATION_CAPABILITIES[0].into(),
                reason: "the production M02 corpus has no second operational scenario".into(),
            }
        })?;
        let recommendation = recommendation_fixture(packet_scenario);
        let second_recommendation = recommendation_fixture(second_packet_scenario);
        let planning_request = planning_request_fixture();
        let plan = empty_plan("evaluation objective");
        let planning_input = ControllerPlanningInput {
            current_request: planning_request.clone(),
            memory: ControllerMemoryContext::empty(),
        };
        let intake_input = intake_input_fixture();
        let direct_tasks_input = intake_input.clone();
        let review_input = review_input_fixture(plan.clone());
        let revision_input = ControllerPlanRevisionInput {
            current_request: ControllerPlanRevisionRequest {
                packet_version:
                    crate::controller_plan_revision::CONTROLLER_PLAN_REVISION_REQUEST_VERSION,
                plan: plan.clone(),
                revision_feedback: "Add the missing implementation detail.".into(),
                planning_context: planning_request,
            },
            memory: ControllerMemoryContext::empty(),
        };
        let capture_input = capture_input_fixture();
        let maintenance_input = maintenance_input_fixture();
        let selection_input = selection_input_fixture();
        let no_target_selection_input = selection_input_no_target_fixture();
        let capture_proposal = ControllerMemoryCaptureResult::ProposeMutation {
            intent: ControllerMemoryMutationIntent::Create {
                draft: capture_input.current_request.candidate.draft.clone(),
            },
        };
        let maintenance_proposal = ControllerMemoryMaintenanceResult::ProposeMutation {
            intent: ControllerMemoryMutationIntent::Remove {
                target: maintenance_input.target.id.clone(),
            },
        };
        Self::new(vec![
            scenario(
                "controller-task-recommendation",
                ControllerSpecializationInput::TaskRecommendation(
                    ControllerRecommendationInput::from_packet(
                        &packet_scenario.packet,
                        ControllerMemoryContext::empty(),
                    ),
                ),
                ControllerSpecializationOutput::TaskRecommendation(recommendation),
            )?,
            scenario(
                "controller-task-recommendation-accept",
                ControllerSpecializationInput::TaskRecommendation(
                    ControllerRecommendationInput::from_packet(
                        &second_packet_scenario.packet,
                        ControllerMemoryContext::empty(),
                    ),
                ),
                ControllerSpecializationOutput::TaskRecommendation(second_recommendation),
            )?,
            scenario(
                "controller-recovery-recommendation",
                ControllerSpecializationInput::Recovery(recovery_input),
                ControllerSpecializationOutput::Recovery(recovery_output),
            )?,
            scenario(
                "controller-recovery-recommendation-multi-legal",
                ControllerSpecializationInput::Recovery(multi_legal_recovery_input),
                ControllerSpecializationOutput::Recovery(RecoveryRecommendation {
                    decision: RecoveryRecommendationDecision::AcknowledgeNonConvergence,
                    rationale: "explicit evaluation fixture".into(),
                    confidence: Some(1.0),
                }),
            )?,
            scenario(
                "controller-plan-generation",
                ControllerSpecializationInput::PlanGeneration(planning_input),
                ControllerSpecializationOutput::PlanGeneration(ControllerPlanResult {
                    plan: plan.clone(),
                    rationale: "explicit evaluation fixture".into(),
                    uncertainty: None,
                }),
            )?,
            scenario(
                "controller-workflow-intake",
                ControllerSpecializationInput::WorkflowIntake(intake_input),
                ControllerSpecializationOutput::WorkflowIntake(ControllerIntakeResult {
                    decision: ControllerIntakeDecision::PlanRequired,
                    details: "explicit evaluation fixture".into(),
                    direct_tasks: Vec::new(),
                }),
            )?,
            scenario(
                "controller-workflow-intake-direct-tasks",
                ControllerSpecializationInput::WorkflowIntake(direct_tasks_input),
                ControllerSpecializationOutput::WorkflowIntake(ControllerIntakeResult {
                    decision: ControllerIntakeDecision::DirectTasks,
                    details: "explicit direct task fixture".into(),
                    direct_tasks: vec![direct_task_fixture()],
                }),
            )?,
            scenario(
                "controller-plan-review",
                ControllerSpecializationInput::PlanReview(review_input.clone()),
                ControllerSpecializationOutput::PlanReview(ControllerPlanReviewResult {
                    decision: ControllerPlanReviewDecision::Approve,
                    details: "explicit evaluation fixture".into(),
                    revision_feedback: None,
                }),
            )?,
            scenario(
                "controller-plan-review-revise",
                ControllerSpecializationInput::PlanReview(review_input.clone()),
                ControllerSpecializationOutput::PlanReview(ControllerPlanReviewResult {
                    decision: ControllerPlanReviewDecision::RevisePlan,
                    details: "explicit revision fixture".into(),
                    revision_feedback: Some("Add the missing implementation detail.".into()),
                }),
            )?,
            scenario(
                "controller-plan-review-operator",
                ControllerSpecializationInput::PlanReview(review_input),
                ControllerSpecializationOutput::PlanReview(ControllerPlanReviewResult {
                    decision: ControllerPlanReviewDecision::OperatorDecisionRequired,
                    details: "explicit operator fixture".into(),
                    revision_feedback: None,
                }),
            )?,
            scenario(
                "controller-plan-revision",
                ControllerSpecializationInput::PlanRevision(revision_input),
                ControllerSpecializationOutput::PlanRevision(plan.clone()),
            )?,
            scenario(
                "controller-memory-capture",
                ControllerSpecializationInput::MemoryCapture(capture_input.clone()),
                ControllerSpecializationOutput::MemoryCapture(
                    ControllerMemoryCaptureResult::Ignore,
                ),
            )?,
            scenario(
                "controller-memory-capture-propose",
                ControllerSpecializationInput::MemoryCapture(capture_input),
                ControllerSpecializationOutput::MemoryCapture(capture_proposal),
            )?,
            scenario(
                "controller-memory-maintenance",
                ControllerSpecializationInput::MemoryMaintenance(maintenance_input.clone()),
                ControllerSpecializationOutput::MemoryMaintenance(
                    ControllerMemoryMaintenanceResult::Keep,
                ),
            )?,
            scenario(
                "controller-memory-maintenance-propose",
                ControllerSpecializationInput::MemoryMaintenance(maintenance_input),
                ControllerSpecializationOutput::MemoryMaintenance(maintenance_proposal),
            )?,
            scenario(
                "controller-memory-selection",
                ControllerSpecializationInput::MemorySelection(selection_input.clone()),
                ControllerSpecializationOutput::MemorySelection(
                    ControllerMemorySelectionResult::SelectTarget {
                        target: selection_input.candidates[0].id.clone(),
                    },
                ),
            )?,
            scenario(
                "controller-memory-selection-no-target",
                ControllerSpecializationInput::MemorySelection(no_target_selection_input),
                ControllerSpecializationOutput::MemorySelection(
                    ControllerMemorySelectionResult::NoTarget,
                ),
            )?,
        ])
    }
}

fn scenario(
    id: &str,
    input: ControllerSpecializationInput,
    expected: ControllerSpecializationOutput,
) -> Result<ControllerSpecializationScenario, ControllerSpecializationEvaluationError> {
    ControllerSpecializationScenario::new(
        id,
        "explicit deterministic representative",
        input.capability(),
        input,
        expected,
        Vec::new(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSpecializationScenarioStatus {
    Pass,
    IncorrectResult,
    ObservedValidationFailure,
    RuntimeFailure,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationScenarioReport {
    pub scenario_id: String,
    pub capability: String,
    pub expected: ControllerSpecializationSemanticResult,
    pub acceptable_alternatives: Vec<ControllerSpecializationSemanticResult>,
    pub observed: Option<ControllerSpecializationSemanticResult>,
    pub status: ControllerSpecializationScenarioStatus,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationAggregate {
    pub total: u64,
    pub passed: u64,
    pub incorrect: u64,
    pub validation_failures: u64,
    pub runtime_failures: u64,
}

impl ControllerSpecializationAggregate {
    fn add(&mut self, status: ControllerSpecializationScenarioStatus) {
        self.total += 1;
        match status {
            ControllerSpecializationScenarioStatus::Pass => self.passed += 1,
            ControllerSpecializationScenarioStatus::IncorrectResult => self.incorrect += 1,
            ControllerSpecializationScenarioStatus::ObservedValidationFailure => {
                self.validation_failures += 1
            }
            ControllerSpecializationScenarioStatus::RuntimeFailure => self.runtime_failures += 1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationCapabilityAggregate {
    pub capability: String,
    pub total: u64,
    pub passed: u64,
    pub incorrect: u64,
    pub validation_failures: u64,
    pub runtime_failures: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationEvaluationReport {
    pub schema_version: u32,
    pub scenarios: Vec<ControllerSpecializationScenarioReport>,
    pub aggregate: ControllerSpecializationAggregate,
    pub capabilities: Vec<ControllerSpecializationCapabilityAggregate>,
}

impl ControllerSpecializationEvaluationReport {
    pub fn validate(&self) -> Result<(), ControllerSpecializationEvaluationError> {
        if self.schema_version != CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION {
            return Err(
                ControllerSpecializationEvaluationError::UnsupportedSchemaVersion(
                    self.schema_version,
                ),
            );
        }
        let mut aggregate = ControllerSpecializationAggregate::default();
        let mut expected_capabilities =
            BTreeMap::<String, ControllerSpecializationCapabilityAggregate>::new();
        let mut previous_id: Option<&str> = None;
        for scenario in &self.scenarios {
            if scenario.scenario_id.is_empty() || scenario.scenario_id.len() > MAX_SCENARIO_ID_BYTES
            {
                return Err(ControllerSpecializationEvaluationError::InvalidReport(
                    "scenario report ID is empty or oversized".into(),
                ));
            }
            if previous_id.is_some_and(|previous| previous >= scenario.scenario_id.as_str()) {
                return Err(ControllerSpecializationEvaluationError::InvalidReport(
                    "scenario reports must be strictly ordered and unique".into(),
                ));
            }
            previous_id = Some(&scenario.scenario_id);
            if !CONTROLLER_SPECIALIZATION_CAPABILITIES.contains(&scenario.capability.as_str()) {
                return Err(ControllerSpecializationEvaluationError::UnknownCapability(
                    scenario.capability.clone(),
                ));
            }
            if scenario.expected.capability() != scenario.capability
                || scenario
                    .acceptable_alternatives
                    .iter()
                    .any(|value| value.capability() != scenario.capability)
                || scenario
                    .observed
                    .as_ref()
                    .is_some_and(|value| value.capability() != scenario.capability)
            {
                return Err(ControllerSpecializationEvaluationError::InvalidReport(
                    "semantic capability does not match scenario capability".into(),
                ));
            }
            match scenario.status {
                ControllerSpecializationScenarioStatus::Pass
                | ControllerSpecializationScenarioStatus::IncorrectResult => {
                    if scenario.observed.is_none() || scenario.error.is_some() {
                        return Err(ControllerSpecializationEvaluationError::InvalidReport("successful or incorrect scenarios require observed semantics and no error".into()));
                    }
                }
                ControllerSpecializationScenarioStatus::ObservedValidationFailure
                | ControllerSpecializationScenarioStatus::RuntimeFailure => {
                    if scenario.observed.is_some()
                        || scenario.error.as_deref().is_none_or(str::is_empty)
                    {
                        return Err(ControllerSpecializationEvaluationError::InvalidReport(
                            "failed scenarios require one error and no observed semantics".into(),
                        ));
                    }
                }
            }
            aggregate.add(scenario.status);
            let entry = expected_capabilities
                .entry(scenario.capability.clone())
                .or_insert_with(|| ControllerSpecializationCapabilityAggregate {
                    capability: scenario.capability.clone(),
                    ..Default::default()
                });
            entry.total += 1;
            match scenario.status {
                ControllerSpecializationScenarioStatus::Pass => entry.passed += 1,
                ControllerSpecializationScenarioStatus::IncorrectResult => entry.incorrect += 1,
                ControllerSpecializationScenarioStatus::ObservedValidationFailure => {
                    entry.validation_failures += 1
                }
                ControllerSpecializationScenarioStatus::RuntimeFailure => {
                    entry.runtime_failures += 1
                }
            }
        }
        if self.aggregate != aggregate
            || self.capabilities != expected_capabilities.values().cloned().collect::<Vec<_>>()
        {
            return Err(ControllerSpecializationEvaluationError::InvalidReport(
                "aggregate accounting does not equal scenario results".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ControllerSpecializationEvaluationError {
    #[error("unsupported evaluation schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("evaluation contains {actual} scenarios; maximum is {max}")]
    TooManyScenarios { actual: usize, max: usize },
    #[error("scenario metadata exceeds its {0} bound")]
    ScenarioBounds(String),
    #[error("duplicate or unordered scenario ID `{0}`")]
    DuplicateOrUnorderedScenario(String),
    #[error("unknown Controller capability `{0}`")]
    UnknownCapability(String),
    #[error("declared capability `{declared}` does not match `{actual}`")]
    CapabilityMismatch { declared: String, actual: String },
    #[error("{capability} input validation failed: {error}")]
    InvalidInput { capability: String, error: String },
    #[error("{capability} output validation failed: {error}")]
    InvalidOutput { capability: String, error: String },
    #[error("{capability} expected output is invalid: {reason}")]
    InvalidExpected { capability: String, reason: String },
    #[error("{capability} acceptable alternative is invalid: {reason}")]
    InvalidAlternative { capability: String, reason: String },
    #[error("{capability} has a mismatched {field} variant")]
    VariantMismatch { capability: String, field: String },
    #[error("evaluation projection failed: {0}")]
    Projection(String),
    #[error("invalid evaluation report: {0}")]
    InvalidReport(String),
}

fn invalid_input(
    capability: &str,
    error: impl ToString,
) -> ControllerSpecializationEvaluationError {
    ControllerSpecializationEvaluationError::InvalidInput {
        capability: capability.into(),
        error: error.to_string(),
    }
}
fn invalid_output(
    capability: &str,
    error: impl ToString,
) -> ControllerSpecializationEvaluationError {
    ControllerSpecializationEvaluationError::InvalidOutput {
        capability: capability.into(),
        error: error.to_string(),
    }
}

enum ScenarioFailure {
    Validation(String),
    Runtime(String),
}

pub fn evaluate_controller_specialization(
    suite: &ControllerSpecializationSuite,
    runtime: &mut dyn LocalInferenceRuntime,
) -> Result<ControllerSpecializationEvaluationReport, ControllerSpecializationEvaluationError> {
    suite.validate()?;
    let mut reports = Vec::with_capacity(suite.scenarios.len());
    for scenario in &suite.scenarios {
        let expected = scenario.expected.semantic(&scenario.input)?;
        let alternatives = scenario
            .acceptable_alternatives
            .iter()
            .map(|value| value.semantic(&scenario.input))
            .collect::<Result<Vec<_>, _>>()?;
        match execute_scenario(&scenario.input, runtime) {
            Ok(observed) => match observed.semantic(&scenario.input) {
                Ok(observed_semantic) => {
                    let pass =
                        observed_semantic == expected || alternatives.contains(&observed_semantic);
                    reports.push(ControllerSpecializationScenarioReport {
                        scenario_id: scenario.id.clone(),
                        capability: scenario.capability.clone(),
                        expected,
                        acceptable_alternatives: alternatives,
                        observed: Some(observed_semantic),
                        status: if pass {
                            ControllerSpecializationScenarioStatus::Pass
                        } else {
                            ControllerSpecializationScenarioStatus::IncorrectResult
                        },
                        error: None,
                    });
                }
                Err(error) => reports.push(failed_report(
                    scenario,
                    expected,
                    alternatives,
                    ScenarioFailure::Validation(error.to_string()),
                )),
            },
            Err(error) => reports.push(failed_report(scenario, expected, alternatives, error)),
        }
    }
    let aggregate = reports.iter().fold(
        ControllerSpecializationAggregate::default(),
        |mut aggregate, report| {
            aggregate.add(report.status);
            aggregate
        },
    );
    let mut capabilities = BTreeMap::<String, ControllerSpecializationCapabilityAggregate>::new();
    for report in &reports {
        let entry = capabilities
            .entry(report.capability.clone())
            .or_insert_with(|| ControllerSpecializationCapabilityAggregate {
                capability: report.capability.clone(),
                ..Default::default()
            });
        entry.total += 1;
        match report.status {
            ControllerSpecializationScenarioStatus::Pass => entry.passed += 1,
            ControllerSpecializationScenarioStatus::IncorrectResult => entry.incorrect += 1,
            ControllerSpecializationScenarioStatus::ObservedValidationFailure => {
                entry.validation_failures += 1
            }
            ControllerSpecializationScenarioStatus::RuntimeFailure => entry.runtime_failures += 1,
        }
    }
    let report = ControllerSpecializationEvaluationReport {
        schema_version: CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION,
        scenarios: reports,
        aggregate,
        capabilities: capabilities.into_values().collect(),
    };
    report.validate()?;
    Ok(report)
}

pub fn evaluate_controller_specialization_suite(
    suite: &ControllerSpecializationSuite,
    runtime: &mut dyn LocalInferenceRuntime,
) -> Result<ControllerSpecializationEvaluationReport, ControllerSpecializationEvaluationError> {
    evaluate_controller_specialization(suite, runtime)
}

fn failed_report(
    scenario: &ControllerSpecializationScenario,
    expected: ControllerSpecializationSemanticResult,
    alternatives: Vec<ControllerSpecializationSemanticResult>,
    failure: ScenarioFailure,
) -> ControllerSpecializationScenarioReport {
    let (status, error) = match failure {
        ScenarioFailure::Validation(error) => (
            ControllerSpecializationScenarioStatus::ObservedValidationFailure,
            error,
        ),
        ScenarioFailure::Runtime(error) => (
            ControllerSpecializationScenarioStatus::RuntimeFailure,
            error,
        ),
    };
    ControllerSpecializationScenarioReport {
        scenario_id: scenario.id.clone(),
        capability: scenario.capability.clone(),
        expected,
        acceptable_alternatives: alternatives,
        observed: None,
        status,
        error: Some(error),
    }
}

fn execute_scenario(
    input: &ControllerSpecializationInput,
    runtime: &mut dyn LocalInferenceRuntime,
) -> Result<ControllerSpecializationOutput, ScenarioFailure> {
    match input {
        ControllerSpecializationInput::TaskRecommendation(input) => ControllerStateBuilder::new()
            .recommend_packet_with_memory(&input.current_packet, input.memory.clone(), runtime)
            .map(ControllerSpecializationOutput::TaskRecommendation)
            .map_err(|error| classify(&error, matches!(&error, ControllerError::Inference(_)))),
        ControllerSpecializationInput::Recovery(input) => RecoveryRecommendationBuilder::new()
            .recommend_inspection_with_memory(
                &recovery_inspection(input),
                input.memory.clone(),
                runtime,
            )
            .map(|result| ControllerSpecializationOutput::Recovery(result.recommendation))
            .map_err(|error| {
                classify(
                    &error,
                    matches!(&error, RecoveryControllerError::Inference(_)),
                )
            }),
        ControllerSpecializationInput::PlanGeneration(input) => {
            crate::controller_planning::ControllerPlanningBuilder::new()
                .propose_with_memory(input, runtime)
                .map(ControllerSpecializationOutput::PlanGeneration)
                .map_err(|error| {
                    classify(
                        &error,
                        matches!(&error, ControllerPlanningError::Inference(_)),
                    )
                })
        }
        ControllerSpecializationInput::WorkflowIntake(input) => ControllerIntakeBuilder::new()
            .classify_with_memory(&input.current_request, input.memory.clone(), runtime)
            .map(ControllerSpecializationOutput::WorkflowIntake)
            .map_err(|error| {
                classify(
                    &error,
                    matches!(&error, ControllerIntakeError::Inference(_)),
                )
            }),
        ControllerSpecializationInput::PlanReview(input) => ControllerPlanReviewBuilder::new()
            .review_with_memory(input, runtime)
            .map(ControllerSpecializationOutput::PlanReview)
            .map_err(|error| {
                classify(
                    &error,
                    matches!(&error, ControllerPlanReviewError::Inference(_)),
                )
            }),
        ControllerSpecializationInput::PlanRevision(input) => ControllerPlanRevisionBuilder::new()
            .revise_with_memory(input, runtime)
            .map(ControllerSpecializationOutput::PlanRevision)
            .map_err(|error| {
                classify(
                    &error,
                    matches!(&error, ControllerPlanRevisionError::Inference(_)),
                )
            }),
        ControllerSpecializationInput::MemoryCapture(input) => {
            ControllerMemoryCaptureBuilder::new()
                .capture_with_memory(input, runtime)
                .map(ControllerSpecializationOutput::MemoryCapture)
                .map_err(|error| {
                    classify(
                        &error,
                        matches!(&error, ControllerMemoryCaptureError::Inference(_)),
                    )
                })
        }
        ControllerSpecializationInput::MemoryMaintenance(input) => {
            ControllerMemoryMaintenanceBuilder::new()
                .maintain(input, runtime)
                .map(ControllerSpecializationOutput::MemoryMaintenance)
                .map_err(|error| {
                    classify(
                        &error,
                        matches!(&error, ControllerMemoryMaintenanceError::Inference(_)),
                    )
                })
        }
        ControllerSpecializationInput::MemorySelection(input) => {
            ControllerMemorySelectionBuilder::new()
                .select(input, runtime)
                .map(ControllerSpecializationOutput::MemorySelection)
                .map_err(|error| {
                    classify(
                        &error,
                        matches!(&error, ControllerMemorySelectionError::Inference(_)),
                    )
                })
        }
    }
}

fn classify(error: impl ToString, runtime: bool) -> ScenarioFailure {
    if runtime {
        ScenarioFailure::Runtime(error.to_string())
    } else {
        ScenarioFailure::Validation(error.to_string())
    }
}
fn recovery_inspection(input: &RecoveryInferenceInput) -> RecoveryInspection {
    RecoveryInspection {
        observation: input.current_request.observation.clone(),
        operations: input.current_request.legal_operations.clone(),
    }
}

fn recommendation_fixture(
    scenario: &crate::controller_evaluation::ControllerEvaluationScenario,
) -> ControllerRecommendation {
    let (next_step, decision_class) = match scenario.expected_decision {
        ControllerDecision::NextStep(next_step) => (Some(next_step), "action"),
        ControllerDecision::OperatorDecision => (None, "operator_decision"),
        ControllerDecision::Unspecified => (None, "action"),
    };
    let mut value = serde_json::Map::new();
    value.insert(
        "suggested_next_step".into(),
        serde_json::to_value(next_step).unwrap_or_default(),
    );
    value.insert("decision_class".into(), serde_json::json!(decision_class));
    value.insert(
        "rationale".into(),
        serde_json::json!("explicit evaluation fixture"),
    );
    let structured_output = serde_json::Value::Object(value);
    let response_text =
        serde_json::to_string(&structured_output).unwrap_or_else(|_| "fixture".into());
    ControllerRecommendation {
        task_id: scenario.packet.task.summary.task_id.clone(),
        response_text,
        suggested_next_step: next_step,
        rationale: "explicit evaluation fixture".into(),
        structured_output: Some(structured_output),
    }
}

fn empty_plan(objective: &str) -> PlanResponse {
    PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: objective.into(),
        assumptions: Vec::new(),
        risks: Vec::new(),
        questions: Vec::new(),
        tasks: Vec::new(),
    }
}

fn planning_request_fixture() -> ControllerPlanningRequest {
    ControllerPlanningRequest {
        packet_version: crate::controller_planning::CONTROLLER_PLANNING_REQUEST_VERSION,
        kind: "project_plan".into(),
        project_name: Some("specialization evaluation".into()),
        engineering_contract: "Keep the proposal bounded.".into(),
        objective: "evaluation objective".into(),
        constraints: Vec::new(),
        target_platforms: Vec::new(),
        stack: vec!["Rust".into()],
        non_goals: vec!["mutation".into()],
        deliverables: vec!["typed plan".into()],
        definition_of_done: vec!["validated".into()],
        response_schema: PlanResponseSchema::v1(),
        role_boundaries: vec!["Controller proposes only.".into()],
        planning_constraints: Vec::new(),
        approval_requirements: vec!["operator approval".into()],
        current_state: None,
    }
}

fn intake_input_fixture() -> ControllerIntakeInput {
    ControllerIntakeInput {
        current_request: ControllerIntakeRequest {
            packet_version: crate::controller_intake::CONTROLLER_INTAKE_REQUEST_VERSION,
            kind: "workflow_intake".into(),
            project_name: "specialization evaluation".into(),
            engineering_contract: "Keep the proposal bounded.".into(),
            objective: "classify this request".into(),
            project_facts: Vec::new(),
            discovery: crate::controller_intake::ControllerIntakeDiscovery {
                fingerprint: "evaluation".into(),
                technology_stack: vec!["Rust".into()],
                important_files: vec!["Cargo.toml".into()],
                architecture_boundaries: vec!["src".into()],
                unknowns_and_risks: Vec::new(),
                validation_commands: vec!["cargo test --lib".into()],
                state: ControllerIntakeState {
                    task_counts: Vec::new(),
                    ready_tasks: Vec::new(),
                    active_tasks: Vec::new(),
                    review_tasks: Vec::new(),
                    blocked_tasks: Vec::new(),
                },
            },
            operator_resolution: None,
        },
        memory: ControllerMemoryContext::empty(),
    }
}

fn direct_task_fixture() -> TaskProposal {
    TaskProposal {
        local_id: "inspect-project".into(),
        title: "Inspect the project".into(),
        objective: "Inspect the bounded project state.".into(),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on: Vec::new(),
        capabilities: Vec::new(),
        scope_mode: None,
        context_files: Vec::new(),
        expected_changes: vec!["inspection result".into()],
        unchanged: vec!["project behavior".into()],
        acceptance_criteria: vec!["The state is documented.".into()],
        required_tests: vec!["cargo test --lib".into()],
        validation: vec!["cargo test --lib".into()],
        execution_hints: ExecutionHints::default(),
        risk_factors: Vec::new(),
    }
}

fn review_input_fixture(plan: PlanResponse) -> ControllerPlanReviewInput {
    ControllerPlanReviewInput {
        current_request: ControllerPlanReviewRequest {
            packet_version: crate::controller_plan_review::CONTROLLER_PLAN_REVIEW_REQUEST_VERSION,
            plan_id: 1,
            plan_version: 1,
            plan_status: PlanStatus::Proposed,
            plan_origin: PlanOrigin::Controller,
            plan,
            project_name: Some("specialization evaluation".into()),
            current_state: ControllerPlanReviewState {
                task_counts: Vec::new(),
                ready_tasks: Vec::new(),
                active_tasks: Vec::new(),
                review_tasks: Vec::new(),
                blocked_tasks: Vec::new(),
                usable_agent_count: 0,
                busy_agent_count: 0,
                quota_reserve_percent: 0,
            },
            operator_resolution: None,
        },
        memory: ControllerMemoryContext::empty(),
    }
}

fn capture_input_fixture() -> ControllerMemoryCaptureInput {
    let candidate = ControllerMemoryCaptureCandidate {
        draft: MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id: 1 },
            subject: "evaluation fact".into(),
            content: "The evaluation fixture is deterministic.".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ProjectFact,
                source_reference: Some("evaluation".into()),
            },
            confidence: Some(0.9),
        },
        source_facts: vec!["Operator supplied a durable project fact.".into()],
    };
    ControllerMemoryCaptureInput::from_request(
        &ControllerMemoryCaptureRequest::from_candidate(candidate),
        ControllerMemoryContext::empty(),
    )
}

fn maintenance_input_fixture() -> ControllerMemoryMaintenanceInput {
    let target = MemoryRecord {
        id: MemoryId::Project {
            project_id: 1,
            id: 1,
        },
        kind: MemoryKind::Project,
        scope: MemoryScope::Project { project_id: 1 },
        subject: "evaluation memory".into(),
        content: "The evaluation fixture is deterministic.".into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::ProjectFact,
            source_reference: Some("evaluation".into()),
        },
        confidence: Some(0.9),
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
        created_at: "evaluation".into(),
        updated_at: "evaluation".into(),
    };
    let request = ControllerMemoryMaintenanceRequest::new(target.id.clone(), Vec::new());
    ControllerMemoryMaintenanceInput::from_resolved_target(
        &request,
        target,
        ControllerMemoryContext::empty(),
    )
}

fn selection_input_fixture() -> ControllerMemorySelectionInput {
    let candidate = ControllerMemorySelectionCandidate {
        id: MemoryId::Project {
            project_id: 1,
            id: 1,
        },
        kind: MemoryKind::Project,
        scope: MemoryScope::Project { project_id: 1 },
        lifecycle: MemoryLifecycle::Active,
        subject: "evaluation memory".into(),
        content: "The evaluation fixture is deterministic.".into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::ProjectFact,
            source_reference: Some("evaluation".into()),
        },
        confidence: Some(0.9),
    };
    ControllerMemorySelectionInput {
        current_project_id: 1,
        current_request: ControllerMemorySelectionRequest::new(vec![
            "The operator identified this memory.".into(),
        ]),
        candidates: vec![candidate],
        eligible_candidate_count: 1,
        selected_candidate_count: 1,
        omitted_candidate_count: 0,
    }
}

fn selection_input_no_target_fixture() -> ControllerMemorySelectionInput {
    ControllerMemorySelectionInput {
        current_project_id: 1,
        current_request: ControllerMemorySelectionRequest::new(vec![
            "No supplied candidate is relevant to this request.".into(),
        ]),
        candidates: Vec::new(),
        eligible_candidate_count: 0,
        selected_candidate_count: 0,
        omitted_candidate_count: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_runtime::LocalInferenceError;
    use std::collections::VecDeque;
    struct FakeRuntime {
        responses: VecDeque<Result<LocalInferenceResponse, LocalInferenceError>>,
    }
    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            _request: &crate::local_runtime::LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            self.responses.pop_front().unwrap_or_else(|| {
                Err(LocalInferenceError::Backend(
                    "missing fixture response".into(),
                ))
            })
        }
    }
    #[test]
    fn representative_suite_covers_all_fixed_capabilities() {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        assert_eq!(suite.scenarios.len(), 17);
        assert!(
            suite
                .scenarios
                .windows(2)
                .all(|window| window[0].id < window[1].id)
        );
        assert!(
            CONTROLLER_SPECIALIZATION_CAPABILITIES
                .iter()
                .all(|capability| suite
                    .scenarios
                    .iter()
                    .any(|scenario| scenario.capability == *capability))
        );
    }
    #[test]
    fn fake_runtime_passes_all_representative_scenarios_and_is_repeatable() {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        let responses = fake_runtime_responses(&suite.scenarios);
        for (scenario, response) in suite
            .scenarios
            .iter()
            .filter(|scenario| uses_runtime(&scenario.input))
            .zip(responses.iter())
        {
            let mut runtime = FakeRuntime {
                responses: vec![response.clone()].into(),
            };
            assert!(execute_scenario(&scenario.input, &mut runtime).is_ok());
        }
        let mut first_runtime = FakeRuntime {
            responses: responses.clone().into(),
        };
        let first = evaluate_controller_specialization(&suite, &mut first_runtime).unwrap();
        let mut second_runtime = FakeRuntime {
            responses: responses.into(),
        };
        let second = evaluate_controller_specialization(&suite, &mut second_runtime).unwrap();
        assert_eq!(
            first.aggregate,
            ControllerSpecializationAggregate {
                total: 17,
                passed: 17,
                incorrect: 0,
                validation_failures: 0,
                runtime_failures: 0
            }
        );
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
    }
    #[test]
    fn runtime_and_validation_failures_do_not_abort_remaining_scenarios() {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        let mut responses = vec![
            Err(LocalInferenceError::Backend(
                "fixture runtime failure".into(),
            )),
            Ok(LocalInferenceResponse::structured(
                "malformed",
                serde_json::json!({"decision": "invalid"}),
            )),
        ];
        for scenario in suite.scenarios.iter().skip(2) {
            if uses_runtime(&scenario.input) {
                responses.push(Ok(scenario.expected.fake_runtime_response().unwrap()));
            }
        }
        let mut runtime = FakeRuntime {
            responses: responses.into(),
        };
        let report = evaluate_controller_specialization(&suite, &mut runtime).unwrap();
        assert_eq!(
            report.scenarios[0].status,
            ControllerSpecializationScenarioStatus::RuntimeFailure
        );
        assert_eq!(
            report.scenarios[1].status,
            ControllerSpecializationScenarioStatus::ObservedValidationFailure
        );
        assert_eq!(report.aggregate.total, 17);
        assert_eq!(report.aggregate.runtime_failures, 1);
        assert_eq!(report.aggregate.validation_failures, 1);
        assert_eq!(report.aggregate.passed, 15);
    }
    #[test]
    fn duplicate_and_unknown_scenarios_fail_closed() {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        let mut duplicate = suite.scenarios.clone();
        duplicate.push(duplicate[0].clone());
        assert!(matches!(
            ControllerSpecializationSuite::new(duplicate),
            Err(ControllerSpecializationEvaluationError::DuplicateOrUnorderedScenario(_))
        ));
        let mut unknown = suite.scenarios[0].clone();
        unknown.capability = "controller.unknown".into();
        assert!(matches!(
            ControllerSpecializationScenario::new(
                unknown.id,
                unknown.description,
                unknown.capability,
                unknown.input,
                unknown.expected,
                unknown.acceptable_alternatives
            ),
            Err(ControllerSpecializationEvaluationError::UnknownCapability(
                _
            ))
        ));
    }

    #[test]
    fn incorrect_typed_result_is_recorded_without_aborting() {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        let mut responses = fake_runtime_responses(&suite.scenarios);
        let index = suite
            .scenarios
            .iter()
            .position(|scenario| scenario.capability == "controller.memory_selection")
            .unwrap();
        let response_index = suite
            .scenarios
            .iter()
            .take(index)
            .filter(|scenario| uses_runtime(&scenario.input))
            .count();
        responses[response_index] = Ok(LocalInferenceResponse::structured(
            "incorrect but well-formed",
            serde_json::json!({"decision": "no_target"}),
        ));
        let mut runtime = FakeRuntime {
            responses: responses.into(),
        };
        let report = evaluate_controller_specialization(&suite, &mut runtime).unwrap();
        assert_eq!(
            report.scenarios[index].status,
            ControllerSpecializationScenarioStatus::IncorrectResult
        );
        assert_eq!(report.aggregate.incorrect, 1);
        assert_eq!(report.aggregate.passed, 16);
    }

    #[test]
    fn no_target_is_incorrect_for_single_candidate_and_review_prose_is_not_semantic() {
        let base = ControllerSpecializationSuite::representative_suite().unwrap();
        let selection = base
            .scenarios
            .iter()
            .find(|scenario| scenario.capability == "controller.memory_selection")
            .unwrap();
        let mut scenarios = base.scenarios.clone();
        let selection_index = scenarios
            .iter()
            .position(|scenario| scenario.capability == "controller.memory_selection")
            .unwrap();
        scenarios[selection_index] = ControllerSpecializationScenario::new(
            selection.id.clone(),
            selection.description.clone(),
            selection.capability.clone(),
            selection.input.clone(),
            selection.expected.clone(),
            Vec::new(),
        )
        .unwrap();
        let review_index = scenarios
            .iter()
            .position(|scenario| scenario.capability == "controller.plan_review")
            .unwrap();
        let review = scenarios[review_index].clone();
        scenarios[review_index] = ControllerSpecializationScenario::new(
            review.id,
            review.description,
            review.capability,
            review.input,
            review.expected,
            vec![ControllerSpecializationOutput::PlanReview(
                ControllerPlanReviewResult {
                    decision: ControllerPlanReviewDecision::Approve,
                    details: "equivalent alternative prose".into(),
                    revision_feedback: None,
                },
            )],
        )
        .unwrap();
        let mut responses = fake_runtime_responses(&scenarios);
        let selection_response_index = scenarios
            .iter()
            .take(selection_index)
            .filter(|scenario| uses_runtime(&scenario.input))
            .count();
        responses[selection_response_index] = Ok(ControllerSpecializationOutput::MemorySelection(
            ControllerMemorySelectionResult::NoTarget,
        )
        .fake_runtime_response()
        .unwrap());
        let review_response_index = scenarios
            .iter()
            .take(review_index)
            .filter(|scenario| uses_runtime(&scenario.input))
            .count();
        responses[review_response_index] =
            ControllerSpecializationOutput::PlanReview(ControllerPlanReviewResult {
                decision: ControllerPlanReviewDecision::Approve,
                details: "different non-semantic prose".into(),
                revision_feedback: None,
            })
            .fake_runtime_response()
            .map(Ok)
            .unwrap();
        let suite = ControllerSpecializationSuite::new(scenarios).unwrap();
        let mut runtime = FakeRuntime {
            responses: responses.into_iter().collect(),
        };
        let report = evaluate_controller_specialization(&suite, &mut runtime).unwrap();
        assert_eq!(
            report.scenarios[selection_index].status,
            ControllerSpecializationScenarioStatus::IncorrectResult
        );
        assert_eq!(report.aggregate.incorrect, 1);
        assert_eq!(report.aggregate.passed, 16);
    }

    #[test]
    fn distinct_capability_branches_pass_through_production_boundaries() {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        let responses = fake_runtime_responses(&suite.scenarios);
        let mut runtime = FakeRuntime {
            responses: responses.into(),
        };
        let report = evaluate_controller_specialization(&suite, &mut runtime).unwrap();
        for id in [
            "controller-task-recommendation-accept",
            "controller-recovery-recommendation-multi-legal",
            "controller-workflow-intake-direct-tasks",
            "controller-plan-review-revise",
            "controller-plan-review-operator",
            "controller-memory-capture-propose",
            "controller-memory-maintenance-propose",
            "controller-memory-selection-no-target",
        ] {
            let scenario = report
                .scenarios
                .iter()
                .find(|scenario| scenario.scenario_id == id)
                .unwrap_or_else(|| panic!("missing branch scenario {id}"));
            assert_eq!(
                scenario.status,
                ControllerSpecializationScenarioStatus::Pass
            );
        }
    }

    #[test]
    fn report_rejects_tampered_aggregate_accounting() {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        let responses = suite
            .scenarios
            .iter()
            .map(|scenario| Ok(scenario.expected.fake_runtime_response().unwrap()))
            .collect::<Vec<_>>();
        let mut runtime = FakeRuntime {
            responses: responses.into(),
        };
        let mut report = evaluate_controller_specialization(&suite, &mut runtime).unwrap();
        report.aggregate.total += 1;
        assert!(matches!(
            report.validate(),
            Err(ControllerSpecializationEvaluationError::InvalidReport(_))
        ));
    }

    fn uses_runtime(input: &ControllerSpecializationInput) -> bool {
        !matches!(
            input,
            ControllerSpecializationInput::MemorySelection(input)
                if input.candidates.is_empty() && input.eligible_candidate_count == 0
        )
    }

    fn fake_runtime_responses(
        scenarios: &[ControllerSpecializationScenario],
    ) -> Vec<Result<LocalInferenceResponse, LocalInferenceError>> {
        scenarios
            .iter()
            .filter(|scenario| uses_runtime(&scenario.input))
            .map(|scenario| Ok(scenario.expected.fake_runtime_response().unwrap()))
            .collect()
    }
}
