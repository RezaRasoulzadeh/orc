//! Typed, explicitly verified Controller experience examples.
//!
//! These records are a global curation dataset, not runtime memory. They are
//! created only by trusted application code and contain declarative JSON and
//! provenance metadata; no inference or automatic harvesting lives here.

use crate::memory::MemoryId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION: u32 = 1;
pub const MAX_CONTROLLER_EXPERIENCE_CAPABILITY_BYTES: usize = 128;
pub const MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES: usize = 16 * 1024;
pub const MAX_CONTROLLER_EXPERIENCE_REFERENCE_BYTES: usize = 256;
pub const MAX_CONTROLLER_EXPERIENCE_CORRECTION_REASON_BYTES: usize = 1024;
pub const MAX_CONTROLLER_EXPERIENCE_QUALITY_RATIONALE_BYTES: usize = 1024;
pub const MAX_CONTROLLER_EXPERIENCE_TIMESTAMP_BYTES: usize = 64;
pub const MAX_CONTROLLER_EXPERIENCE_EXAMPLE_BYTES: usize = 32 * 1024;
pub const MAX_CONTROLLER_EXPERIENCE_PAGE_SIZE: usize = 128;
pub const MAX_CONTROLLER_EXPERIENCE_PAGE_OFFSET: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerExperienceVerificationBasis {
    OperatorAttestation,
    ExplicitCorrection,
    ExternalEvaluation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerExperienceOutcome {
    Accepted,
    Corrected,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerExperienceExampleLifecycle {
    Active,
    Retired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerExperienceLifecycleFilter {
    Active,
    Retired,
    All,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerExperienceProvenance {
    pub project_id: Option<i64>,
    pub task_id: Option<String>,
    pub run_id: Option<i64>,
    pub plan_id: Option<i64>,
    pub review_id: Option<i64>,
    pub memory_id: Option<MemoryId>,
    pub source_reference: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerExperienceCorrectionMetadata {
    pub original_output: Value,
    pub operator_reference: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerExperienceQuality {
    pub score: u8,
    pub rationale: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerExperienceExampleDraft {
    pub schema_version: u32,
    pub capability: String,
    pub input: Value,
    pub accepted_output: Value,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerExperienceExample {
    pub id: i64,
    pub schema_version: u32,
    pub capability: String,
    pub input: Value,
    pub accepted_output: Value,
    pub verification_basis: ControllerExperienceVerificationBasis,
    pub provenance: ControllerExperienceProvenance,
    pub correction: Option<ControllerExperienceCorrectionMetadata>,
    pub outcome: ControllerExperienceOutcome,
    pub quality: ControllerExperienceQuality,
    pub created_at: String,
    pub lifecycle: ControllerExperienceExampleLifecycle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerExperienceExampleQuery {
    pub capability: Option<String>,
    pub lifecycle: ControllerExperienceLifecycleFilter,
    pub limit: usize,
    pub offset: usize,
}

impl ControllerExperienceExampleQuery {
    pub fn active(limit: usize, offset: usize) -> Self {
        Self {
            capability: None,
            lifecycle: ControllerExperienceLifecycleFilter::Active,
            limit,
            offset,
        }
    }

    pub fn validate(&self) -> Result<(), ControllerExperienceValidationError> {
        if let Some(capability) = &self.capability {
            validate_bounded_non_empty(
                capability,
                "capability filter",
                MAX_CONTROLLER_EXPERIENCE_CAPABILITY_BYTES,
            )?;
        }
        if self.limit == 0 || self.limit > MAX_CONTROLLER_EXPERIENCE_PAGE_SIZE {
            return Err(ControllerExperienceValidationError::InvalidQuery(
                "query limit must be between 1 and the page-size bound".into(),
            ));
        }
        if self.offset > MAX_CONTROLLER_EXPERIENCE_PAGE_OFFSET {
            return Err(ControllerExperienceValidationError::InvalidQuery(
                "query offset exceeds its bound".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ControllerExperienceValidationError {
    #[error("unsupported Controller experience schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("{0}")]
    Invalid(String),
    #[error("{field} exceeds {max} bytes (got {actual})")]
    TooLarge {
        field: &'static str,
        actual: usize,
        max: usize,
    },
    #[error("serialized Controller experience example exceeds {max} bytes (got {actual})")]
    ExampleTooLarge { actual: usize, max: usize },
    #[error("invalid Controller experience query: {0}")]
    InvalidQuery(String),
}

impl ControllerExperienceExampleDraft {
    pub fn validate(&self) -> Result<(), ControllerExperienceValidationError> {
        if self.schema_version != CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION {
            return Err(
                ControllerExperienceValidationError::UnsupportedSchemaVersion(self.schema_version),
            );
        }
        validate_bounded_non_empty(
            &self.capability,
            "capability",
            MAX_CONTROLLER_EXPERIENCE_CAPABILITY_BYTES,
        )?;
        validate_payload(&self.input, "input")?;
        validate_payload(&self.accepted_output, "accepted_output")?;
        validate_provenance(&self.provenance)?;
        validate_quality(&self.quality)?;
        if let Some(correction) = &self.correction {
            validate_correction(correction, &self.accepted_output)?;
        }
        if matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            != self.correction.is_some()
        {
            return Err(ControllerExperienceValidationError::Invalid(
                "corrected outcome requires correction metadata and other outcomes forbid it"
                    .into(),
            ));
        }
        let provisional = ControllerExperienceExample {
            id: i64::MAX,
            schema_version: self.schema_version,
            capability: self.capability.clone(),
            input: self.input.clone(),
            accepted_output: self.accepted_output.clone(),
            verification_basis: self.verification_basis,
            provenance: self.provenance.clone(),
            correction: self.correction.clone(),
            outcome: self.outcome,
            quality: self.quality.clone(),
            created_at: "9999-12-31 23:59:59".into(),
            lifecycle: ControllerExperienceExampleLifecycle::Active,
        };
        provisional.validate()
    }
}

impl ControllerExperienceExample {
    pub fn validate(&self) -> Result<(), ControllerExperienceValidationError> {
        if self.id <= 0 {
            return Err(ControllerExperienceValidationError::Invalid(
                "example id must be positive".into(),
            ));
        }
        if self.schema_version != CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION {
            return Err(
                ControllerExperienceValidationError::UnsupportedSchemaVersion(self.schema_version),
            );
        }
        validate_bounded_non_empty(
            &self.capability,
            "capability",
            MAX_CONTROLLER_EXPERIENCE_CAPABILITY_BYTES,
        )?;
        validate_payload(&self.input, "input")?;
        validate_payload(&self.accepted_output, "accepted_output")?;
        validate_provenance(&self.provenance)?;
        validate_quality(&self.quality)?;
        if self.created_at.trim().is_empty() {
            return Err(ControllerExperienceValidationError::Invalid(
                "created_at must not be empty".into(),
            ));
        }
        if self.created_at.len() > MAX_CONTROLLER_EXPERIENCE_TIMESTAMP_BYTES {
            return Err(ControllerExperienceValidationError::TooLarge {
                field: "created_at",
                actual: self.created_at.len(),
                max: MAX_CONTROLLER_EXPERIENCE_TIMESTAMP_BYTES,
            });
        }
        if let Some(correction) = &self.correction {
            validate_correction(correction, &self.accepted_output)?;
        }
        if matches!(self.outcome, ControllerExperienceOutcome::Corrected)
            != self.correction.is_some()
        {
            return Err(ControllerExperienceValidationError::Invalid(
                "corrected outcome requires correction metadata and other outcomes forbid it"
                    .into(),
            ));
        }
        let actual = serde_json::to_vec(self).map_err(|error| {
            ControllerExperienceValidationError::Invalid(format!(
                "example serialization failed: {error}"
            ))
        })?;
        if actual.len() > MAX_CONTROLLER_EXPERIENCE_EXAMPLE_BYTES {
            return Err(ControllerExperienceValidationError::ExampleTooLarge {
                actual: actual.len(),
                max: MAX_CONTROLLER_EXPERIENCE_EXAMPLE_BYTES,
            });
        }
        Ok(())
    }
}

fn validate_payload(
    value: &Value,
    field: &'static str,
) -> Result<(), ControllerExperienceValidationError> {
    if value.is_null() {
        return Err(ControllerExperienceValidationError::Invalid(format!(
            "{field} payload must not be null"
        )));
    }
    let actual = serde_json::to_vec(value)
        .map_err(|error| {
            ControllerExperienceValidationError::Invalid(format!(
                "{field} serialization failed: {error}"
            ))
        })?
        .len();
    if actual == 0 {
        return Err(ControllerExperienceValidationError::Invalid(format!(
            "{field} payload must not be empty"
        )));
    }
    if actual > MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES {
        return Err(ControllerExperienceValidationError::TooLarge {
            field,
            actual,
            max: MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES,
        });
    }
    Ok(())
}

fn validate_provenance(
    provenance: &ControllerExperienceProvenance,
) -> Result<(), ControllerExperienceValidationError> {
    for (name, value) in [
        ("run_id", provenance.run_id),
        ("plan_id", provenance.plan_id),
        ("review_id", provenance.review_id),
    ] {
        if value.is_some_and(|value| value <= 0) {
            return Err(ControllerExperienceValidationError::Invalid(format!(
                "{name} must be positive when present"
            )));
        }
    }
    if provenance.project_id.is_some_and(|value| value <= 0) {
        return Err(ControllerExperienceValidationError::Invalid(
            "project provenance must be positive when present".into(),
        ));
    }
    if let Some(memory_id) = &provenance.memory_id
        && (memory_id.value() <= 0
            || matches!(memory_id, MemoryId::Project { project_id, .. } if *project_id <= 0))
    {
        return Err(ControllerExperienceValidationError::Invalid(
            "memory provenance identity must use positive IDs".into(),
        ));
    }
    for (field, value) in [
        ("task_id", provenance.task_id.as_deref()),
        ("source_reference", provenance.source_reference.as_deref()),
    ] {
        if let Some(value) = value {
            validate_bounded_non_empty(value, field, MAX_CONTROLLER_EXPERIENCE_REFERENCE_BYTES)?;
        }
    }
    Ok(())
}

fn validate_correction(
    correction: &ControllerExperienceCorrectionMetadata,
    accepted_output: &Value,
) -> Result<(), ControllerExperienceValidationError> {
    validate_payload(&correction.original_output, "correction.original_output")?;
    if correction.original_output == *accepted_output {
        return Err(ControllerExperienceValidationError::Invalid(
            "correction metadata requires an accepted output different from the original".into(),
        ));
    }
    validate_bounded_non_empty(
        &correction.operator_reference,
        "correction.operator_reference",
        MAX_CONTROLLER_EXPERIENCE_REFERENCE_BYTES,
    )?;
    validate_bounded_non_empty(
        &correction.reason,
        "correction.reason",
        MAX_CONTROLLER_EXPERIENCE_CORRECTION_REASON_BYTES,
    )?;
    Ok(())
}

fn validate_quality(
    quality: &ControllerExperienceQuality,
) -> Result<(), ControllerExperienceValidationError> {
    if quality.score > 100 {
        return Err(ControllerExperienceValidationError::Invalid(
            "quality score must be between 0 and 100".into(),
        ));
    }
    validate_bounded_non_empty(
        &quality.rationale,
        "quality.rationale",
        MAX_CONTROLLER_EXPERIENCE_QUALITY_RATIONALE_BYTES,
    )
}

fn validate_bounded_non_empty(
    value: &str,
    field: &'static str,
    max: usize,
) -> Result<(), ControllerExperienceValidationError> {
    if value.trim().is_empty() {
        return Err(ControllerExperienceValidationError::Invalid(format!(
            "{field} must not be empty"
        )));
    }
    if value.len() > max {
        return Err(ControllerExperienceValidationError::TooLarge {
            field,
            actual: value.len(),
            max,
        });
    }
    Ok(())
}
