//! Explicit curation for the Controller planning capability.
//!
//! This module adapts one already-produced, typed planning interaction into
//! the canonical M08-001 dataset record. It performs no inference, planning
//! execution, workflow observation, or automatic verification.

use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use crate::controller_planning::{
    ControllerPlanResult, ControllerPlanningError, ControllerPlanningInput,
};
use thiserror::Error;

/// Fixed code-owned M08 capability identity for Controller plan generation.
pub const CONTROLLER_PLAN_GENERATION_EXPERIENCE_CAPABILITY: &str = "controller.plan_generation";

/// One explicit, already-produced planning result curation request. The
/// capability identity is intentionally absent and cannot be overridden by
/// callers.
#[derive(Clone, Debug)]
pub struct ControllerExperiencePlanningRequest {
    pub input: ControllerPlanningInput,
    pub observed: ControllerPlanResult,
    pub accepted: ControllerPlanResult,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperiencePlanningError {
    #[error("planning input validation failed: {0}")]
    Input(#[source] ControllerPlanningError),
    #[error("observed planning result validation failed: {0}")]
    Observed(#[source] ControllerPlanningError),
    #[error("accepted planning result validation failed: {0}")]
    Accepted(#[source] ControllerPlanningError),
    #[error("invalid planning curation: {0}")]
    Invalid(String),
    #[error("planning result projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperiencePlanningRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperiencePlanningError> {
        self.input
            .validate()
            .map_err(ControllerExperiencePlanningError::Input)?;
        self.observed
            .validate()
            .map_err(ControllerExperiencePlanningError::Observed)?;
        self.accepted
            .validate()
            .map_err(ControllerExperiencePlanningError::Accepted)?;

        let input = serde_json::to_value(&self.input)
            .map_err(ControllerExperiencePlanningError::Projection)?;
        let observed_output = serde_json::to_value(&self.observed)
            .map_err(ControllerExperiencePlanningError::Projection)?;
        let accepted_output = serde_json::to_value(&self.accepted)
            .map_err(ControllerExperiencePlanningError::Projection)?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperiencePlanningError::Invalid(
                    "equal observed and accepted outputs cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperiencePlanningError::Invalid(
                    "differing observed and accepted outputs require a corrected outcome".into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperiencePlanningError::Invalid(
                    "differing observed and accepted outputs require correction metadata".into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperiencePlanningError::Invalid(
                    "correction metadata must preserve the exact observed planning result".into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_PLAN_GENERATION_EXPERIENCE_CAPABILITY.into(),
            input,
            accepted_output,
            verification_basis: self.verification_basis,
            provenance: self.provenance.clone(),
            correction,
            outcome: self.outcome,
            quality: self.quality.clone(),
        })
    }
}
