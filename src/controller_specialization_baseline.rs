//! Typed, read-only baseline evidence for the canonical M09-002 suite.
//!
//! This module owns no scenario corpus and no Controller execution path. It
//! wraps the result of the M09-002 evaluator with stable local-runtime
//! identity and bounded failure evidence so a later model can be compared
//! against the same expectations.

use crate::controller_specialization_evaluation::{
    CONTROLLER_SPECIALIZATION_CAPABILITIES, CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION,
    ControllerSpecializationAggregate, ControllerSpecializationCapabilityAggregate,
    ControllerSpecializationEvaluationError, ControllerSpecializationEvaluationReport,
    ControllerSpecializationFailureEvidence, ControllerSpecializationFailureKind,
    ControllerSpecializationScenarioReport, ControllerSpecializationScenarioStatus,
    ControllerSpecializationSemanticResult, ControllerSpecializationSuite,
    MAX_CONTROLLER_SPECIALIZATION_FAILURE_BYTES,
};
use crate::local_runtime::{LocalInferenceParameters, LocalInferenceRequest, LocalRuntimeConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

/// Schema version for the typed M09-003 baseline report.
pub const CONTROLLER_SPECIALIZATION_BASELINE_SCHEMA_VERSION: u32 = 1;
/// The only backend identity accepted by this baseline boundary.
pub const CONTROLLER_SPECIALIZATION_BASELINE_BACKEND: &str = "llama_cpp";
pub const MAX_CONTROLLER_SPECIALIZATION_BASELINE_SCENARIOS: usize = 64;
pub const MAX_CONTROLLER_SPECIALIZATION_BASELINE_REQUESTS: usize = 64;
pub const MAX_CONTROLLER_SPECIALIZATION_BASELINE_MODEL_ID_BYTES: usize = 256;
pub const MAX_CONTROLLER_SPECIALIZATION_BASELINE_FAILURE_BYTES: usize =
    MAX_CONTROLLER_SPECIALIZATION_FAILURE_BYTES;

/// Privacy-safe identity for the user-supplied local model. The full path is
/// deliberately excluded from reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationBaselineModelIdentity {
    pub model_file_name: String,
}

impl ControllerSpecializationBaselineModelIdentity {
    pub fn new(
        model_file_name: impl Into<String>,
    ) -> Result<Self, ControllerSpecializationBaselineError> {
        let identity = Self {
            model_file_name: model_file_name.into(),
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn from_runtime_config(
        config: &LocalRuntimeConfig,
    ) -> Result<Self, ControllerSpecializationBaselineError> {
        let file_name = config
            .model_path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                ControllerSpecializationBaselineError::InvalidMetadata(
                    "model path must have a UTF-8 file name".into(),
                )
            })?;
        Self::new(file_name)
    }

    pub(crate) fn validate(&self) -> Result<(), ControllerSpecializationBaselineError> {
        if self.model_file_name.is_empty()
            || self.model_file_name.len() > MAX_CONTROLLER_SPECIALIZATION_BASELINE_MODEL_ID_BYTES
            || self.model_file_name == "."
            || self.model_file_name == ".."
            || self.model_file_name.contains('/')
            || self.model_file_name.contains('\\')
        {
            return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                "model_file_name must be one bounded file name".into(),
            ));
        }
        Ok(())
    }
}

/// One exact production request configuration observed by the recording
/// runtime. It is indexed by canonical M09-002 scenario identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationBaselineRuntimeRequest {
    pub scenario_id: String,
    pub capability: String,
    pub parameters: LocalInferenceParameters,
}

/// Runtime/model settings relevant to interpreting a baseline run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationBaselineRuntime {
    pub backend: String,
    pub context_tokens: u32,
    pub threads: Option<u32>,
    pub requests: Vec<ControllerSpecializationBaselineRuntimeRequest>,
}

