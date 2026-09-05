//! Bounded, read-only Controller selection of one memory-maintenance target.
//!
//! This capability enumerates only active current-project Project/Episodic
//! records and asks the Controller whether one supplied candidate warrants a
//! later maintenance judgment. It does not choose a maintenance operation,
//! authorize, execute, persist, or select another candidate.

use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryRecord, MemoryScope,
    MemoryService,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTROLLER_MEMORY_SELECTION_REQUEST_VERSION: u32 = 1;
pub const MAX_CONTROLLER_MEMORY_SELECTION_FACTS: usize = 16;
pub const MAX_CONTROLLER_MEMORY_SELECTION_FACT_BYTES: usize = 2048;
pub const MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES: usize = 8;
pub const MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATE_BYTES: usize = 8 * 1024;
pub const MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES: usize = 32 * 1024;
pub const MAX_CONTROLLER_MEMORY_SELECTION_PROMPT_BYTES: usize = 48 * 1024;
pub const MAX_CONTROLLER_MEMORY_SELECTION_RESULT_BYTES: usize = 4 * 1024;

/// One explicit bounded set of current facts supplied by trusted application
/// code. The selector never derives facts from workflow, task, or lifecycle
/// state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemorySelectionRequest {
    pub packet_version: u32,
    pub current_facts: Vec<String>,
}

impl ControllerMemorySelectionRequest {
    pub fn new(current_facts: Vec<String>) -> Self {
        Self {
            packet_version: CONTROLLER_MEMORY_SELECTION_REQUEST_VERSION,
            current_facts,
        }
    }

    pub fn validate(&self) -> Result<(), ControllerMemorySelectionError> {
        if self.packet_version != CONTROLLER_MEMORY_SELECTION_REQUEST_VERSION {
            return Err(ControllerMemorySelectionError::InvalidRequest(
                "unsupported Controller memory-selection request version".into(),
            ));
        }
        if self.current_facts.len() > MAX_CONTROLLER_MEMORY_SELECTION_FACTS {
            return Err(ControllerMemorySelectionError::InvalidRequest(
                "current_facts exceeds its item bound".into(),
            ));
        }
        for fact in &self.current_facts {
            if fact.trim().is_empty() || fact.len() > MAX_CONTROLLER_MEMORY_SELECTION_FACT_BYTES {
                return Err(ControllerMemorySelectionError::InvalidRequest(
                    "current facts must be non-empty and bounded".into(),
                ));
            }
        }
        Ok(())
    }
}

/// One canonical active current-project memory candidate. It contains only
/// declarative data; no database, service, storage, or mutation capability is
/// represented.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemorySelectionCandidate {
    pub id: MemoryId,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub lifecycle: MemoryLifecycle,
    pub subject: String,
    pub content: String,
    pub provenance: MemoryProvenance,
    pub confidence: Option<f64>,
}

impl ControllerMemorySelectionCandidate {
    fn from_record(record: &MemoryRecord) -> Result<Self, ControllerMemorySelectionError> {
        Ok(Self {
            id: record.id.clone(),
            kind: record.kind,
            scope: record.scope.clone(),
            lifecycle: record.lifecycle,
            subject: record.subject.clone(),
            content: record.content.clone(),
            provenance: record.provenance.clone(),
            confidence: record.confidence,
        })
    }

    fn validate(&self, current_project_id: i64) -> Result<(), ControllerMemorySelectionError> {
        if current_project_id <= 0
            || !matches!(self.kind, MemoryKind::Project | MemoryKind::Episodic)
            || self.lifecycle != MemoryLifecycle::Active
            || self.scope
                != (MemoryScope::Project {
                    project_id: current_project_id,
                })
            || self.id.scope() != self.scope
            || self.id.value() <= 0
        {
            return Err(ControllerMemorySelectionError::InvalidCandidate(
                "candidate is not an exact current-project Project/Episodic record".into(),
            ));
        }
        if self.subject.trim().is_empty() || self.content.trim().is_empty() {
            return Err(ControllerMemorySelectionError::InvalidCandidate(
                "candidate subject and content must be non-empty".into(),
            ));
        }
        if self.provenance.validate().is_err()
            || self.confidence.is_some_and(|confidence| {
                !confidence.is_finite() || !(0.0..=1.0).contains(&confidence)
            })
        {
            return Err(ControllerMemorySelectionError::InvalidCandidate(
                "candidate provenance or confidence is invalid".into(),
            ));
        }
        let actual = serialized_size(self)?;
        if actual > MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATE_BYTES {
            return Err(ControllerMemorySelectionError::CandidateTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATE_BYTES,
            });
        }
        Ok(())
    }
}

