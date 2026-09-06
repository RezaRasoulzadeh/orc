//! Explicit curation for the Controller Plan-review capability.
//!
//! This module adapts one already-produced, typed Plan-review interaction
//! into the canonical M08-001 dataset record. It performs no inference,
//! harvesting, Plan/workflow observation, or automatic verification.

use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use crate::controller_plan_review::{
    ControllerPlanReviewError, ControllerPlanReviewInput, ControllerPlanReviewResult,
};
use thiserror::Error;

/// Fixed code-owned M08 capability identity for Controller Plan review.
pub const CONTROLLER_PLAN_REVIEW_EXPERIENCE_CAPABILITY: &str = "controller.plan_review";

/// One explicit, already-produced Controller Plan-review curation request.
/// The capability identity is intentionally absent and cannot be overridden
/// by callers.
#[derive(Clone, Debug, PartialEq)]
pub struct ControllerExperiencePlanReviewRequest {
    pub input: ControllerPlanReviewInput,
    pub observed: ControllerPlanReviewResult,
    pub accepted: ControllerPlanReviewResult,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperiencePlanReviewError {
    #[error("Controller Plan review input validation failed: {0}")]
    Input(#[source] ControllerPlanReviewError),
    #[error("observed Controller Plan review result validation failed: {0}")]
    Observed(#[source] ControllerPlanReviewError),
    #[error("accepted Controller Plan review result validation failed: {0}")]
    Accepted(#[source] ControllerPlanReviewError),
    #[error("invalid Controller Plan review curation: {0}")]
    Invalid(String),
    #[error("Controller Plan review projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperiencePlanReviewRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperiencePlanReviewError> {
        self.input
            .validate()
            .map_err(ControllerExperiencePlanReviewError::Input)?;
        self.observed
            .validate()
            .map_err(ControllerExperiencePlanReviewError::Observed)?;
        self.accepted
            .validate()
            .map_err(ControllerExperiencePlanReviewError::Accepted)?;

        let input = serde_json::to_value(&self.input)
            .map_err(ControllerExperiencePlanReviewError::Projection)?;
        let observed_output = serde_json::to_value(&self.observed)
            .map_err(ControllerExperiencePlanReviewError::Projection)?;
        let accepted_output = serde_json::to_value(&self.accepted)
            .map_err(ControllerExperiencePlanReviewError::Projection)?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperiencePlanReviewError::Invalid(
                    "equal observed and accepted outputs cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperiencePlanReviewError::Invalid(
                    "differing observed and accepted outputs require a corrected outcome".into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperiencePlanReviewError::Invalid(
                    "differing observed and accepted outputs require correction metadata".into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperiencePlanReviewError::Invalid(
                    "correction metadata must preserve the exact observed Controller Plan review result".into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_PLAN_REVIEW_EXPERIENCE_CAPABILITY.into(),
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
