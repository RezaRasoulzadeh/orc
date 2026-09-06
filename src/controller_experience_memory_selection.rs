//! Explicit curation for the Controller memory-selection judgment capability.
//!
//! This module adapts one already-produced, typed target-selection judgment
//! into the canonical M08-001 dataset record. It performs no inference,
//! candidate enumeration, storage lookup, target refresh, maintenance,
//! mutation execution, or automatic verification.

use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use crate::controller_memory_selection::{
    ControllerMemorySelectionError, ControllerMemorySelectionInput, ControllerMemorySelectionResult,
};
use thiserror::Error;

/// Fixed code-owned M08 capability identity for memory-maintenance target
/// selection.
pub const CONTROLLER_MEMORY_SELECTION_EXPERIENCE_CAPABILITY: &str = "controller.memory_selection";

/// One explicit, already-produced Controller memory-selection curation
/// request. Capability identity is intentionally absent and cannot be
/// overridden by a caller.
#[derive(Clone, Debug, PartialEq)]
pub struct ControllerExperienceMemorySelectionRequest {
    pub input: ControllerMemorySelectionInput,
    pub observed: ControllerMemorySelectionResult,
    pub accepted: ControllerMemorySelectionResult,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperienceMemorySelectionError {
    #[error("Controller memory-selection input validation failed: {0}")]
    Input(#[source] ControllerMemorySelectionError),
    #[error("observed Controller memory-selection result validation failed: {0}")]
    Observed(#[source] ControllerMemorySelectionError),
    #[error("accepted Controller memory-selection result validation failed: {0}")]
    Accepted(#[source] ControllerMemorySelectionError),
    #[error("invalid Controller memory-selection curation: {0}")]
    Invalid(String),
    #[error("Controller memory-selection projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperienceMemorySelectionRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperienceMemorySelectionError> {
        self.input
            .validate()
            .map_err(ControllerExperienceMemorySelectionError::Input)?;
        self.observed
            .validate(&self.input)
            .map_err(ControllerExperienceMemorySelectionError::Observed)?;
        self.accepted
            .validate(&self.input)
            .map_err(ControllerExperienceMemorySelectionError::Accepted)?;

        let input = serde_json::to_value(&self.input)
            .map_err(ControllerExperienceMemorySelectionError::Projection)?;
        let observed_output = serde_json::to_value(&self.observed)
            .map_err(ControllerExperienceMemorySelectionError::Projection)?;
        let accepted_output = serde_json::to_value(&self.accepted)
            .map_err(ControllerExperienceMemorySelectionError::Projection)?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperienceMemorySelectionError::Invalid(
                    "equal observed and accepted selection results cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperienceMemorySelectionError::Invalid(
                    "differing observed and accepted selection results require a corrected outcome"
                        .into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperienceMemorySelectionError::Invalid(
                    "differing observed and accepted selection results require correction metadata"
                        .into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperienceMemorySelectionError::Invalid(
                    "correction metadata must preserve the exact observed selection result".into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_MEMORY_SELECTION_EXPERIENCE_CAPABILITY.into(),
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
