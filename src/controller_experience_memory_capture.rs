//! Explicit curation for the Controller memory-capture judgment capability.
//!
//! This module adapts one already-produced, typed capture judgment into the
//! canonical M08-001 dataset record. It performs no inference, storage lookup,
//! mutation proposal, grant handling, execution, or automatic verification.

use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use crate::controller_memory_capture::{
    ControllerMemoryCaptureError, ControllerMemoryCaptureInput, ControllerMemoryCaptureResult,
};
use thiserror::Error;

/// Fixed code-owned M08 capability identity for memory-capture judgment.
pub const CONTROLLER_MEMORY_CAPTURE_EXPERIENCE_CAPABILITY: &str = "controller.memory_capture";

/// One explicit, already-produced Controller memory-capture curation request.
/// Capability identity is intentionally absent and cannot be overridden by a
/// caller.
#[derive(Clone, Debug)]
pub struct ControllerExperienceMemoryCaptureRequest {
    pub input: ControllerMemoryCaptureInput,
    pub observed: ControllerMemoryCaptureResult,
    pub accepted: ControllerMemoryCaptureResult,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperienceMemoryCaptureError {
    #[error("Controller memory-capture input validation failed: {0}")]
    Input(#[source] ControllerMemoryCaptureError),
    #[error("observed Controller memory-capture result validation failed: {0}")]
    Observed(#[source] ControllerMemoryCaptureError),
    #[error("accepted Controller memory-capture result validation failed: {0}")]
    Accepted(#[source] ControllerMemoryCaptureError),
    #[error("invalid Controller memory-capture curation: {0}")]
    Invalid(String),
    #[error("Controller memory-capture projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperienceMemoryCaptureRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperienceMemoryCaptureError> {
        self.input
            .validate()
            .map_err(ControllerExperienceMemoryCaptureError::Input)?;
        self.observed
            .validate(&self.input.current_request.candidate)
            .map_err(ControllerExperienceMemoryCaptureError::Observed)?;
        self.accepted
            .validate(&self.input.current_request.candidate)
            .map_err(ControllerExperienceMemoryCaptureError::Accepted)?;

        let input = serde_json::to_value(&self.input)
            .map_err(ControllerExperienceMemoryCaptureError::Projection)?;
        let observed_output = serde_json::to_value(&self.observed)
            .map_err(ControllerExperienceMemoryCaptureError::Projection)?;
        let accepted_output = serde_json::to_value(&self.accepted)
            .map_err(ControllerExperienceMemoryCaptureError::Projection)?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperienceMemoryCaptureError::Invalid(
                    "equal observed and accepted capture results cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperienceMemoryCaptureError::Invalid(
                    "differing observed and accepted capture results require a corrected outcome"
                        .into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperienceMemoryCaptureError::Invalid(
                    "differing observed and accepted capture results require correction metadata"
                        .into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperienceMemoryCaptureError::Invalid(
                    "correction metadata must preserve the exact observed capture result".into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_MEMORY_CAPTURE_EXPERIENCE_CAPABILITY.into(),
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
