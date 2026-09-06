//! Explicit curation for the Controller recovery-recommendation capability.
//!
//! This module adapts one already-produced, typed recovery interaction into
//! the canonical M08-001 dataset record. It performs no inference, recovery
//! execution, workflow observation, or automatic verification.

use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use crate::recovery_controller::{
    RecoveryControllerError, RecoveryInferenceInput, RecoveryRecommendation,
};
use thiserror::Error;

/// Fixed code-owned M08 capability identity for recovery recommendation.
pub const CONTROLLER_RECOVERY_RECOMMENDATION_EXPERIENCE_CAPABILITY: &str =
    "controller.recovery_recommendation";

/// One explicit, already-produced recovery recommendation curation request.
/// The capability identity is intentionally absent and cannot be overridden by
/// callers.
#[derive(Clone, Debug, PartialEq)]
pub struct ControllerExperienceRecoveryRecommendationRequest {
    pub input: RecoveryInferenceInput,
    pub observed: RecoveryRecommendation,
    pub accepted: RecoveryRecommendation,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperienceRecoveryRecommendationError {
    #[error("recovery inference input validation failed: {0}")]
    Input(#[source] RecoveryControllerError),
    #[error("observed recovery recommendation validation failed: {0}")]
    Observed(#[source] RecoveryControllerError),
    #[error("accepted recovery recommendation validation failed: {0}")]
    Accepted(#[source] RecoveryControllerError),
    #[error("recovery provenance task identity mismatch: expected {expected}, actual {actual}")]
    ProvenanceTaskIdentityMismatch { expected: String, actual: String },
    #[error("invalid recovery recommendation curation: {0}")]
    Invalid(String),
    #[error("recovery recommendation projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperienceRecoveryRecommendationRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperienceRecoveryRecommendationError>
    {
        let input_json = self
            .input
            .validate()
            .map_err(ControllerExperienceRecoveryRecommendationError::Input)?;
        self.observed
            .validate()
            .map_err(ControllerExperienceRecoveryRecommendationError::Observed)?;
        self.accepted
            .validate()
            .map_err(ControllerExperienceRecoveryRecommendationError::Accepted)?;

        let expected_task = self.input.current_request.observation.task_id.clone();
        if let Some(actual) = &self.provenance.task_id
            && actual != &expected_task
        {
            return Err(
                ControllerExperienceRecoveryRecommendationError::ProvenanceTaskIdentityMismatch {
                    expected: expected_task,
                    actual: actual.clone(),
                },
            );
        }

        let input = serde_json::from_str(&input_json)
            .map_err(ControllerExperienceRecoveryRecommendationError::Projection)?;
        let observed_output = serde_json::to_value(&self.observed)
            .map_err(ControllerExperienceRecoveryRecommendationError::Projection)?;
        let accepted_output = serde_json::to_value(&self.accepted)
            .map_err(ControllerExperienceRecoveryRecommendationError::Projection)?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperienceRecoveryRecommendationError::Invalid(
                    "equal observed and accepted outputs cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperienceRecoveryRecommendationError::Invalid(
                    "differing observed and accepted outputs require a corrected outcome".into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperienceRecoveryRecommendationError::Invalid(
                    "differing observed and accepted outputs require correction metadata".into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperienceRecoveryRecommendationError::Invalid(
                    "correction metadata must preserve the exact observed recovery recommendation"
                        .into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_RECOVERY_RECOMMENDATION_EXPERIENCE_CAPABILITY.into(),
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
