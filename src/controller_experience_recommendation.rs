//! Explicit curation for the normal Controller task-recommendation capability.
//!
//! This module adapts one already-produced, validated recommendation
//! interaction into the canonical M08-001 dataset record. It performs no
//! inference, harvesting, workflow observation, or automatic verification.

use crate::controller::{ControllerError, ControllerRecommendation, ControllerRecommendationInput};
use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use thiserror::Error;

/// Fixed code-owned M08 capability identity for normal task recommendation.
pub const CONTROLLER_RECOMMENDATION_EXPERIENCE_CAPABILITY: &str = "controller.task_recommendation";

/// One explicit, already-produced normal Controller recommendation curation
/// request. The capability is intentionally absent and cannot be overridden by
/// callers.
#[derive(Clone, Debug, PartialEq)]
pub struct ControllerExperienceRecommendationRequest {
    pub input: ControllerRecommendationInput,
    pub observed: ControllerRecommendation,
    pub accepted: ControllerRecommendation,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperienceRecommendationError {
    #[error("recommendation input validation failed: {0}")]
    Input(#[source] ControllerError),
    #[error("observed recommendation validation failed: {0}")]
    Observed(#[source] ControllerError),
    #[error("accepted recommendation validation failed: {0}")]
    Accepted(#[source] ControllerError),
    #[error(
        "recommendation task identity mismatch: expected {expected}, observed {observed}, accepted {accepted}"
    )]
    TaskIdentityMismatch {
        expected: String,
        observed: String,
        accepted: String,
    },
    #[error("invalid recommendation curation: {0}")]
    Invalid(String),
    #[error("recommendation input projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperienceRecommendationRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperienceRecommendationError> {
        self.input
            .validate()
            .map_err(ControllerExperienceRecommendationError::Input)?;
        self.observed
            .validate()
            .map_err(ControllerExperienceRecommendationError::Observed)?;
        self.accepted
            .validate()
            .map_err(ControllerExperienceRecommendationError::Accepted)?;

        let expected_task = self.input.current_packet.task.summary.task_id.clone();
        if self.observed.task_id != expected_task || self.accepted.task_id != expected_task {
            return Err(
                ControllerExperienceRecommendationError::TaskIdentityMismatch {
                    expected: expected_task,
                    observed: self.observed.task_id.clone(),
                    accepted: self.accepted.task_id.clone(),
                },
            );
        }

        let input = serde_json::to_value(&self.input)
            .map_err(ControllerExperienceRecommendationError::Projection)?;
        let observed_output = self.observed.structured_output.clone().ok_or_else(|| {
            ControllerExperienceRecommendationError::Invalid(
                "observed recommendation has no canonical structured output".into(),
            )
        })?;
        let accepted_output = self.accepted.structured_output.clone().ok_or_else(|| {
            ControllerExperienceRecommendationError::Invalid(
                "accepted recommendation has no canonical structured output".into(),
            )
        })?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperienceRecommendationError::Invalid(
                    "equal observed and accepted outputs cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperienceRecommendationError::Invalid(
                    "differing observed and accepted outputs require a corrected outcome".into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperienceRecommendationError::Invalid(
                    "differing observed and accepted outputs require correction metadata".into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperienceRecommendationError::Invalid(
                    "correction metadata must preserve the exact observed structured output".into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_RECOMMENDATION_EXPERIENCE_CAPABILITY.into(),
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
