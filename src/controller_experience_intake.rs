//! Explicit curation for the Controller workflow-intake capability.
//!
//! This module adapts one already-produced, typed intake interaction into the
//! canonical M08-001 dataset record. It performs no inference, harvesting,
//! workflow observation, or automatic verification.

use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use crate::controller_intake::{
    ControllerIntakeError, ControllerIntakeInput, ControllerIntakeResult,
};
use thiserror::Error;

/// Fixed code-owned M08 capability identity for workflow intake.
pub const CONTROLLER_WORKFLOW_INTAKE_EXPERIENCE_CAPABILITY: &str = "controller.workflow_intake";

/// One explicit, already-produced Controller workflow-intake curation
/// request. The capability identity is intentionally absent and cannot be
/// overridden by callers.
#[derive(Clone, Debug, PartialEq)]
pub struct ControllerExperienceIntakeRequest {
    pub input: ControllerIntakeInput,
    pub observed: ControllerIntakeResult,
    pub accepted: ControllerIntakeResult,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperienceIntakeError {
    #[error("Controller intake input validation failed: {0}")]
    Input(#[source] ControllerIntakeError),
    #[error("observed Controller intake result validation failed: {0}")]
    Observed(#[source] ControllerIntakeError),
    #[error("accepted Controller intake result validation failed: {0}")]
    Accepted(#[source] ControllerIntakeError),
    #[error("invalid Controller intake curation: {0}")]
    Invalid(String),
    #[error("Controller intake projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperienceIntakeRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperienceIntakeError> {
        self.input
            .validate()
            .map_err(ControllerExperienceIntakeError::Input)?;
        self.observed
            .validate()
            .map_err(ControllerExperienceIntakeError::Observed)?;
        self.accepted
            .validate()
            .map_err(ControllerExperienceIntakeError::Accepted)?;

        let input = serde_json::to_value(&self.input)
            .map_err(ControllerExperienceIntakeError::Projection)?;
        let observed_output = serde_json::to_value(&self.observed)
            .map_err(ControllerExperienceIntakeError::Projection)?;
        let accepted_output = serde_json::to_value(&self.accepted)
            .map_err(ControllerExperienceIntakeError::Projection)?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperienceIntakeError::Invalid(
                    "equal observed and accepted outputs cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperienceIntakeError::Invalid(
                    "differing observed and accepted outputs require a corrected outcome".into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperienceIntakeError::Invalid(
                    "differing observed and accepted outputs require correction metadata".into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperienceIntakeError::Invalid(
                    "correction metadata must preserve the exact observed Controller intake result"
                        .into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_WORKFLOW_INTAKE_EXPERIENCE_CAPABILITY.into(),
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