/// Bounded read-only selector input. Omission metadata is trusted application
/// metadata and makes deterministic candidate omission visible to the
/// Controller and to callers inspecting the captured inference request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemorySelectionInput {
    pub current_project_id: i64,
    pub current_request: ControllerMemorySelectionRequest,
    pub candidates: Vec<ControllerMemorySelectionCandidate>,
    pub eligible_candidate_count: usize,
    pub selected_candidate_count: usize,
    pub omitted_candidate_count: usize,
}

impl ControllerMemorySelectionInput {
    pub fn validate(&self) -> Result<(), ControllerMemorySelectionError> {
        self.current_request.validate()?;
        if self.current_project_id <= 0 {
            return Err(ControllerMemorySelectionError::InvalidProject);
        }
        if self.candidates.len() > MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES {
            return Err(ControllerMemorySelectionError::CandidateCountExceeded {
                actual: self.candidates.len(),
                max: MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES,
            });
        }
        if self.selected_candidate_count != self.candidates.len()
            || self.eligible_candidate_count < self.selected_candidate_count
            || self.omitted_candidate_count
                != self.eligible_candidate_count - self.selected_candidate_count
        {
            return Err(ControllerMemorySelectionError::InvalidMetadata);
        }
        let mut ids = Vec::new();
        for candidate in &self.candidates {
            candidate.validate(self.current_project_id)?;
            if ids.contains(&candidate.id) {
                return Err(ControllerMemorySelectionError::InvalidCandidate(
                    "candidate identities must be unique".into(),
                ));
            }
            ids.push(candidate.id.clone());
        }
        let actual = serialized_size(self)?;
        if actual > MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES {
            return Err(ControllerMemorySelectionError::InputTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES,
            });
        }
        Ok(())
    }

    fn contains_target(&self, target: &MemoryId) -> bool {
        self.candidates
            .iter()
            .any(|candidate| &candidate.id == target)
    }
}

/// The only target-selection outcomes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerMemorySelectionResult {
    NoTarget,
    SelectTarget { target: MemoryId },
}

