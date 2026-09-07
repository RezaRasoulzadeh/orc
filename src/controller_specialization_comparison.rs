//! Deterministic, model-independent comparison and promotion gate for M09-003.
//!
//! This boundary consumes already validated baseline reports only. It owns no
//! runtime, storage, Controller action, training, or deployment behavior.

use crate::controller_specialization_baseline::{
    ControllerSpecializationBaselineError, ControllerSpecializationBaselineModelIdentity,
    ControllerSpecializationBaselineReport, ControllerSpecializationBaselineScenario,
    MAX_CONTROLLER_SPECIALIZATION_BASELINE_SCENARIOS,
};
use crate::controller_specialization_evaluation::{
    CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION, ControllerSpecializationAggregate,
    ControllerSpecializationCapabilityAggregate, ControllerSpecializationFailureKind,
    ControllerSpecializationScenarioStatus, ControllerSpecializationSemanticResult,
    ControllerSpecializationSuite,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Schema version for the M09-004 typed comparison report.
pub const CONTROLLER_SPECIALIZATION_COMPARISON_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSpecializationComparability {
    Comparable,
    NonComparable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSpecializationComparisonDecision {
    Promote,
    Reject,
    SelfComparison,
    NonComparable,
}

/// The authoritative machine-readable reason categories. Their declaration
/// order is the canonical serialization/order of reasons in a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSpecializationComparisonReasonCode {
    SuiteSchemaMismatch,
    ScenarioCountMismatch,
    ScenarioIdentityMismatch,
    CapabilityIdentityMismatch,
    ExpectedSemanticMismatch,
    AcceptableAlternativesMismatch,
    RequestParametersMismatch,
    RuntimeConfigurationMismatch,
    SelfComparison,
    StrictGlobalImprovementRequired,
    ExecutionErrorsIncreased,
    CapabilityPassRegression,
    BaselinePassScenarioRegression,
    NewParseFailure,
    NewValidationFailure,
    NewRuntimeFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSpecializationScenarioOutcomeClass {
    Pass,
    IncorrectResult,
    Parse,
    Validation,
    Runtime,
}

impl ControllerSpecializationScenarioOutcomeClass {
    fn from_parts(
        status: ControllerSpecializationScenarioStatus,
        failure: Option<
            &crate::controller_specialization_evaluation::ControllerSpecializationFailureEvidence,
        >,
    ) -> Result<Self, ControllerSpecializationComparisonError> {
        match status {
            ControllerSpecializationScenarioStatus::Pass => Ok(Self::Pass),
            ControllerSpecializationScenarioStatus::IncorrectResult => Ok(Self::IncorrectResult),
            ControllerSpecializationScenarioStatus::ObservedValidationFailure => {
                let failure = failure.ok_or_else(|| {
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        "validation-failure scenario has no failure evidence".into(),
                    )
                })?;
                match failure.kind() {
                    ControllerSpecializationFailureKind::Parse => Ok(Self::Parse),
                    ControllerSpecializationFailureKind::Validation => Ok(Self::Validation),
                    ControllerSpecializationFailureKind::Runtime => Err(
                        ControllerSpecializationComparisonError::InvalidComparisonReport(
                            "validation-failure scenario has runtime evidence".into(),
                        ),
                    ),
                }
            }
            ControllerSpecializationScenarioStatus::RuntimeFailure => {
                let failure = failure.ok_or_else(|| {
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        "runtime-failure scenario has no failure evidence".into(),
                    )
                })?;
                if failure.kind() != ControllerSpecializationFailureKind::Runtime {
                    return Err(
                        ControllerSpecializationComparisonError::InvalidComparisonReport(
                            "runtime-failure scenario has non-runtime evidence".into(),
                        ),
                    );
                }
                Ok(Self::Runtime)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationScenarioTransition {
    pub baseline: ControllerSpecializationScenarioOutcomeClass,
    pub candidate: ControllerSpecializationScenarioOutcomeClass,
}

impl ControllerSpecializationScenarioTransition {
    fn classify(
        baseline: ControllerSpecializationScenarioOutcomeClass,
        candidate: ControllerSpecializationScenarioOutcomeClass,
    ) -> Self {
        Self {
            baseline,
            candidate,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationScenarioComparison {
    pub scenario_id: String,
    pub capability: String,
    pub expected: ControllerSpecializationSemanticResult,
    pub acceptable_alternatives: Vec<ControllerSpecializationSemanticResult>,
    pub baseline_status: ControllerSpecializationScenarioStatus,
    pub candidate_status: ControllerSpecializationScenarioStatus,
    pub baseline_outcome: ControllerSpecializationScenarioOutcomeClass,
    pub candidate_outcome: ControllerSpecializationScenarioOutcomeClass,
    pub transition: ControllerSpecializationScenarioTransition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSpecializationSignedDelta {
    Negative(u64),
    Zero,
    Positive(u64),
}

impl ControllerSpecializationSignedDelta {
    fn between(candidate: u64, baseline: u64) -> Self {
        match candidate.cmp(&baseline) {
            std::cmp::Ordering::Less => Self::Negative(baseline - candidate),
            std::cmp::Ordering::Equal => Self::Zero,
            std::cmp::Ordering::Greater => Self::Positive(candidate - baseline),
        }
    }

    fn validate(&self) -> Result<(), ControllerSpecializationComparisonError> {
        if matches!(self, Self::Negative(0) | Self::Positive(0)) {
            return Err(
                ControllerSpecializationComparisonError::InvalidComparisonReport(
                    "signed delta has a zero magnitude".into(),
                ),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationCountDelta {
    pub total: ControllerSpecializationSignedDelta,
    pub passed: ControllerSpecializationSignedDelta,
    pub incorrect: ControllerSpecializationSignedDelta,
    pub validation_failures: ControllerSpecializationSignedDelta,
    pub runtime_failures: ControllerSpecializationSignedDelta,
}

impl ControllerSpecializationCountDelta {
    fn between(
        candidate: &ControllerSpecializationAggregate,
        baseline: &ControllerSpecializationAggregate,
    ) -> Self {
        Self {
            total: ControllerSpecializationSignedDelta::between(candidate.total, baseline.total),
            passed: ControllerSpecializationSignedDelta::between(candidate.passed, baseline.passed),
            incorrect: ControllerSpecializationSignedDelta::between(
                candidate.incorrect,
                baseline.incorrect,
            ),
            validation_failures: ControllerSpecializationSignedDelta::between(
                candidate.validation_failures,
                baseline.validation_failures,
            ),
            runtime_failures: ControllerSpecializationSignedDelta::between(
                candidate.runtime_failures,
                baseline.runtime_failures,
            ),
        }
    }

    fn validate(&self) -> Result<(), ControllerSpecializationComparisonError> {
        self.total.validate()?;
        self.passed.validate()?;
        self.incorrect.validate()?;
        self.validation_failures.validate()?;
        self.runtime_failures.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationGlobalDelta {
    pub baseline: ControllerSpecializationAggregate,
    pub candidate: ControllerSpecializationAggregate,
    pub delta: ControllerSpecializationCountDelta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationCapabilityDelta {
    pub capability: String,
    pub baseline: ControllerSpecializationCapabilityAggregate,
    pub candidate: ControllerSpecializationCapabilityAggregate,
    pub delta: ControllerSpecializationCountDelta,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSpecializationComparisonReport {
    pub schema_version: u32,
    pub baseline_model: ControllerSpecializationBaselineModelIdentity,
    pub candidate_model: ControllerSpecializationBaselineModelIdentity,
    pub baseline_suite_schema_version: u32,
    pub candidate_suite_schema_version: u32,
    pub baseline_scenario_count: usize,
    pub candidate_scenario_count: usize,
    pub comparability: ControllerSpecializationComparability,
    pub scenarios: Vec<ControllerSpecializationScenarioComparison>,
    pub capabilities: Vec<ControllerSpecializationCapabilityDelta>,
    pub global: Option<ControllerSpecializationGlobalDelta>,
    pub decision: ControllerSpecializationComparisonDecision,
    pub reasons: Vec<ControllerSpecializationComparisonReasonCode>,
}

impl ControllerSpecializationComparisonReport {
    pub fn validate(&self) -> Result<(), ControllerSpecializationComparisonError> {
        if self.schema_version != CONTROLLER_SPECIALIZATION_COMPARISON_SCHEMA_VERSION {
            return Err(
                ControllerSpecializationComparisonError::UnsupportedSchemaVersion(
                    self.schema_version,
                ),
            );
        }
        self.baseline_model.validate().map_err(|error| {
            ControllerSpecializationComparisonError::InvalidComparisonReport(error.to_string())
        })?;
        self.candidate_model.validate().map_err(|error| {
            ControllerSpecializationComparisonError::InvalidComparisonReport(error.to_string())
        })?;
        if self.baseline_scenario_count > MAX_CONTROLLER_SPECIALIZATION_BASELINE_SCENARIOS
            || self.candidate_scenario_count > MAX_CONTROLLER_SPECIALIZATION_BASELINE_SCENARIOS
        {
            return Err(
                ControllerSpecializationComparisonError::InvalidComparisonReport(
                    "comparison scenario count is outside its bound".into(),
                ),
            );
        }
        validate_reasons(&self.reasons)?;
        match self.comparability {
            ControllerSpecializationComparability::NonComparable => {
                if self.decision != ControllerSpecializationComparisonDecision::NonComparable
                    || self.global.is_some()
                    || !self.scenarios.is_empty()
                    || !self.capabilities.is_empty()
                    || self.reasons.is_empty()
                    || self
                        .reasons
                        .iter()
                        .any(|reason| !is_comparability_reason(*reason))
                {
                    return Err(
                        ControllerSpecializationComparisonError::InvalidComparisonReport(
                            "non-comparable report evidence is inconsistent".into(),
                        ),
                    );
                }
                return Ok(());
            }
            ControllerSpecializationComparability::Comparable => {}
        }
        if self.baseline_suite_schema_version != CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION
            || self.candidate_suite_schema_version
                != CONTROLLER_SPECIALIZATION_EVALUATION_SCHEMA_VERSION
            || self.baseline_scenario_count != self.candidate_scenario_count
            || self.scenarios.len() != self.baseline_scenario_count
        {
            return Err(
                ControllerSpecializationComparisonError::InvalidComparisonReport(
                    "comparable report suite identity is inconsistent".into(),
                ),
            );
        }
        let mut previous_id: Option<&str> = None;
        for scenario in &self.scenarios {
            if scenario.scenario_id.is_empty()
                || previous_id.is_some_and(|previous| previous >= scenario.scenario_id.as_str())
            {
                return Err(
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        "comparison scenarios must be strictly ordered and unique".into(),
                    ),
                );
            }
            previous_id = Some(&scenario.scenario_id);
            if scenario.expected.capability() != scenario.capability
                || scenario
                    .acceptable_alternatives
                    .iter()
                    .any(|alternative| alternative.capability() != scenario.capability)
            {
                return Err(
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        "comparison semantic capability does not match scenario capability".into(),
                    ),
                );
            }
            if scenario.transition
                != ControllerSpecializationScenarioTransition::classify(
                    scenario.baseline_outcome,
                    scenario.candidate_outcome,
                )
            {
                return Err(
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        "scenario transition does not match typed outcomes".into(),
                    ),
                );
            }
            if !status_matches_class(scenario.baseline_status, scenario.baseline_outcome)
                || !status_matches_class(scenario.candidate_status, scenario.candidate_outcome)
            {
                return Err(
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        "scenario status does not match typed outcome".into(),
                    ),
                );
            }
        }
        let global = self.global.as_ref().ok_or_else(|| {
            ControllerSpecializationComparisonError::InvalidComparisonReport(
                "comparable report has no global delta".into(),
            )
        })?;
        validate_global_delta(global, &self.scenarios)?;
        if self.capabilities.len() != 9 {
            return Err(
                ControllerSpecializationComparisonError::InvalidComparisonReport(
                    "comparison must contain all nine capability deltas".into(),
                ),
            );
        }
        let suite = ControllerSpecializationSuite::representative_suite().map_err(|error| {
            ControllerSpecializationComparisonError::InvalidComparisonReport(error.to_string())
        })?;
        if self.scenarios.len() != suite.scenarios.len() {
            return Err(
                ControllerSpecializationComparisonError::InvalidComparisonReport(
                    "comparison does not contain the complete canonical suite".into(),
                ),
            );
        }
        for (comparison, scenario) in self.scenarios.iter().zip(&suite.scenarios) {
            let expected = scenario
                .expected
                .semantic_result(&scenario.input)
                .map_err(|error| {
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        error.to_string(),
                    )
                })?;
            let alternatives = scenario
                .acceptable_alternatives
                .iter()
                .map(|alternative| alternative.semantic_result(&scenario.input))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        error.to_string(),
                    )
                })?;
            if comparison.scenario_id != scenario.id
                || comparison.capability != scenario.capability
                || comparison.expected != expected
                || comparison.acceptable_alternatives != alternatives
            {
                return Err(
                    ControllerSpecializationComparisonError::InvalidComparisonReport(
                        "comparison scenario authority is not canonical".into(),
                    ),
                );
            }
        }
        let mut expected_capabilities = Vec::new();
        for scenario in &self.scenarios {
            if !expected_capabilities.contains(&scenario.capability) {
                expected_capabilities.push(scenario.capability.clone());
            }
        }
        expected_capabilities.sort_unstable();
        if self
            .capabilities
            .iter()
            .map(|delta| delta.capability.clone())
            .collect::<Vec<_>>()
            != expected_capabilities
        {
            return Err(
                ControllerSpecializationComparisonError::InvalidComparisonReport(
                    "capability deltas are not complete and lexicographically ordered".into(),
                ),
            );
        }
        for capability in &self.capabilities {
            validate_capability_delta(capability, &self.scenarios)?;
        }
        let expected_reasons = gate_reasons(
            &self.baseline_model,
            &self.candidate_model,
            global,
            &self.capabilities,
            &self.scenarios,
        );
        if self.reasons != expected_reasons {
            return Err(
                ControllerSpecializationComparisonError::InvalidComparisonReport(
                    "comparison reasons do not equal typed evidence".into(),
                ),
            );
        }
        let expected_decision =
            decision_for(&self.baseline_model, &self.candidate_model, &self.reasons);
        if self.decision != expected_decision {
            return Err(
                ControllerSpecializationComparisonError::InvalidComparisonReport(
                    "comparison decision does not equal typed evidence".into(),
                ),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ControllerSpecializationComparisonError {
    #[error("unsupported comparison schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("baseline report is invalid: {0}")]
    InvalidBaselineReport(String),
    #[error("candidate report is invalid: {0}")]
    InvalidCandidateReport(String),
    #[error("comparison report is invalid: {0}")]
    InvalidComparisonReport(String),
}

pub fn compare_controller_specialization_reports(
    baseline: &ControllerSpecializationBaselineReport,
    candidate: &ControllerSpecializationBaselineReport,
) -> Result<ControllerSpecializationComparisonReport, ControllerSpecializationComparisonError> {
    validate_baseline(baseline, false)?;
    validate_baseline(candidate, true)?;
    let reasons = comparability_reasons(baseline, candidate);
    if !reasons.is_empty() {
        let report = ControllerSpecializationComparisonReport {
            schema_version: CONTROLLER_SPECIALIZATION_COMPARISON_SCHEMA_VERSION,
            baseline_model: baseline.model.clone(),
            candidate_model: candidate.model.clone(),
            baseline_suite_schema_version: baseline.suite_schema_version,
            candidate_suite_schema_version: candidate.suite_schema_version,
            baseline_scenario_count: baseline.scenario_count,
            candidate_scenario_count: candidate.scenario_count,
            comparability: ControllerSpecializationComparability::NonComparable,
            scenarios: Vec::new(),
            capabilities: Vec::new(),
            global: None,
            decision: ControllerSpecializationComparisonDecision::NonComparable,
            reasons,
        };
        report.validate()?;
        return Ok(report);
    }

    let scenarios = baseline
        .scenarios
        .iter()
        .zip(&candidate.scenarios)
        .map(|(baseline, candidate)| compare_scenario(baseline, candidate))
        .collect::<Result<Vec<_>, _>>()?;
    let capabilities = baseline
        .capabilities
        .iter()
        .zip(&candidate.capabilities)
        .map(|(baseline, candidate)| compare_capability(baseline, candidate))
        .collect::<Vec<_>>();
    let global = ControllerSpecializationGlobalDelta {
        baseline: baseline.aggregate.clone(),
        candidate: candidate.aggregate.clone(),
        delta: ControllerSpecializationCountDelta::between(
            &candidate.aggregate,
            &baseline.aggregate,
        ),
    };
    let reasons = gate_reasons(
        &baseline.model,
        &candidate.model,
        &global,
        &capabilities,
        &scenarios,
    );
    let decision = decision_for(&baseline.model, &candidate.model, &reasons);
    let report = ControllerSpecializationComparisonReport {
        schema_version: CONTROLLER_SPECIALIZATION_COMPARISON_SCHEMA_VERSION,
        baseline_model: baseline.model.clone(),
        candidate_model: candidate.model.clone(),
        baseline_suite_schema_version: baseline.suite_schema_version,
        candidate_suite_schema_version: candidate.suite_schema_version,
        baseline_scenario_count: baseline.scenario_count,
        candidate_scenario_count: candidate.scenario_count,
        comparability: ControllerSpecializationComparability::Comparable,
        scenarios,
        capabilities,
        global: Some(global),
        decision,
        reasons,
    };
    report.validate()?;
    Ok(report)
}

pub fn compare_controller_specialization(
    baseline: &ControllerSpecializationBaselineReport,
    candidate: &ControllerSpecializationBaselineReport,
) -> Result<ControllerSpecializationComparisonReport, ControllerSpecializationComparisonError> {
    compare_controller_specialization_reports(baseline, candidate)
}

fn validate_baseline(
    report: &ControllerSpecializationBaselineReport,
    candidate: bool,
) -> Result<(), ControllerSpecializationComparisonError> {
    let invalid = |error: ControllerSpecializationBaselineError| {
        if candidate {
            ControllerSpecializationComparisonError::InvalidCandidateReport(error.to_string())
        } else {
            ControllerSpecializationComparisonError::InvalidBaselineReport(error.to_string())
        }
    };
    report.validate().map_err(invalid)?;
    let suite = ControllerSpecializationSuite::representative_suite().map_err(|error| {
        invalid(ControllerSpecializationBaselineError::InvalidMetadata(
            error.to_string(),
        ))
    })?;
    report.validate_against_suite(&suite).map_err(invalid)
}

fn comparability_reasons(
    baseline: &ControllerSpecializationBaselineReport,
    candidate: &ControllerSpecializationBaselineReport,
) -> Vec<ControllerSpecializationComparisonReasonCode> {
    let mut reasons = Vec::new();
    if baseline.suite_schema_version != candidate.suite_schema_version {
        reasons.push(ControllerSpecializationComparisonReasonCode::SuiteSchemaMismatch);
    }
    if baseline.scenario_count != candidate.scenario_count
        || baseline.scenarios.len() != candidate.scenarios.len()
    {
        reasons.push(ControllerSpecializationComparisonReasonCode::ScenarioCountMismatch);
    }
    if baseline
        .scenarios
        .iter()
        .map(|scenario| scenario.scenario_id.as_str())
        .ne(candidate
            .scenarios
            .iter()
            .map(|scenario| scenario.scenario_id.as_str()))
    {
        reasons.push(ControllerSpecializationComparisonReasonCode::ScenarioIdentityMismatch);
    }
    if baseline
        .scenarios
        .iter()
        .map(|scenario| scenario.capability.as_str())
        .ne(candidate
            .scenarios
            .iter()
            .map(|scenario| scenario.capability.as_str()))
    {
        reasons.push(ControllerSpecializationComparisonReasonCode::CapabilityIdentityMismatch);
    }
    if baseline
        .scenarios
        .iter()
        .map(|scenario| &scenario.expected)
        .ne(candidate
            .scenarios
            .iter()
            .map(|scenario| &scenario.expected))
    {
        reasons.push(ControllerSpecializationComparisonReasonCode::ExpectedSemanticMismatch);
    }
    if baseline
        .scenarios
        .iter()
        .map(|scenario| &scenario.acceptable_alternatives)
        .ne(candidate
            .scenarios
            .iter()
            .map(|scenario| &scenario.acceptable_alternatives))
    {
        reasons.push(ControllerSpecializationComparisonReasonCode::AcceptableAlternativesMismatch);
    }
    if baseline.runtime.requests != candidate.runtime.requests {
        reasons.push(ControllerSpecializationComparisonReasonCode::RequestParametersMismatch);
    }
    if baseline.runtime.backend != candidate.runtime.backend
        || baseline.runtime.context_tokens != candidate.runtime.context_tokens
        || baseline.runtime.threads != candidate.runtime.threads
    {
        reasons.push(ControllerSpecializationComparisonReasonCode::RuntimeConfigurationMismatch);
    }
    reasons
}

fn compare_scenario(
    baseline: &ControllerSpecializationBaselineScenario,
    candidate: &ControllerSpecializationBaselineScenario,
) -> Result<ControllerSpecializationScenarioComparison, ControllerSpecializationComparisonError> {
    let baseline_outcome = ControllerSpecializationScenarioOutcomeClass::from_parts(
        baseline.status,
        baseline.failure.as_ref(),
    )?;
    let candidate_outcome = ControllerSpecializationScenarioOutcomeClass::from_parts(
        candidate.status,
        candidate.failure.as_ref(),
    )?;
    Ok(ControllerSpecializationScenarioComparison {
        scenario_id: baseline.scenario_id.clone(),
        capability: baseline.capability.clone(),
        expected: baseline.expected.clone(),
        acceptable_alternatives: baseline.acceptable_alternatives.clone(),
        baseline_status: baseline.status,
        candidate_status: candidate.status,
        baseline_outcome,
        candidate_outcome,
        transition: ControllerSpecializationScenarioTransition::classify(
            baseline_outcome,
            candidate_outcome,
        ),
    })
}

fn compare_capability(
    baseline: &ControllerSpecializationCapabilityAggregate,
    candidate: &ControllerSpecializationCapabilityAggregate,
) -> ControllerSpecializationCapabilityDelta {
    let baseline_counts = ControllerSpecializationAggregate {
        total: baseline.total,
        passed: baseline.passed,
        incorrect: baseline.incorrect,
        validation_failures: baseline.validation_failures,
        runtime_failures: baseline.runtime_failures,
    };
    let candidate_counts = ControllerSpecializationAggregate {
        total: candidate.total,
        passed: candidate.passed,
        incorrect: candidate.incorrect,
        validation_failures: candidate.validation_failures,
        runtime_failures: candidate.runtime_failures,
    };
    ControllerSpecializationCapabilityDelta {
        capability: baseline.capability.clone(),
        baseline: baseline.clone(),
        candidate: candidate.clone(),
        delta: ControllerSpecializationCountDelta::between(&candidate_counts, &baseline_counts),
    }
}

fn gate_reasons(
    baseline_model: &ControllerSpecializationBaselineModelIdentity,
    candidate_model: &ControllerSpecializationBaselineModelIdentity,
    global: &ControllerSpecializationGlobalDelta,
    capabilities: &[ControllerSpecializationCapabilityDelta],
    scenarios: &[ControllerSpecializationScenarioComparison],
) -> Vec<ControllerSpecializationComparisonReasonCode> {
    if baseline_model == candidate_model {
        return vec![ControllerSpecializationComparisonReasonCode::SelfComparison];
    }
    let mut reasons = Vec::new();
    if global.candidate.passed <= global.baseline.passed {
        reasons.push(ControllerSpecializationComparisonReasonCode::StrictGlobalImprovementRequired);
    }
    if execution_error_count(&global.candidate) > execution_error_count(&global.baseline) {
        reasons.push(ControllerSpecializationComparisonReasonCode::ExecutionErrorsIncreased);
    }
    if capabilities
        .iter()
        .any(|capability| capability.candidate.passed < capability.baseline.passed)
    {
        reasons.push(ControllerSpecializationComparisonReasonCode::CapabilityPassRegression);
    }
    if scenarios.iter().any(|scenario| {
        scenario.baseline_outcome == ControllerSpecializationScenarioOutcomeClass::Pass
            && scenario.candidate_outcome != ControllerSpecializationScenarioOutcomeClass::Pass
    }) {
        reasons.push(ControllerSpecializationComparisonReasonCode::BaselinePassScenarioRegression);
    }
    if scenarios.iter().any(|scenario| {
        scenario.candidate_outcome == ControllerSpecializationScenarioOutcomeClass::Parse
            && scenario.baseline_outcome != ControllerSpecializationScenarioOutcomeClass::Parse
    }) {
        reasons.push(ControllerSpecializationComparisonReasonCode::NewParseFailure);
    }
    if scenarios.iter().any(|scenario| {
        scenario.candidate_outcome == ControllerSpecializationScenarioOutcomeClass::Validation
            && scenario.baseline_outcome != ControllerSpecializationScenarioOutcomeClass::Validation
    }) {
        reasons.push(ControllerSpecializationComparisonReasonCode::NewValidationFailure);
    }
    if scenarios.iter().any(|scenario| {
        scenario.candidate_outcome == ControllerSpecializationScenarioOutcomeClass::Runtime
            && scenario.baseline_outcome != ControllerSpecializationScenarioOutcomeClass::Runtime
    }) {
        reasons.push(ControllerSpecializationComparisonReasonCode::NewRuntimeFailure);
    }
    reasons
}

fn decision_for(
    baseline_model: &ControllerSpecializationBaselineModelIdentity,
    candidate_model: &ControllerSpecializationBaselineModelIdentity,
    reasons: &[ControllerSpecializationComparisonReasonCode],
) -> ControllerSpecializationComparisonDecision {
    if baseline_model == candidate_model {
        ControllerSpecializationComparisonDecision::SelfComparison
    } else if reasons.is_empty() {
        ControllerSpecializationComparisonDecision::Promote
    } else {
        ControllerSpecializationComparisonDecision::Reject
    }
}

fn execution_error_count(aggregate: &ControllerSpecializationAggregate) -> u64 {
    aggregate
        .validation_failures
        .saturating_add(aggregate.runtime_failures)
}

fn validate_reasons(
    reasons: &[ControllerSpecializationComparisonReasonCode],
) -> Result<(), ControllerSpecializationComparisonError> {
    if reasons.windows(2).any(|window| window[0] >= window[1]) {
        return Err(
            ControllerSpecializationComparisonError::InvalidComparisonReport(
                "reasons must be ordered and duplicate-free".into(),
            ),
        );
    }
    Ok(())
}

fn is_comparability_reason(reason: ControllerSpecializationComparisonReasonCode) -> bool {
    matches!(
        reason,
        ControllerSpecializationComparisonReasonCode::SuiteSchemaMismatch
            | ControllerSpecializationComparisonReasonCode::ScenarioCountMismatch
            | ControllerSpecializationComparisonReasonCode::ScenarioIdentityMismatch
            | ControllerSpecializationComparisonReasonCode::CapabilityIdentityMismatch
            | ControllerSpecializationComparisonReasonCode::ExpectedSemanticMismatch
            | ControllerSpecializationComparisonReasonCode::AcceptableAlternativesMismatch
            | ControllerSpecializationComparisonReasonCode::RequestParametersMismatch
            | ControllerSpecializationComparisonReasonCode::RuntimeConfigurationMismatch
    )
}

fn status_matches_class(
    status: ControllerSpecializationScenarioStatus,
    class: ControllerSpecializationScenarioOutcomeClass,
) -> bool {
    matches!(
        (status, class),
        (
            ControllerSpecializationScenarioStatus::Pass,
            ControllerSpecializationScenarioOutcomeClass::Pass,
        ) | (
            ControllerSpecializationScenarioStatus::IncorrectResult,
            ControllerSpecializationScenarioOutcomeClass::IncorrectResult,
        ) | (
            ControllerSpecializationScenarioStatus::ObservedValidationFailure,
            ControllerSpecializationScenarioOutcomeClass::Parse,
        ) | (
            ControllerSpecializationScenarioStatus::ObservedValidationFailure,
            ControllerSpecializationScenarioOutcomeClass::Validation,
        ) | (
            ControllerSpecializationScenarioStatus::RuntimeFailure,
            ControllerSpecializationScenarioOutcomeClass::Runtime,
        )
    )
}

fn validate_global_delta(
    global: &ControllerSpecializationGlobalDelta,
    scenarios: &[ControllerSpecializationScenarioComparison],
) -> Result<(), ControllerSpecializationComparisonError> {
    validate_aggregate(&global.baseline)?;
    validate_aggregate(&global.candidate)?;
    validate_count_delta(&global.delta, &global.candidate, &global.baseline)?;
    let (baseline, candidate) = aggregate_from_scenarios(scenarios);
    if global.baseline != baseline || global.candidate != candidate {
        return Err(
            ControllerSpecializationComparisonError::InvalidComparisonReport(
                "global delta aggregates do not equal scenario evidence".into(),
            ),
        );
    }
    Ok(())
}

fn validate_capability_delta(
    capability: &ControllerSpecializationCapabilityDelta,
    scenarios: &[ControllerSpecializationScenarioComparison],
) -> Result<(), ControllerSpecializationComparisonError> {
    if capability.baseline.capability != capability.capability
        || capability.candidate.capability != capability.capability
    {
        return Err(
            ControllerSpecializationComparisonError::InvalidComparisonReport(
                "capability delta identity is inconsistent".into(),
            ),
        );
    }
    validate_capability_aggregate(&capability.baseline)?;
    validate_capability_aggregate(&capability.candidate)?;
    let baseline = aggregate_from_capability_scenarios(scenarios, &capability.capability, false);
    let candidate = aggregate_from_capability_scenarios(scenarios, &capability.capability, true);
    if capability.baseline != baseline || capability.candidate != candidate {
        return Err(
            ControllerSpecializationComparisonError::InvalidComparisonReport(
                "capability delta aggregates do not equal scenario evidence".into(),
            ),
        );
    }
    let baseline_counts = aggregate_from_capability(&capability.baseline);
    let candidate_counts = aggregate_from_capability(&capability.candidate);
    validate_count_delta(&capability.delta, &candidate_counts, &baseline_counts)
}

fn validate_count_delta(
    delta: &ControllerSpecializationCountDelta,
    candidate: &ControllerSpecializationAggregate,
    baseline: &ControllerSpecializationAggregate,
) -> Result<(), ControllerSpecializationComparisonError> {
    delta.validate()?;
    if *delta != ControllerSpecializationCountDelta::between(candidate, baseline) {
        return Err(
            ControllerSpecializationComparisonError::InvalidComparisonReport(
                "signed count delta is inconsistent".into(),
            ),
        );
    }
    Ok(())
}

fn validate_aggregate(
    aggregate: &ControllerSpecializationAggregate,
) -> Result<(), ControllerSpecializationComparisonError> {
    let sum = aggregate
        .passed
        .checked_add(aggregate.incorrect)
        .and_then(|sum| sum.checked_add(aggregate.validation_failures))
        .and_then(|sum| sum.checked_add(aggregate.runtime_failures));
    if sum != Some(aggregate.total) {
        return Err(
            ControllerSpecializationComparisonError::InvalidComparisonReport(
                "global aggregate does not equal its status counts".into(),
            ),
        );
    }
    Ok(())
}

fn validate_capability_aggregate(
    aggregate: &ControllerSpecializationCapabilityAggregate,
) -> Result<(), ControllerSpecializationComparisonError> {
    let sum = aggregate
        .passed
        .checked_add(aggregate.incorrect)
        .and_then(|sum| sum.checked_add(aggregate.validation_failures))
        .and_then(|sum| sum.checked_add(aggregate.runtime_failures));
    if sum != Some(aggregate.total) {
        return Err(
            ControllerSpecializationComparisonError::InvalidComparisonReport(
                "capability aggregate does not equal its status counts".into(),
            ),
        );
    }
    Ok(())
}

fn aggregate_from_scenarios(
    scenarios: &[ControllerSpecializationScenarioComparison],
) -> (
    ControllerSpecializationAggregate,
    ControllerSpecializationAggregate,
) {
    let mut baseline = ControllerSpecializationAggregate::default();
    let mut candidate = ControllerSpecializationAggregate::default();
    for scenario in scenarios {
        add_outcome(&mut baseline, scenario.baseline_outcome);
        add_outcome(&mut candidate, scenario.candidate_outcome);
    }
    (baseline, candidate)
}

fn aggregate_from_capability_scenarios(
    scenarios: &[ControllerSpecializationScenarioComparison],
    capability: &str,
    candidate: bool,
) -> ControllerSpecializationCapabilityAggregate {
    let mut aggregate = ControllerSpecializationCapabilityAggregate {
        capability: capability.into(),
        ..Default::default()
    };
    for scenario in scenarios
        .iter()
        .filter(|scenario| scenario.capability == capability)
    {
        aggregate.total += 1;
        let outcome = if candidate {
            scenario.candidate_outcome
        } else {
            scenario.baseline_outcome
        };
        match outcome {
            ControllerSpecializationScenarioOutcomeClass::Pass => aggregate.passed += 1,
            ControllerSpecializationScenarioOutcomeClass::IncorrectResult => {
                aggregate.incorrect += 1
            }
            ControllerSpecializationScenarioOutcomeClass::Parse
            | ControllerSpecializationScenarioOutcomeClass::Validation => {
                aggregate.validation_failures += 1
            }
            ControllerSpecializationScenarioOutcomeClass::Runtime => {
                aggregate.runtime_failures += 1
            }
        }
    }
    aggregate
}

fn aggregate_from_capability(
    aggregate: &ControllerSpecializationCapabilityAggregate,
) -> ControllerSpecializationAggregate {
    ControllerSpecializationAggregate {
        total: aggregate.total,
        passed: aggregate.passed,
        incorrect: aggregate.incorrect,
        validation_failures: aggregate.validation_failures,
        runtime_failures: aggregate.runtime_failures,
    }
}

fn add_outcome(
    aggregate: &mut ControllerSpecializationAggregate,
    outcome: ControllerSpecializationScenarioOutcomeClass,
) {
    aggregate.total += 1;
    match outcome {
        ControllerSpecializationScenarioOutcomeClass::Pass => aggregate.passed += 1,
        ControllerSpecializationScenarioOutcomeClass::IncorrectResult => aggregate.incorrect += 1,
        ControllerSpecializationScenarioOutcomeClass::Parse
        | ControllerSpecializationScenarioOutcomeClass::Validation => {
            aggregate.validation_failures += 1
        }
        ControllerSpecializationScenarioOutcomeClass::Runtime => aggregate.runtime_failures += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_specialization_baseline::{
        ControllerSpecializationBaselineFailure, ControllerSpecializationBaselineModelIdentity,
        ControllerSpecializationBaselineReport, ControllerSpecializationBaselineRuntime,
        ControllerSpecializationBaselineRuntimeRequest,
    };
    use crate::controller_specialization_evaluation::{
        ControllerSpecializationEvaluationReport, ControllerSpecializationSuite,
        evaluate_controller_specialization,
    };
    use crate::local_runtime::{LocalInferenceResponse, LocalInferenceRuntime, LocalRuntimeConfig};
    use std::collections::VecDeque;

    struct FakeRuntime {
        responses:
            VecDeque<Result<LocalInferenceResponse, crate::local_runtime::LocalInferenceError>>,
        requests: Vec<crate::local_runtime::LocalInferenceRequest>,
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            request: &crate::local_runtime::LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, crate::local_runtime::LocalInferenceError> {
            self.requests.push(request.clone());
            self.responses.pop_front().unwrap()
        }
    }

    fn reports(
        candidate_model: &str,
        mutate_candidate: impl FnOnce(&mut ControllerSpecializationEvaluationReport),
    ) -> (
        ControllerSpecializationBaselineReport,
        ControllerSpecializationBaselineReport,
    ) {
        reports_with_mutations("baseline.gguf", candidate_model, |_| {}, mutate_candidate)
    }

    fn reports_with_mutations(
        baseline_model: &str,
        candidate_model: &str,
        mutate_baseline: impl FnOnce(&mut ControllerSpecializationEvaluationReport),
        mutate_candidate: impl FnOnce(&mut ControllerSpecializationEvaluationReport),
    ) -> (
        ControllerSpecializationBaselineReport,
        ControllerSpecializationBaselineReport,
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
        let mut baseline_evaluation =
            evaluate_controller_specialization(&suite, &mut runtime).unwrap();
        let mut candidate_evaluation = baseline_evaluation.clone();
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
        let runtime_metadata = ControllerSpecializationBaselineRuntime::from_llama_cpp_config(
            &LocalRuntimeConfig::new("model.gguf"),
            requests,
        )
        .unwrap();
        mutate_baseline(&mut baseline_evaluation);
        let baseline = ControllerSpecializationBaselineReport::from_evaluation_report(
            &suite,
            &baseline_evaluation,
            ControllerSpecializationBaselineModelIdentity::new(baseline_model).unwrap(),
            runtime_metadata.clone(),
        )
        .unwrap();
        mutate_candidate(&mut candidate_evaluation);
        let candidate = ControllerSpecializationBaselineReport::from_evaluation_report(
            &suite,
            &candidate_evaluation,
            ControllerSpecializationBaselineModelIdentity::new(candidate_model).unwrap(),
            runtime_metadata,
        )
        .unwrap();
        (baseline, candidate)
    }

    fn mark_incorrect(report: &mut ControllerSpecializationEvaluationReport, index: usize) {
        let capability = report.scenarios[index].capability.clone();
        report.scenarios[index].status = ControllerSpecializationScenarioStatus::IncorrectResult;
        report.scenarios[index].observed = Some(report.scenarios[index].expected.clone());
        report.scenarios[index].failure = None;
        report.aggregate.passed -= 1;
        report.aggregate.incorrect += 1;
        let aggregate = report
            .capabilities
            .iter_mut()
            .find(|aggregate| aggregate.capability == capability)
            .unwrap();
        aggregate.passed -= 1;
        aggregate.incorrect += 1;
    }

    fn mark_failure(
        report: &mut ControllerSpecializationEvaluationReport,
        index: usize,
        status: ControllerSpecializationScenarioStatus,
        failure: ControllerSpecializationBaselineFailure,
    ) {
        let capability = report.scenarios[index].capability.clone();
        report.scenarios[index].status = status;
        report.scenarios[index].observed = None;
        report.scenarios[index].failure = Some(failure);
        report.aggregate.passed -= 1;
        let aggregate = report
            .capabilities
            .iter_mut()
            .find(|aggregate| aggregate.capability == capability)
            .unwrap();
        aggregate.passed -= 1;
        match status {
            ControllerSpecializationScenarioStatus::ObservedValidationFailure => {
                report.aggregate.validation_failures += 1;
                aggregate.validation_failures += 1;
            }
            ControllerSpecializationScenarioStatus::RuntimeFailure => {
                report.aggregate.runtime_failures += 1;
                aggregate.runtime_failures += 1;
            }
            ControllerSpecializationScenarioStatus::Pass
            | ControllerSpecializationScenarioStatus::IncorrectResult => {
                panic!("failure helper received a non-failure status")
            }
        }
    }

    fn mark_outcome(
        report: &mut ControllerSpecializationEvaluationReport,
        index: usize,
        outcome: ControllerSpecializationScenarioOutcomeClass,
    ) {
        match outcome {
            ControllerSpecializationScenarioOutcomeClass::Pass => {}
            ControllerSpecializationScenarioOutcomeClass::IncorrectResult => {
                mark_incorrect(report, index)
            }
            ControllerSpecializationScenarioOutcomeClass::Parse => mark_failure(
                report,
                index,
                ControllerSpecializationScenarioStatus::ObservedValidationFailure,
                ControllerSpecializationBaselineFailure::Parse {
                    raw_output: "{".into(),
                    parse_error: "unexpected end".into(),
                },
            ),
            ControllerSpecializationScenarioOutcomeClass::Validation => mark_failure(
                report,
                index,
                ControllerSpecializationScenarioStatus::ObservedValidationFailure,
                ControllerSpecializationBaselineFailure::Validation {
                    error: "typed output is invalid".into(),
                },
            ),
            ControllerSpecializationScenarioOutcomeClass::Runtime => mark_failure(
                report,
                index,
                ControllerSpecializationScenarioStatus::RuntimeFailure,
                ControllerSpecializationBaselineFailure::Runtime {
                    error: "backend failed".into(),
                },
            ),
        }
    }

    fn assert_transition(
        baseline_outcome: ControllerSpecializationScenarioOutcomeClass,
        candidate_outcome: ControllerSpecializationScenarioOutcomeClass,
    ) {
        let (baseline, candidate) = reports_with_mutations(
            "baseline.gguf",
            "candidate.gguf",
            |evaluation| mark_outcome(evaluation, 0, baseline_outcome),
            |evaluation| mark_outcome(evaluation, 0, candidate_outcome),
        );
        let report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        assert_eq!(
            report.scenarios[0].transition,
            ControllerSpecializationScenarioTransition {
                baseline: baseline_outcome,
                candidate: candidate_outcome,
            }
        );
    }

    #[test]
    fn every_typed_outcome_transition_is_preserved_exactly() {
        let outcomes = [
            ControllerSpecializationScenarioOutcomeClass::Pass,
            ControllerSpecializationScenarioOutcomeClass::IncorrectResult,
            ControllerSpecializationScenarioOutcomeClass::Parse,
            ControllerSpecializationScenarioOutcomeClass::Validation,
            ControllerSpecializationScenarioOutcomeClass::Runtime,
        ];
        for baseline in outcomes {
            for candidate in outcomes {
                assert_transition(baseline, candidate);
            }
        }
    }

    #[test]
    fn comparable_reports_promote_only_on_strict_safe_improvement() {
        let (baseline, candidate) = reports_with_mutations(
            "baseline.gguf",
            "candidate.gguf",
            |evaluation| mark_incorrect(evaluation, 0),
            |_| {},
        );
        let report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        assert_eq!(
            report.decision,
            ControllerSpecializationComparisonDecision::Promote
        );
        assert_eq!(
            report.scenarios[0].transition,
            ControllerSpecializationScenarioTransition {
                baseline: ControllerSpecializationScenarioOutcomeClass::IncorrectResult,
                candidate: ControllerSpecializationScenarioOutcomeClass::Pass,
            }
        );
        assert_eq!(
            report.global.as_ref().unwrap().delta.passed,
            ControllerSpecializationSignedDelta::Positive(1)
        );
        assert_eq!(
            report.global.as_ref().unwrap().delta.incorrect,
            ControllerSpecializationSignedDelta::Negative(1)
        );
        let capability = report
            .capabilities
            .iter()
            .find(|capability| capability.capability == baseline.scenarios[0].capability)
            .unwrap();
        assert_eq!(
            capability.delta.passed,
            ControllerSpecializationSignedDelta::Positive(1)
        );

        let (baseline, candidate) = reports("candidate.gguf", |_| {});
        let report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        assert_eq!(
            report.decision,
            ControllerSpecializationComparisonDecision::Reject
        );
        assert!(report.reasons.contains(
            &ControllerSpecializationComparisonReasonCode::StrictGlobalImprovementRequired
        ));
    }

    #[test]
    fn self_comparison_is_explicitly_non_promotable() {
        let (baseline, candidate) = reports("baseline.gguf", |_| {});
        let report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        assert_eq!(
            report.decision,
            ControllerSpecializationComparisonDecision::SelfComparison
        );
        assert_eq!(
            report.reasons,
            vec![ControllerSpecializationComparisonReasonCode::SelfComparison]
        );
    }

    #[test]
    fn exact_deltas_and_serialization_are_deterministic() {
        let (baseline, candidate) = reports("candidate.gguf", |_| {});
        let first = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        let second = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert_eq!(
            first.global.as_ref().unwrap().delta.passed,
            ControllerSpecializationSignedDelta::Zero
        );
        assert_eq!(first.capabilities.len(), 9);
    }

    #[test]
    fn capability_and_scenario_regressions_reject_even_with_global_improvement() {
        let (baseline, candidate) = reports_with_mutations(
            "baseline.gguf",
            "candidate.gguf",
            |evaluation| {
                mark_incorrect(evaluation, 0);
                mark_incorrect(evaluation, 1);
            },
            |evaluation| mark_incorrect(evaluation, 2),
        );
        let report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        assert_eq!(report.global.as_ref().unwrap().baseline.passed, 15);
        assert_eq!(report.global.as_ref().unwrap().candidate.passed, 16);
        assert_eq!(
            report.decision,
            ControllerSpecializationComparisonDecision::Reject
        );
        assert!(
            report
                .reasons
                .contains(&ControllerSpecializationComparisonReasonCode::CapabilityPassRegression)
        );
        assert!(report.reasons.contains(
            &ControllerSpecializationComparisonReasonCode::BaselinePassScenarioRegression
        ));
    }

    #[test]
    fn new_failure_classes_reject_and_existing_same_class_is_not_new() {
        let failures = [
            (
                ControllerSpecializationScenarioStatus::ObservedValidationFailure,
                ControllerSpecializationBaselineFailure::Parse {
                    raw_output: "{".into(),
                    parse_error: "unexpected end".into(),
                },
                ControllerSpecializationComparisonReasonCode::NewParseFailure,
            ),
            (
                ControllerSpecializationScenarioStatus::ObservedValidationFailure,
                ControllerSpecializationBaselineFailure::Validation {
                    error: "typed output is invalid".into(),
                },
                ControllerSpecializationComparisonReasonCode::NewValidationFailure,
            ),
            (
                ControllerSpecializationScenarioStatus::RuntimeFailure,
                ControllerSpecializationBaselineFailure::Runtime {
                    error: "backend failed".into(),
                },
                ControllerSpecializationComparisonReasonCode::NewRuntimeFailure,
            ),
        ];
        for (status, failure, reason) in failures {
            let (baseline, candidate) = reports("candidate.gguf", |evaluation| {
                mark_failure(evaluation, 0, status, failure)
            });
            let report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
            assert_eq!(
                report.decision,
                ControllerSpecializationComparisonDecision::Reject
            );
            assert!(report.reasons.contains(&reason));
            assert!(matches!(
                report.scenarios[0].candidate_outcome,
                ControllerSpecializationScenarioOutcomeClass::Parse
                    | ControllerSpecializationScenarioOutcomeClass::Validation
                    | ControllerSpecializationScenarioOutcomeClass::Runtime
            ));
        }

        let (baseline, candidate) = reports_with_mutations(
            "baseline.gguf",
            "candidate.gguf",
            |evaluation| {
                mark_failure(
                    evaluation,
                    0,
                    ControllerSpecializationScenarioStatus::ObservedValidationFailure,
                    ControllerSpecializationBaselineFailure::Parse {
                        raw_output: "{".into(),
                        parse_error: "unexpected end".into(),
                    },
                )
            },
            |evaluation| {
                mark_failure(
                    evaluation,
                    0,
                    ControllerSpecializationScenarioStatus::ObservedValidationFailure,
                    ControllerSpecializationBaselineFailure::Parse {
                        raw_output: "different bounded output".into(),
                        parse_error: "different diagnostic".into(),
                    },
                )
            },
        );
        let report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        assert!(
            !report
                .reasons
                .contains(&ControllerSpecializationComparisonReasonCode::NewParseFailure)
        );
    }

    #[test]
    fn comparison_validation_rejects_tampered_transitions_deltas_and_reasons() {
        let (baseline, candidate) = reports_with_mutations(
            "baseline.gguf",
            "candidate.gguf",
            |evaluation| mark_incorrect(evaluation, 0),
            |_| {},
        );
        let mut report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        report.scenarios[0].transition.baseline =
            ControllerSpecializationScenarioOutcomeClass::Pass;
        assert!(matches!(
            report.validate(),
            Err(ControllerSpecializationComparisonError::InvalidComparisonReport(_))
        ));

        let mut report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        report.scenarios[0].transition.candidate =
            ControllerSpecializationScenarioOutcomeClass::IncorrectResult;
        assert!(matches!(
            report.validate(),
            Err(ControllerSpecializationComparisonError::InvalidComparisonReport(_))
        ));

        let mut report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        report.global.as_mut().unwrap().delta.passed =
            ControllerSpecializationSignedDelta::Positive(2);
        assert!(report.validate().is_err());

        let mut report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        report
            .reasons
            .push(ControllerSpecializationComparisonReasonCode::StrictGlobalImprovementRequired);
        assert!(report.validate().is_err());
    }

    #[test]
    fn malformed_reports_and_comparability_mismatches_fail_closed() {
        let (baseline, candidate) = reports("candidate.gguf", |_| {});
        let mut malformed = candidate.clone();
        malformed.schema_version += 1;
        assert!(matches!(
            compare_controller_specialization_reports(&baseline, &malformed),
            Err(ControllerSpecializationComparisonError::InvalidCandidateReport(_))
        ));

        let mut changed = candidate.clone();
        changed.runtime.context_tokens += 1;
        changed.runtime.requests[0].parameters.max_output_tokens += 1;
        let report = compare_controller_specialization_reports(&baseline, &changed).unwrap();
        assert_eq!(
            report.decision,
            ControllerSpecializationComparisonDecision::NonComparable
        );
        assert!(
            report.reasons.contains(
                &ControllerSpecializationComparisonReasonCode::RuntimeConfigurationMismatch
            )
        );
        assert!(
            report
                .reasons
                .contains(&ControllerSpecializationComparisonReasonCode::RequestParametersMismatch)
        );

        let same_capability = candidate
            .scenarios
            .iter()
            .enumerate()
            .find_map(|(index, scenario)| {
                candidate
                    .scenarios
                    .iter()
                    .enumerate()
                    .skip(index + 1)
                    .find(|(_, other)| other.capability == scenario.capability)
                    .map(|(other_index, _)| (index, other_index))
            })
            .unwrap();
        let mut expected_changed = candidate.clone();
        expected_changed.scenarios[same_capability.0].expected = expected_changed.scenarios
            [same_capability.1]
            .expected
            .clone();
        assert!(matches!(
            compare_controller_specialization_reports(&baseline, &expected_changed),
            Err(ControllerSpecializationComparisonError::InvalidCandidateReport(_))
        ));

        let mut alternatives_changed = candidate.clone();
        let alternative = alternatives_changed.scenarios[same_capability.0]
            .expected
            .clone();
        alternatives_changed.scenarios[same_capability.0]
            .acceptable_alternatives
            .push(alternative);
        assert!(matches!(
            compare_controller_specialization_reports(&baseline, &alternatives_changed),
            Err(ControllerSpecializationComparisonError::InvalidCandidateReport(_))
        ));

        let mut reordered = candidate.clone();
        reordered.scenarios.swap(0, 1);
        assert!(matches!(
            compare_controller_specialization_reports(&baseline, &reordered),
            Err(ControllerSpecializationComparisonError::InvalidCandidateReport(_))
        ));
    }

    #[test]
    fn promotion_reasons_cover_global_capability_scenario_and_new_failures() {
        let (baseline, mut candidate) = reports("candidate.gguf", |_| {});
        candidate.scenarios[0].status = ControllerSpecializationScenarioStatus::IncorrectResult;
        candidate.scenarios[0].observed = Some(candidate.scenarios[0].expected.clone());
        candidate.aggregate.passed -= 1;
        candidate.aggregate.incorrect += 1;
        candidate.capabilities[0].passed -= 1;
        candidate.capabilities[0].incorrect += 1;
        let report = compare_controller_specialization_reports(&baseline, &candidate).unwrap();
        assert_eq!(
            report.decision,
            ControllerSpecializationComparisonDecision::Reject
        );
        assert!(report.reasons.contains(
            &ControllerSpecializationComparisonReasonCode::StrictGlobalImprovementRequired
        ));
        assert!(
            report
                .reasons
                .contains(&ControllerSpecializationComparisonReasonCode::CapabilityPassRegression)
        );
        assert!(report.reasons.contains(
            &ControllerSpecializationComparisonReasonCode::BaselinePassScenarioRegression
        ));
    }
}