impl ControllerSpecializationBaselineRuntime {
    pub fn from_llama_cpp_config(
        config: &LocalRuntimeConfig,
        requests: Vec<ControllerSpecializationBaselineRuntimeRequest>,
    ) -> Result<Self, ControllerSpecializationBaselineError> {
        config.validate().map_err(|error| {
            ControllerSpecializationBaselineError::RuntimeMetadata(error.to_string())
        })?;
        let runtime = Self {
            backend: CONTROLLER_SPECIALIZATION_BASELINE_BACKEND.into(),
            context_tokens: config.context_tokens(),
            threads: config.threads(),
            requests,
        };
        runtime.validate()?;
        Ok(runtime)
    }

    fn validate(&self) -> Result<(), ControllerSpecializationBaselineError> {
        if self.backend != CONTROLLER_SPECIALIZATION_BASELINE_BACKEND {
            return Err(ControllerSpecializationBaselineError::RuntimeMetadata(
                "unsupported baseline backend".into(),
            ));
        }
        if self.context_tokens == 0
            || self.requests.len() > MAX_CONTROLLER_SPECIALIZATION_BASELINE_REQUESTS
        {
            return Err(ControllerSpecializationBaselineError::RuntimeMetadata(
                "runtime metadata is outside its bounds".into(),
            ));
        }
        if self.threads.is_some_and(|threads| threads == 0) {
            return Err(ControllerSpecializationBaselineError::RuntimeMetadata(
                "threads must be greater than zero when present".into(),
            ));
        }
        let mut previous_id: Option<&str> = None;
        for request in &self.requests {
            if request.scenario_id.is_empty()
                || request.scenario_id.len()
                    > crate::controller_specialization_evaluation::MAX_CONTROLLER_SPECIALIZATION_SCENARIO_ID_BYTES
            {
                return Err(ControllerSpecializationBaselineError::RuntimeMetadata(
                    "runtime request scenario ID is outside its bounds".into(),
                ));
            }
            if previous_id.is_some_and(|previous| previous >= request.scenario_id.as_str()) {
                return Err(ControllerSpecializationBaselineError::RuntimeMetadata(
                    "runtime requests must be strictly ordered".into(),
                ));
            }
            previous_id = Some(&request.scenario_id);
            if !CONTROLLER_SPECIALIZATION_CAPABILITIES.contains(&request.capability.as_str()) {
                return Err(ControllerSpecializationBaselineError::UnknownCapability(
                    request.capability.clone(),
                ));
            }
            LocalInferenceRequest::new("baseline runtime metadata", request.parameters.clone())
                .map_err(|error| {
                    ControllerSpecializationBaselineError::RuntimeMetadata(error.to_string())
                })?;
        }
        Ok(())
    }
}