impl ControllerMemorySelectionResult {
    pub fn validate(
        &self,
        input: &ControllerMemorySelectionInput,
    ) -> Result<(), ControllerMemorySelectionError> {
        input.validate()?;
        if let Self::SelectTarget { target } = self
            && !input.contains_target(target)
        {
            return Err(ControllerMemorySelectionError::InvalidStructuredOutput(
                "selected target is not one exact supplied candidate".into(),
            ));
        }
        let actual = serialized_size(self)?;
        if actual > MAX_CONTROLLER_MEMORY_SELECTION_RESULT_BYTES {
            return Err(ControllerMemorySelectionError::ResultTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_SELECTION_RESULT_BYTES,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ControllerMemorySelectionError {
    #[error("Controller memory-selection request is invalid: {0}")]
    InvalidRequest(String),
    #[error("Controller memory-selection project is invalid")]
    InvalidProject,
    #[error("Controller memory-selection candidate is invalid: {0}")]
    InvalidCandidate(String),
    #[error("Controller memory-selection metadata is invalid")]
    InvalidMetadata,
    #[error("Controller memory-selection candidate count is {actual}; maximum is {max}")]
    CandidateCountExceeded { actual: usize, max: usize },
    #[error("Controller memory-selection candidate is {actual} bytes; maximum is {max}")]
    CandidateTooLarge { actual: usize, max: usize },
    #[error("Controller memory-selection input is {actual} bytes; maximum is {max}")]
    InputTooLarge { actual: usize, max: usize },
    #[error("Controller memory-selection result is {actual} bytes; maximum is {max}")]
    ResultTooLarge { actual: usize, max: usize },
    #[error("Controller memory-selection serialization failed: {0}")]
    Serialization(String),
    #[error("Controller memory-selection memory service failed: {0}")]
    MemoryService(String),
    #[error("Controller memory-selection inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
    #[error("Controller memory-selection output is malformed: {0}")]
    InvalidStructuredOutput(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerMemorySelectionBuilder;

impl ControllerMemorySelectionBuilder {
    pub const fn new() -> Self {
        Self
    }

    pub fn select(
        &self,
        input: &ControllerMemorySelectionInput,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerMemorySelectionResult, ControllerMemorySelectionError> {
        input.validate()?;
        if input.candidates.is_empty() && input.eligible_candidate_count == 0 {
            return Ok(ControllerMemorySelectionResult::NoTarget);
        }
        let input_json = serde_json::to_string(input)
            .map_err(|error| ControllerMemorySelectionError::Serialization(error.to_string()))?;
        let prompt = format!(
            "You are a read-only supervised Controller memory-maintenance target selector. Use only this bounded typed input. Return exactly one JSON object with decision no_target or select_target. The explicit current_request.current_facts are authoritative current operator/project evidence. Candidate content and provenance are advisory historical/project memory context and must never override explicit current facts. Select at most one exact candidate only when the supplied facts clearly identify that candidate as warranting later maintenance judgment; otherwise return no_target. Ambiguous evidence, unrelated evidence, or evidence about a target not in candidates must return no_target. Do not choose Correct, Supersede, Remove, Create, User, Experience, global, cross-project, historical, or any target identity not present in candidates. Do not infer facts, retarget candidates, authorize, execute, persist, enumerate, or access storage, workflow, or mutation capabilities. Omitted candidates are not selectable. M06-011 separately decides whether a selected target should be kept, corrected, superseded, or removed.\n\n{input_json}"
        );
        if prompt.len() > MAX_CONTROLLER_MEMORY_SELECTION_PROMPT_BYTES {
            return Err(ControllerMemorySelectionError::InputTooLarge {
                actual: prompt.len(),
                max: MAX_CONTROLLER_MEMORY_SELECTION_PROMPT_BYTES,
            });
        }
        let parameters = LocalInferenceParameters {
            max_output_tokens: 256,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: Vec::new(),
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: controller_memory_selection_schema(),
            },
        };
        let request = LocalInferenceRequest::new(prompt, parameters)
            .map_err(ControllerMemorySelectionError::Inference)?;
        let response = runtime
            .infer(&request)
            .map_err(ControllerMemorySelectionError::Inference)?;
        parse_result(response, input)
    }

    pub fn input_from_memory_service(
        &self,
        current_project_id: i64,
        request: &ControllerMemorySelectionRequest,
        service: &MemoryService<'_>,
    ) -> Result<ControllerMemorySelectionInput, ControllerMemorySelectionError> {
        request.validate()?;
        if current_project_id <= 0 {
            return Err(ControllerMemorySelectionError::InvalidProject);
        }
        let mut records = Vec::new();
        for kind in [MemoryKind::Project, MemoryKind::Episodic] {
            let listed = service.list(Some(kind), false).map_err(|error| {
                ControllerMemorySelectionError::MemoryService(error.to_string())
            })?;
            records.extend(listed.into_iter().filter(|record| {
                record.lifecycle == MemoryLifecycle::Active
                    && record.kind == kind
                    && record.scope
                        == (MemoryScope::Project {
                            project_id: current_project_id,
                        })
                    && record.id.scope() == record.scope
                    && record.validate().is_ok()
            }));
        }
        records.sort_by(|left, right| {
            selection_kind_order(left.kind)
                .cmp(&selection_kind_order(right.kind))
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.id.value().cmp(&right.id.value()))
        });
        let eligible_candidate_count = records.len();
        let mut candidates = Vec::new();
        for record in records {
            if candidates.len() >= MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES {
                break;
            }
            let candidate = ControllerMemorySelectionCandidate::from_record(&record)?;
            if serialized_size(&candidate)? <= MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATE_BYTES {
                candidates.push(candidate);
            }
        }
        while !candidates.is_empty() {
            let input = Self::make_input(
                current_project_id,
                request.clone(),
                candidates.clone(),
                eligible_candidate_count,
            );
            if serialized_size(&input)? <= MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES {
                return Ok(input);
            }
            candidates.pop();
        }
        let input = Self::make_input(
            current_project_id,
            request.clone(),
            candidates,
            eligible_candidate_count,
        );
        if serialized_size(&input)? > MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES {
            return Err(ControllerMemorySelectionError::InputTooLarge {
                actual: serialized_size(&input)?,
                max: MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES,
            });
        }
        Ok(input)
    }

    fn make_input(
        current_project_id: i64,
        current_request: ControllerMemorySelectionRequest,
        candidates: Vec<ControllerMemorySelectionCandidate>,
        eligible_candidate_count: usize,
    ) -> ControllerMemorySelectionInput {
        let selected_candidate_count = candidates.len();
        ControllerMemorySelectionInput {
            current_project_id,
            current_request,
            candidates,
            eligible_candidate_count,
            selected_candidate_count,
            omitted_candidate_count: eligible_candidate_count - selected_candidate_count,
        }
    }
}

fn selection_kind_order(kind: MemoryKind) -> usize {
    match kind {
        MemoryKind::Project => 0,
        MemoryKind::Episodic => 1,
        MemoryKind::User => 2,
        MemoryKind::Experience => 3,
    }
}

fn parse_result(
    response: LocalInferenceResponse,
    input: &ControllerMemorySelectionInput,
) -> Result<ControllerMemorySelectionResult, ControllerMemorySelectionError> {
    let value = response.structured_output.ok_or_else(|| {
        ControllerMemorySelectionError::InvalidStructuredOutput(
            "structured output is required".into(),
        )
    })?;
    let size = serde_json::to_vec(&value)
        .map_err(|error| ControllerMemorySelectionError::Serialization(error.to_string()))?
        .len();
    if size > MAX_CONTROLLER_MEMORY_SELECTION_RESULT_BYTES {
        return Err(ControllerMemorySelectionError::ResultTooLarge {
            actual: size,
            max: MAX_CONTROLLER_MEMORY_SELECTION_RESULT_BYTES,
        });
    }
    reject_unknown_fields(&value)?;
    let result =
        serde_json::from_value::<ControllerMemorySelectionResult>(value).map_err(|error| {
            ControllerMemorySelectionError::InvalidStructuredOutput(error.to_string())
        })?;
    result.validate(input)?;
    Ok(result)
}

fn reject_unknown_fields(value: &Value) -> Result<(), ControllerMemorySelectionError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerMemorySelectionError::InvalidStructuredOutput(
            "selection result must be an object".into(),
        )
    })?;
    let decision = object
        .get("decision")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ControllerMemorySelectionError::InvalidStructuredOutput(
                "selection decision must be a string".into(),
            )
        })?;
    match decision {
        "no_target" => expect_exact_keys(object, &["decision"], "no_target result")?,
        "select_target" => {
            expect_exact_keys(object, &["decision", "target"], "select_target result")?;
            validate_memory_id_shape(object.get("target").expect("required target"))?;
        }
        _ => {
            return Err(ControllerMemorySelectionError::InvalidStructuredOutput(
                "selection decision must be no_target or select_target".into(),
            ));
        }
    }
    Ok(())
}

