//! Explicit curation for the Controller Plan-revision generation capability.
//!
//! This module adapts one already-produced Plan-revision interaction into the
//! canonical M08-001 dataset record. The generated PlanResponse is the
//! reasoning output authority; trusted parent/review lineage is deliberately
//! outside this request and output projection.

use crate::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleDraft,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use crate::controller_plan_revision::{ControllerPlanRevisionError, ControllerPlanRevisionInput};
use crate::protocol::PlanResponse;
use thiserror::Error;

/// Fixed code-owned M08 capability identity for Plan-revision generation.
pub const CONTROLLER_PLAN_REVISION_EXPERIENCE_CAPABILITY: &str = "controller.plan_revision";

/// One explicit, already-produced Controller Plan-revision curation request.
/// The capability and trusted lineage fields are intentionally absent and
/// cannot be overridden or supplied by callers.
#[derive(Clone, Debug)]
pub struct ControllerExperiencePlanRevisionRequest {
    pub input: ControllerPlanRevisionInput,
    pub observed: PlanResponse,
    pub accepted: PlanResponse,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Debug, Error)]
pub enum ControllerExperiencePlanRevisionError {
    #[error("Controller Plan revision input validation failed: {0}")]
    Input(#[source] ControllerPlanRevisionError),
    #[error("observed PlanResponse validation failed: {0}")]
    Observed(String),
    #[error("accepted PlanResponse validation failed: {0}")]
    Accepted(String),
    #[error("invalid Controller Plan-revision curation: {0}")]
    Invalid(String),
    #[error("Controller Plan-revision projection failed: {0}")]
    Projection(#[source] serde_json::Error),
}

impl ControllerExperiencePlanRevisionRequest {
    /// Validate and project this request into exactly one canonical M08-001
    /// draft. Persistence is deliberately left to the existing M08 API.
    pub fn into_example_draft(
        &self,
    ) -> Result<ControllerExperienceExampleDraft, ControllerExperiencePlanRevisionError> {
        self.input
            .validate()
            .map_err(ControllerExperiencePlanRevisionError::Input)?;
        self.observed
            .validate()
            .map_err(|error| ControllerExperiencePlanRevisionError::Observed(error.to_string()))?;
        self.accepted
            .validate()
            .map_err(|error| ControllerExperiencePlanRevisionError::Accepted(error.to_string()))?;

        let input = serde_json::to_value(&self.input)
            .map_err(ControllerExperiencePlanRevisionError::Projection)?;
        let observed_output = serde_json::to_value(&self.observed)
            .map_err(ControllerExperiencePlanRevisionError::Projection)?;
        let accepted_output = serde_json::to_value(&self.accepted)
            .map_err(ControllerExperiencePlanRevisionError::Projection)?;

        let correction = if observed_output == accepted_output {
            if self.correction.is_some()
                || matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            {
                return Err(ControllerExperiencePlanRevisionError::Invalid(
                    "equal observed and accepted PlanResponse outputs cannot carry correction metadata or a corrected outcome".into(),
                ));
            }
            None
        } else {
            if !matches!(self.outcome, ControllerExperienceOutcome::Corrected) {
                return Err(ControllerExperiencePlanRevisionError::Invalid(
                    "differing observed and accepted PlanResponse outputs require a corrected outcome".into(),
                ));
            }
            let metadata = self.correction.clone().ok_or_else(|| {
                ControllerExperiencePlanRevisionError::Invalid(
                    "differing observed and accepted PlanResponse outputs require correction metadata".into(),
                )
            })?;
            if metadata.original_output != observed_output {
                return Err(ControllerExperiencePlanRevisionError::Invalid(
                    "correction metadata must preserve the exact observed PlanResponse".into(),
                ));
            }
            Some(metadata)
        };

        Ok(ControllerExperienceExampleDraft {
            schema_version:
                crate::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
            capability: CONTROLLER_PLAN_REVISION_EXPERIENCE_CAPABILITY.into(),
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