pub type ControllerSpecializationBaselineFailure = ControllerSpecializationFailureEvidence;
pub type ControllerSpecializationBaselineFailureKind = ControllerSpecializationFailureKind;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationBaselineScenario {
    pub scenario_id: String,
    pub capability: String,
    pub expected: ControllerSpecializationSemanticResult,
    pub acceptable_alternatives: Vec<ControllerSpecializationSemanticResult>,
    pub observed: Option<ControllerSpecializationSemanticResult>,
    pub status: ControllerSpecializationScenarioStatus,
    pub failure: Option<ControllerSpecializationBaselineFailure>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationBaselineReport {
    pub schema_version: u32,
    pub suite_schema_version: u32,
    pub scenario_count: usize,
    pub model: ControllerSpecializationBaselineModelIdentity,
    pub runtime: ControllerSpecializationBaselineRuntime,
    pub scenarios: Vec<ControllerSpecializationBaselineScenario>,
    pub aggregate: ControllerSpecializationAggregate,
    pub capabilities: Vec<ControllerSpecializationCapabilityAggregate>,
}

impl ControllerSpecializationBaselineReport {
    /// Wrap an already-computed M09-002 report without recalculating expected
    /// answers or running any additional inference.
    pub fn from_evaluation_report(
        suite: &ControllerSpecializationSuite,
        evaluation: &ControllerSpecializationEvaluationReport,
        model: ControllerSpecializationBaselineModelIdentity,
        runtime: ControllerSpecializationBaselineRuntime,
    ) -> Result<Self, ControllerSpecializationBaselineError> {
        suite.validate().map_err(from_evaluation_error)?;
        evaluation.validate().map_err(from_evaluation_error)?;
        let scenarios = evaluation
            .scenarios
            .iter()
            .map(ControllerSpecializationBaselineScenario::from_evaluation)
            .collect::<Result<Vec<_>, _>>()?;
        let report = Self {
            schema_version: CONTROLLER_SPECIALIZATION_BASELINE_SCHEMA_VERSION,
            suite_schema_version: suite.schema_version,
            scenario_count: scenarios.len(),
            model,
            runtime,
            scenarios,
            aggregate: evaluation.aggregate.clone(),
            capabilities: evaluation.capabilities.clone(),
        };
        report.validate_against_suite(suite)?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), ControllerSpecializationBaselineError> {
        if self.schema_version != CONTROLLER_SPECIALIZATION_BASELINE_SCHEMA_VERSION {
            return Err(
                ControllerSpecializationBaselineError::UnsupportedSchemaVersion(
                    self.schema_version,
                ),
            );
        }
        if self.suite_schema_version != CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION {
            return Err(
                ControllerSpecializationBaselineError::UnsupportedSuiteSchemaVersion(
                    self.suite_schema_version,
                ),
            );
        }
        if self.scenario_count != self.scenarios.len()
            || self.scenarios.len() > MAX_CONTROLLER_SPECIALIZATION_BASELINE_SCENARIOS
        {
            return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                "scenario_count does not equal bounded scenario entries".into(),
            ));
        }
        self.model.validate()?;
        self.runtime.validate()?;

        let mut previous_id: Option<&str> = None;
        let mut scenario_capabilities = BTreeSet::new();
        for scenario in &self.scenarios {
            if scenario.scenario_id.is_empty()
                || scenario.scenario_id.len()
                    > crate::controller_specialization_evaluation::MAX_CONTROLLER_SPECIALIZATION_SCENARIO_ID_BYTES
            {
                return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                    "scenario ID is outside its bounds".into(),
                ));
            }
            if previous_id.is_some_and(|previous| previous >= scenario.scenario_id.as_str()) {
                return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                    "baseline scenarios must be strictly ordered and unique".into(),
                ));
            }
            previous_id = Some(&scenario.scenario_id);
            if !CONTROLLER_SPECIALIZATION_CAPABILITIES.contains(&scenario.capability.as_str()) {
                return Err(ControllerSpecializationBaselineError::UnknownCapability(
                    scenario.capability.clone(),
                ));
            }
            scenario_capabilities.insert(scenario.capability.as_str());
            match scenario.status {
                ControllerSpecializationScenarioStatus::Pass
                | ControllerSpecializationScenarioStatus::IncorrectResult => {
                    if scenario.observed.is_none() || scenario.failure.is_some() {
                        return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                            "observed result statuses require observed semantics only".into(),
                        ));
                    }
                }
                ControllerSpecializationScenarioStatus::ObservedValidationFailure
                | ControllerSpecializationScenarioStatus::RuntimeFailure => {
                    if scenario.observed.is_some() || scenario.failure.is_none() {
                        return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                            "failure statuses require bounded failure evidence only".into(),
                        ));
                    }
                }
            }
            if let Some(failure) = &scenario.failure {
                let expected_kind = match scenario.status {
                    ControllerSpecializationScenarioStatus::ObservedValidationFailure => None,
                    ControllerSpecializationScenarioStatus::RuntimeFailure => {
                        Some(ControllerSpecializationFailureKind::Runtime)
                    }
                    ControllerSpecializationScenarioStatus::Pass
                    | ControllerSpecializationScenarioStatus::IncorrectResult => {
                        return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                            "observed-result statuses cannot carry failure evidence".into(),
                        ));
                    }
                };
                if failure.validate().is_err() {
                    return Err(ControllerSpecializationBaselineError::FailureEvidenceBounds);
                }
                if expected_kind.is_some_and(|kind| failure.kind() != kind)
                    || (matches!(
                        scenario.status,
                        ControllerSpecializationScenarioStatus::ObservedValidationFailure
                    ) && !matches!(
                        failure.kind(),
                        ControllerSpecializationFailureKind::Parse
                            | ControllerSpecializationFailureKind::Validation
                    ))
                {
                    return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                        "failure kind does not match scenario status".into(),
                    ));
                }
            }
        }

        let evaluation = self.as_evaluation_report();
        evaluation.validate().map_err(from_evaluation_error)?;
        let represented = self
            .capabilities
            .iter()
            .map(|aggregate| aggregate.capability.as_str())
            .collect::<Vec<_>>();
        let mut expected_capabilities = CONTROLLER_SPECIALIZATION_CAPABILITIES.to_vec();
        expected_capabilities.sort_unstable();
        if represented != expected_capabilities
            || scenario_capabilities.len() != CONTROLLER_SPECIALIZATION_CAPABILITIES.len()
            || scenario_capabilities
                .iter()
                .any(|capability| !CONTROLLER_SPECIALIZATION_CAPABILITIES.contains(capability))
        {
            return Err(ControllerSpecializationBaselineError::InvalidMetadata(
                "capability aggregates must be fixed and lexicographically ordered".into(),
            ));
        }
        Ok(())
    }

    /// Validate identity, expectations, request configuration, and ordering
    /// against the exact canonical M09-002 suite.
    pub fn validate_against_suite(
        &self,
        suite: &ControllerSpecializationSuite,
    ) -> Result<(), ControllerSpecializationBaselineError> {
        self.validate()?;
        suite.validate().map_err(from_evaluation_error)?;
        if self.suite_schema_version != suite.schema_version
            || self.scenario_count != suite.scenarios.len()
        {
            return Err(ControllerSpecializationBaselineError::SuiteIdentityMismatch);
        }
        for (report, scenario) in self.scenarios.iter().zip(&suite.scenarios) {
            if report.scenario_id != scenario.id || report.capability != scenario.capability {
                return Err(ControllerSpecializationBaselineError::SuiteIdentityMismatch);
            }
            let expected = scenario
                .expected
                .semantic_result(&scenario.input)
                .map_err(from_evaluation_error)?;
            let alternatives = scenario
                .acceptable_alternatives
                .iter()
                .map(|alternative| alternative.semantic_result(&scenario.input))
                .collect::<Result<Vec<_>, _>>()
                .map_err(from_evaluation_error)?;
            if report.expected != expected || report.acceptable_alternatives != alternatives {
                return Err(ControllerSpecializationBaselineError::SuiteIdentityMismatch);
            }
        }
        let expected_requests = suite
            .scenarios
            .iter()
            .filter(|scenario| scenario.requires_runtime())
            .collect::<Vec<_>>();
        if self.runtime.requests.len() != expected_requests.len() {
            return Err(ControllerSpecializationBaselineError::RuntimeMetadata(
                "recorded request count does not match the canonical evaluator path".into(),
            ));
        }
        for (request, scenario) in self.runtime.requests.iter().zip(expected_requests) {
            if request.scenario_id != scenario.id || request.capability != scenario.capability {
                return Err(ControllerSpecializationBaselineError::RuntimeMetadata(
                    "recorded request identity does not match canonical scenario order".into(),
                ));
            }
        }
        Ok(())
    }

    fn as_evaluation_report(&self) -> ControllerSpecializationEvaluationReport {
        ControllerSpecializationEvaluationReport {
            schema_version: CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION,
            scenarios: self
                .scenarios
                .iter()
                .map(|scenario| ControllerSpecializationScenarioReport {
                    scenario_id: scenario.scenario_id.clone(),
                    capability: scenario.capability.clone(),
                    expected: scenario.expected.clone(),
                    acceptable_alternatives: scenario.acceptable_alternatives.clone(),
                    observed: scenario.observed.clone(),
                    status: scenario.status,
                    failure: scenario.failure.clone(),
                })
                .collect(),
            aggregate: self.aggregate.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

impl ControllerSpecializationBaselineScenario {
    fn from_evaluation(
        report: &ControllerSpecializationScenarioReport,
    ) -> Result<Self, ControllerSpecializationBaselineError> {
        Ok(Self {
            scenario_id: report.scenario_id.clone(),
            capability: report.capability.clone(),
            expected: report.expected.clone(),
            acceptable_alternatives: report.acceptable_alternatives.clone(),
            observed: report.observed.clone(),
            status: report.status,
            failure: report.failure.clone(),
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ControllerSpecializationBaselineError {
    #[error("unsupported baseline schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("unsupported M09-002 suite schema version {0}")]
    UnsupportedSuiteSchemaVersion(u32),
    #[error("baseline metadata is invalid: {0}")]
    InvalidMetadata(String),
    #[error("baseline runtime metadata is invalid: {0}")]
    RuntimeMetadata(String),
    #[error("baseline contains unknown capability `{0}`")]
    UnknownCapability(String),
    #[error("baseline failure evidence exceeds its bound")]
    FailureEvidenceBounds,
    #[error("baseline scenario identity does not match the canonical M09-002 suite")]
    SuiteIdentityMismatch,
    #[error("baseline metadata could not be serialized: {0}")]
    Serialization(String),
}

fn from_evaluation_error(
    error: ControllerSpecializationEvaluationError,
) -> ControllerSpecializationBaselineError {
    ControllerSpecializationBaselineError::InvalidMetadata(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_specialization_evaluation::evaluate_controller_specialization;
    use crate::local_runtime::{
        LocalInferenceError, LocalInferenceResponse, LocalInferenceRuntime,
    };
    use std::collections::VecDeque;

    struct FakeRuntime {
        responses: VecDeque<Result<LocalInferenceResponse, LocalInferenceError>>,
        requests: Vec<crate::local_runtime::LocalInferenceRequest>,
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            request: &crate::local_runtime::LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            self.requests.push(request.clone());
            self.responses.pop_front().unwrap_or_else(|| {
                Err(LocalInferenceError::Backend("missing fake response".into()))
            })
        }
    }

    fn fixture() -> (
        ControllerSpecializationSuite,
        ControllerSpecializationEvaluationReport,
        ControllerSpecializationBaselineRuntime,
    ) {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        let responses = suite
            .scenarios
            .iter()
            .filter(|scenario| scenario.requires_runtime())
            .map(|scenario| Ok(scenario.expected.fake_runtime_response().unwrap()))
            .collect();
        let mut runtime = FakeRuntime {
            responses,
            requests: Vec::new(),
        };
        let evaluation = evaluate_controller_specialization(&suite, &mut runtime).unwrap();
        let requests = suite
            .scenarios
            .iter()
            .filter(|scenario| scenario.requires_runtime())
            .zip(runtime.requests)
            .map(
                |(scenario, request)| ControllerSpecializationBaselineRuntimeRequest {
                    scenario_id: scenario.id.clone(),
                    capability: scenario.capability.clone(),
                    parameters: request.parameters,
                },
            )
            .collect();
        let runtime = ControllerSpecializationBaselineRuntime::from_llama_cpp_config(
            &LocalRuntimeConfig::new("Qwen3-8B.gguf"),
            requests,
        )
        .unwrap();
        (suite, evaluation, runtime)
    }

    #[test]
    fn baseline_wraps_exact_canonical_suite_and_is_deterministic() {
        let (suite, evaluation, runtime) = fixture();
        let model = ControllerSpecializationBaselineModelIdentity::new("Qwen3-8B.gguf").unwrap();
        let first = ControllerSpecializationBaselineReport::from_evaluation_report(
            &suite,
            &evaluation,
            model.clone(),
            runtime.clone(),
        )
        .unwrap();
        let second = ControllerSpecializationBaselineReport::from_evaluation_report(
            &suite,
            &evaluation,
            model,
            runtime,
        )
        .unwrap();
        assert_eq!(first.schema_version, 1);
        assert_eq!(
            first.suite_schema_version,
            CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION
        );
        assert_eq!(first.scenario_count, 17);
        assert_eq!(first.scenarios.len(), 17);
        assert_eq!(first.capabilities.len(), 9);
        let mut expected_capabilities = CONTROLLER_SPECIALIZATION_CAPABILITIES.to_vec();
        expected_capabilities.sort_unstable();
        assert_eq!(
            first
                .capabilities
                .iter()
                .map(|aggregate| aggregate.capability.as_str())
                .collect::<Vec<_>>(),
            expected_capabilities
        );
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert_eq!(first, second);
    }

    #[test]
    fn baseline_validation_rejects_metadata_and_identity_tampering() {
        let (suite, evaluation, runtime) = fixture();
        let model = ControllerSpecializationBaselineModelIdentity::new("Qwen3-8B.gguf").unwrap();
        let mut report = ControllerSpecializationBaselineReport::from_evaluation_report(
            &suite,
            &evaluation,
            model,
            runtime,
        )
        .unwrap();
        report.schema_version += 1;
        assert!(matches!(
            report.validate(),
            Err(ControllerSpecializationBaselineError::UnsupportedSchemaVersion(_))
        ));
        report.schema_version = CONTROLLER_SPECIALIZATION_BASELINE_SCHEMA_VERSION;
        report.scenarios[0].scenario_id = "zzzz".into();
        assert!(matches!(
            report.validate_against_suite(&suite),
            Err(ControllerSpecializationBaselineError::SuiteIdentityMismatch)
                | Err(ControllerSpecializationBaselineError::InvalidMetadata(_))
        ));
        let encoded = serde_json::to_value(&report).unwrap();
        let mut object = encoded.as_object().unwrap().clone();
        object.insert("unexpected".into(), serde_json::json!(true));
        assert!(
            serde_json::from_value::<ControllerSpecializationBaselineReport>(
                serde_json::Value::Object(object)
            )
            .is_err()
        );
        report.scenarios[0].scenario_id = "controller-task-recommendation".into();
        report.scenarios[0].capability = "controller.unknown".into();
        assert!(matches!(
            report.validate(),
            Err(ControllerSpecializationBaselineError::UnknownCapability(_))
        ));
    }

    #[test]
    fn baseline_validation_rejects_unbounded_failure_evidence() {
        let (suite, evaluation, runtime) = fixture();
        let model = ControllerSpecializationBaselineModelIdentity::new("Qwen3-8B.gguf").unwrap();
        let mut report = ControllerSpecializationBaselineReport::from_evaluation_report(
            &suite,
            &evaluation,
            model,
            runtime,
        )
        .unwrap();
        report.scenarios[0].status = ControllerSpecializationScenarioStatus::RuntimeFailure;
        report.scenarios[0].observed = None;
        report.scenarios[0].failure = Some(ControllerSpecializationBaselineFailure::Runtime {
            error: "x".repeat(MAX_CONTROLLER_SPECIALIZATION_BASELINE_FAILURE_BYTES + 1),
        });
        assert_eq!(
            report.validate(),
            Err(ControllerSpecializationBaselineError::FailureEvidenceBounds)
        );
    }

    #[test]
    fn baseline_report_has_no_orc_app_or_storage_dependency() {
        let (suite, evaluation, runtime) = fixture();
        let report = ControllerSpecializationBaselineReport::from_evaluation_report(
            &suite,
            &evaluation,
            ControllerSpecializationBaselineModelIdentity::new("model.gguf").unwrap(),
            runtime,
        )
        .unwrap();
        assert_eq!(report.aggregate.total, report.scenarios.len() as u64);
        assert!(
            report
                .scenarios
                .iter()
                .all(|scenario| scenario.failure.is_none())
        );
    }

    #[test]
    fn baseline_preserves_runtime_failure_evidence_without_aborting() {
        let suite = ControllerSpecializationSuite::representative_suite().unwrap();
        let responses = suite
            .scenarios
            .iter()
            .filter(|scenario| scenario.requires_runtime())
            .enumerate()
            .map(|(index, scenario)| {
                if index == 0 {
                    Err(LocalInferenceError::Backend(
                        "synthetic backend failure".into(),
                    ))
                } else {
                    Ok(scenario.expected.fake_runtime_response().unwrap())
                }
            })
            .collect();
        let mut runtime = FakeRuntime {
            responses,
            requests: Vec::new(),
        };
        let evaluation = evaluate_controller_specialization(&suite, &mut runtime).unwrap();
        let requests = suite
            .scenarios
            .iter()
            .filter(|scenario| scenario.requires_runtime())
            .zip(runtime.requests)
            .map(
                |(scenario, request)| ControllerSpecializationBaselineRuntimeRequest {
                    scenario_id: scenario.id.clone(),
                    capability: scenario.capability.clone(),
                    parameters: request.parameters,
                },
            )
            .collect();
        let report = ControllerSpecializationBaselineReport::from_evaluation_report(
            &suite,
            &evaluation,
            ControllerSpecializationBaselineModelIdentity::new("model.gguf").unwrap(),
            ControllerSpecializationBaselineRuntime::from_llama_cpp_config(
                &LocalRuntimeConfig::new("model.gguf"),
                requests,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            report.scenarios[0].status,
            ControllerSpecializationScenarioStatus::RuntimeFailure
        );
        assert_eq!(
            report.scenarios[0]
                .failure
                .as_ref()
                .map(ControllerSpecializationBaselineFailure::kind),
            Some(ControllerSpecializationBaselineFailureKind::Runtime)
        );
        assert_eq!(report.aggregate.total, 17);
        assert_eq!(report.aggregate.runtime_failures, 1);
        assert_eq!(report.aggregate.passed, 16);
    }

    #[test]
    fn baseline_preserves_parse_validation_and_runtime_evidence() {
        fn build_report() -> ControllerSpecializationBaselineReport {
            let suite = ControllerSpecializationSuite::representative_suite().unwrap();
            let responses = suite
                .scenarios
                .iter()
                .filter(|scenario| scenario.requires_runtime())
                .enumerate()
                .map(|(index, scenario)| match index {
                    0 => Err(LocalInferenceError::InvalidStructuredOutput {
                        raw_output: "malformed model JSON".into(),
                        parse_error: "unexpected end of input".into(),
                    }),
                    1 => Ok(LocalInferenceResponse::structured(
                        "typed but contract-invalid",
                        serde_json::json!({
                            "suggested_next_step": null,
                            "decision_class": "invalid",
                            "rationale": "synthetic"
                        }),
                    )),
                    2 => Err(LocalInferenceError::Backend(
                        "synthetic backend failure".into(),
                    )),
                    _ => Ok(scenario.expected.fake_runtime_response().unwrap()),
                })
                .collect();
            let mut runtime = FakeRuntime {
                responses,
                requests: Vec::new(),
            };
            let evaluation = evaluate_controller_specialization(&suite, &mut runtime).unwrap();
            let requests = suite
                .scenarios
                .iter()
                .filter(|scenario| scenario.requires_runtime())
                .zip(runtime.requests)
                .map(
                    |(scenario, request)| ControllerSpecializationBaselineRuntimeRequest {
                        scenario_id: scenario.id.clone(),
                        capability: scenario.capability.clone(),
                        parameters: request.parameters,
                    },
                )
                .collect();
            ControllerSpecializationBaselineReport::from_evaluation_report(
                &suite,
                &evaluation,
                ControllerSpecializationBaselineModelIdentity::new("model.gguf").unwrap(),
                ControllerSpecializationBaselineRuntime::from_llama_cpp_config(
                    &LocalRuntimeConfig::new("model.gguf"),
                    requests,
                )
                .unwrap(),
            )
            .unwrap()
        }

        let first = build_report();
        let second = build_report();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert!(matches!(
            first.scenarios[0].failure,
            Some(ControllerSpecializationBaselineFailure::Parse { .. })
        ));
        assert!(matches!(
            first.scenarios[1].failure,
            Some(ControllerSpecializationBaselineFailure::Validation { .. })
        ));
        assert!(matches!(
            first.scenarios[2].failure,
            Some(ControllerSpecializationBaselineFailure::Runtime { .. })
        ));
        assert_eq!(first.aggregate.validation_failures, 2);
        assert_eq!(first.aggregate.runtime_failures, 1);
        assert_eq!(first.aggregate.passed, 14);
        if let Some(ControllerSpecializationBaselineFailure::Parse {
            raw_output,
            parse_error,
        }) = first.scenarios[0].failure.as_ref()
        {
            assert_eq!(raw_output, "malformed model JSON");
            assert_eq!(parse_error, "unexpected end of input");
        } else {
            panic!("expected parse evidence");
        }
    }
}