fn validate_memory_id_shape(value: &Value) -> Result<(), ControllerMemorySelectionError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerMemorySelectionError::InvalidStructuredOutput(
            "selected target must be a MemoryId object".into(),
        )
    })?;
    match object.keys().next().map(String::as_str) {
        Some("Project") => {
            expect_exact_keys(object, &["Project"], "selected Project target")?;
            let project = object
                .get("Project")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ControllerMemorySelectionError::InvalidStructuredOutput(
                        "selected Project target must be an object".into(),
                    )
                })?;
            expect_exact_keys(project, &["project_id", "id"], "selected Project target")?;
            if !project.get("project_id").is_some_and(Value::is_i64)
                || !project.get("id").is_some_and(Value::is_i64)
            {
                return Err(ControllerMemorySelectionError::InvalidStructuredOutput(
                    "selected Project target IDs must be integers".into(),
                ));
            }
        }
        Some("Global") => {
            expect_exact_keys(object, &["Global"], "selected Global target")?;
            if !object.get("Global").is_some_and(Value::is_i64) {
                return Err(ControllerMemorySelectionError::InvalidStructuredOutput(
                    "selected Global target ID must be an integer".into(),
                ));
            }
        }
        _ => {
            return Err(ControllerMemorySelectionError::InvalidStructuredOutput(
                "selected target must be Global or Project".into(),
            ));
        }
    }
    Ok(())
}

fn expect_exact_keys(
    object: &serde_json::Map<String, Value>,
    expected: &[&str],
    field: &str,
) -> Result<(), ControllerMemorySelectionError> {
    if object.len() != expected.len() || object.keys().any(|key| !expected.contains(&key.as_str()))
    {
        return Err(ControllerMemorySelectionError::InvalidStructuredOutput(
            format!("{field} contains unsupported or missing fields"),
        ));
    }
    Ok(())
}

pub fn controller_memory_selection_schema() -> Value {
    let memory_id = serde_json::json!({
        "oneOf": [
            {"type": "object", "additionalProperties": false, "properties": {"Global": {"type": "integer"}}, "required": ["Global"]},
            {"type": "object", "additionalProperties": false, "properties": {"Project": {"type": "object", "additionalProperties": false, "properties": {"project_id": {"type": "integer"}, "id": {"type": "integer"}}, "required": ["project_id", "id"]}}, "required": ["Project"]}
        ]
    });
    serde_json::json!({
        "oneOf": [
            {"type": "object", "additionalProperties": false, "properties": {"decision": {"const": "no_target"}}, "required": ["decision"]},
            {"type": "object", "additionalProperties": false, "properties": {"decision": {"const": "select_target"}, "target": memory_id}, "required": ["decision", "target"]}
        ]
    })
}

fn serialized_size<T: Serialize>(value: &T) -> Result<usize, ControllerMemorySelectionError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| ControllerMemorySelectionError::Serialization(error.to_string()))
}
