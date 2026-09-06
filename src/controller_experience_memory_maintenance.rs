//! Explicit curation for the Controller memory-maintenance judgment capability.
//!
//! This module adapts one already-produced, typed maintenance judgment into
//! the canonical M08-001 dataset record. It performs no inference, storage
//! lookup, target selection, mutation execution, grant handling, or automatic
//! verification.

use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use crate::controller_memory_maintenance::{
    ControllerMemoryMaintenanceError, ControllerMemoryMaintenanceInput,
    ControllerMemoryMaintenanceResult,
};
use thiserror::Error;

/// Fixed code-owned M08 capability identity for memory-maintenance judgment.
pub const CONTROLLER_MEMORY_MAINTENANCE_EXPERIENCE_CAPABILITY: &str =
    "controller.memory_maintenance";

/// One explicit, already-produced Controller memory-maintenance curation
/// request. Capability identity is intentionally absent and cannot be
/// overridden by a caller.
#[derive(Clone, Debug, PartialEq)]
pub struct ControllerExperienceMemoryMaintenanceRequest {
    pub input: ControllerMemoryMaintenanceInput,
    pub observed: ControllerMemoryMaintenanceResult,
    pub accepted: ControllerMemoryMaintenanceResult,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperienceMemoryMaintenanceError {
    #[error("Controller memory maintenance input validation failed: {0}")]
    Input(#[source] ControllerMemoryMaintenanceError),
    #[error("observed Controller memory maintenance result validation failed: {0}")]
    Observed(#[source] ControllerMemoryMaintenanceError),
    #[error("accepted Controller memory maintenance result validation failed: {0}")]
    Accepted(#[source] ControllerMemoryMaintenanceError),
    #[error("invalid Controller memory maintenance curation: {0}")]
    Invalid(String),
    #[error("Controller memory maintenance projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperienceMemoryMaintenanceRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperienceMemoryMaintenanceError> {
        self.input
            .validate()
            .map_err(ControllerExperienceMemoryMaintenanceError::Input)?;
        self.observed
            .validate(&self.input)
            .map_err(ControllerExperienceMemoryMaintenanceError::Observed)?;
        self.accepted
            .validate(&self.input)
            .map_err(ControllerExperienceMemoryMaintenanceError::Accepted)?;

        let input = serde_json::to_value(&self.input)
            .map_err(ControllerExperienceMemoryMaintenanceError::Projection)?;
        let observed_output = serde_json::to_value(&self.observed)
            .map_err(ControllerExperienceMemoryMaintenanceError::Projection)?;
        let accepted_output = serde_json::to_value(&self.accepted)
            .map_err(ControllerExperienceMemoryMaintenanceError::Projection)?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperienceMemoryMaintenanceError::Invalid(
                    "equal observed and accepted maintenance results cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperienceMemoryMaintenanceError::Invalid(
                    "differing observed and accepted maintenance results require a corrected outcome".into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperienceMemoryMaintenanceError::Invalid(
                    "differing observed and accepted maintenance results require correction metadata".into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperienceMemoryMaintenanceError::Invalid(
                    "correction metadata must preserve the exact observed maintenance result"
                        .into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_MEMORY_MAINTENANCE_EXPERIENCE_CAPABILITY.into(),
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
