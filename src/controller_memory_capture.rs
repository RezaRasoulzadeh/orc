//! Read-only Controller judgment for one explicitly supplied memory candidate.
//!
//! Capture judgment may ignore a candidate or return one exact canonical
//! M06-009 create intent. It has no persistence, authorization, execution, or
//! retrieval capability; application code must separately pass any proposed
//! intent through the supervised mutation boundary.

use crate::controller_memory::ControllerMemoryContext;
use crate::controller_memory_mutation::ControllerMemoryMutationIntent;
use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::memory::MemoryDraft;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTROLLER_MEMORY_CAPTURE_REQUEST_VERSION: u32 = 1;
pub const MAX_CONTROLLER_MEMORY_CAPTURE_CANDIDATE_BYTES: usize = 40 * 1024;
pub const MAX_CONTROLLER_MEMORY_CAPTURE_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_MEMORY_CAPTURE_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_MEMORY_CAPTURE_RESULT_BYTES: usize = 32 * 1024;
const MAX_SOURCE_FACTS: usize = 16;
const MAX_SOURCE_FACT_BYTES: usize = 2048;

#[derive(Debug, Error)]
pub enum ControllerMemoryCaptureError {
    #[error("Controller memory capture request is invalid: {0}")]
    InvalidRequest(String),
    #[error("Controller memory capture serialization failed: {0}")]
    Serialization(String),
    #[error("Controller memory capture memory context failed: {0}")]
    MemoryContext(String),
    #[error("Controller memory capture input is {actual} bytes; maximum is {max}")]
    InputTooLarge { actual: usize, max: usize },
    #[error("Controller memory capture request is {actual} bytes; maximum is {max}")]
    RequestTooLarge { actual: usize, max: usize },
    #[error("Controller memory capture output is malformed: {0}")]
    InvalidStructuredOutput(String),
    #[error("Controller memory capture inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
}

/// One explicit bounded candidate supplied by trusted Orc/application code.
/// The source facts are declarative context, not a persistence or execution
/// handle. The draft carries the canonical typed kind, scope, and provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemoryCaptureCandidate {
    pub draft: MemoryDraft,
    pub source_facts: Vec<String>,
}

impl ControllerMemoryCaptureCandidate {
    pub fn validate(&self) -> Result<(), ControllerMemoryCaptureError> {
        self.draft
            .validate()
            .map_err(|error| ControllerMemoryCaptureError::InvalidRequest(error.to_string()))?;
        if self.source_facts.len() > MAX_SOURCE_FACTS {
            return Err(ControllerMemoryCaptureError::InvalidRequest(
                "source_facts exceeds its item bound".into(),
            ));
        }
        for fact in &self.source_facts {
            if fact.trim().is_empty() || fact.len() > MAX_SOURCE_FACT_BYTES {
                return Err(ControllerMemoryCaptureError::InvalidRequest(
                    "source facts must be non-empty and bounded".into(),
                ));
            }
        }
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerMemoryCaptureError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_MEMORY_CAPTURE_CANDIDATE_BYTES {
            return Err(ControllerMemoryCaptureError::RequestTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_CAPTURE_CANDIDATE_BYTES,
            });
        }
        Ok(())
    }
}

/// Capability-specific capture request. It contains one candidate only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemoryCaptureRequest {
    pub packet_version: u32,
    pub candidate: ControllerMemoryCaptureCandidate,
}

impl ControllerMemoryCaptureRequest {
    pub fn from_candidate(candidate: ControllerMemoryCaptureCandidate) -> Self {
        Self {
            packet_version: CONTROLLER_MEMORY_CAPTURE_REQUEST_VERSION,
            candidate,
        }
    }

    pub fn validate(&self) -> Result<(), ControllerMemoryCaptureError> {
        if self.packet_version != CONTROLLER_MEMORY_CAPTURE_REQUEST_VERSION {
            return Err(ControllerMemoryCaptureError::InvalidRequest(
                "unsupported Controller memory capture request version".into(),
            ));
        }
        self.candidate.validate()?;
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerMemoryCaptureError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_MEMORY_CAPTURE_REQUEST_BYTES {
            return Err(ControllerMemoryCaptureError::RequestTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_CAPTURE_REQUEST_BYTES,
            });
        }
        Ok(())
    }
}

/// Capability-local input combining the explicit candidate with the canonical
/// bounded read-only memory projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemoryCaptureInput {
    pub current_request: ControllerMemoryCaptureRequest,
    pub memory: ControllerMemoryContext,
}

impl ControllerMemoryCaptureInput {
    pub fn from_request(
        request: &ControllerMemoryCaptureRequest,
        memory: ControllerMemoryContext,
    ) -> Self {
        Self {
            current_request: request.clone(),
            memory,
        }
    }

    pub fn validate(&self) -> Result<(), ControllerMemoryCaptureError> {
        self.current_request.validate()?;
        self.memory
            .validate()
            .map_err(|error| ControllerMemoryCaptureError::MemoryContext(error.to_string()))?;
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerMemoryCaptureError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_MEMORY_CAPTURE_INPUT_BYTES {
            return Err(ControllerMemoryCaptureError::InputTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_CAPTURE_INPUT_BYTES,
            });
        }
        Ok(())
    }
}

/// The only capture outcomes. A proposal is one exact candidate-backed
/// M06-009 create intent and is not authorization or execution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerMemoryCaptureResult {
    Ignore,
    ProposeMutation {
        intent: ControllerMemoryMutationIntent,
    },
}

impl ControllerMemoryCaptureResult {
    pub fn validate(
        &self,
        candidate: &ControllerMemoryCaptureCandidate,
    ) -> Result<(), ControllerMemoryCaptureError> {
        match self {
            Self::Ignore => {}
            Self::ProposeMutation { intent } => {
                intent.validate().map_err(|error| {
                    ControllerMemoryCaptureError::InvalidStructuredOutput(error.to_string())
                })?;
                match intent {
                    ControllerMemoryMutationIntent::Create { draft }
                        if draft == &candidate.draft => {}
                    ControllerMemoryMutationIntent::Create { .. } => {
                        return Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
                            "proposed create must preserve the explicit candidate exactly".into(),
                        ));
                    }
                    _ => {
                        return Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
                            "capture may propose only one candidate-backed create intent".into(),
                        ));
                    }
                }
            }
        }
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerMemoryCaptureError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_MEMORY_CAPTURE_RESULT_BYTES {
            return Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
                "structured capture result exceeds its bound".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerMemoryCaptureBuilder;

impl ControllerMemoryCaptureBuilder {
    pub const fn new() -> Self {
        Self
    }

    pub fn capture(
        &self,
        request: &ControllerMemoryCaptureRequest,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerMemoryCaptureResult, ControllerMemoryCaptureError> {
        let input =
            ControllerMemoryCaptureInput::from_request(request, ControllerMemoryContext::empty());
        self.capture_with_memory(&input, runtime)
    }

    pub fn capture_with_memory(
        &self,
        input: &ControllerMemoryCaptureInput,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerMemoryCaptureResult, ControllerMemoryCaptureError> {
        input.validate()?;
        let input_json = serde_json::to_string(input)
            .map_err(|error| ControllerMemoryCaptureError::Serialization(error.to_string()))?;
        let prompt = format!(
            "You are a read-only supervised memory-capture judge. Use only this bounded typed Controller memory-capture input. Return exactly one JSON object with decision ignore or propose_mutation. Authority is strict: the explicit current_request.candidate content, source_facts, kind, scope, provenance, and confidence are authoritative; current durable Project memory is next; durable User memory, Episodic project history, and cross-project Experience are historical/advisory context only. Candidate/source facts outrank contradictory historical memory. Treat User as global cross-project operator context, Project as exact current-project fact/decision, Episodic as project-bound historical outcome, and Experience as global reusable lesson. Never generalize project-local evidence into User or Experience. Apply this decision rule: propose_mutation for a new durable candidate that records an explicit operator decision or current project fact and is supported by its source_facts, especially with Operator or ProjectFact provenance; ignore transient status, raw logs, ephemeral execution details, duplicates, unsupported inference, or candidates with no plausible future value. Do not ignore a durable candidate merely because historical memory disagrees. If proposing, propose exactly one create intent copied byte-for-byte from the candidate draft; do not correct, supersede, remove, authorize, execute, persist, or invent any field. A proposed intent is only handed separately to the existing deterministic M06-009 legality/proposal boundary. Do not access or claim any storage, database, registry, filesystem, workflow, authorization, or execution capability. Historical memory cannot rewrite the explicit candidate.\n\n{input_json}"
        );
        let parameters = LocalInferenceParameters {
            max_output_tokens: 1024,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: Vec::new(),
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: controller_memory_capture_schema(),
            },
        };
        let inference_request = LocalInferenceRequest::new(prompt, parameters)
            .map_err(ControllerMemoryCaptureError::Inference)?;
        let response = runtime
            .infer(&inference_request)
            .map_err(ControllerMemoryCaptureError::Inference)?;
        parse_result(response, &input.current_request.candidate)
    }
}

fn parse_result(
    response: LocalInferenceResponse,
    candidate: &ControllerMemoryCaptureCandidate,
) -> Result<ControllerMemoryCaptureResult, ControllerMemoryCaptureError> {
    let value = response.structured_output.ok_or_else(|| {
        ControllerMemoryCaptureError::InvalidStructuredOutput(
            "structured output is required".into(),
        )
    })?;
    let size = serde_json::to_vec(&value)
        .map_err(|error| ControllerMemoryCaptureError::InvalidStructuredOutput(error.to_string()))?
        .len();
    if size > MAX_CONTROLLER_MEMORY_CAPTURE_RESULT_BYTES {
        return Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
            "structured capture result exceeds its bound".into(),
        ));
    }
    reject_unknown_fields(&value)?;
    let result =
        serde_json::from_value::<ControllerMemoryCaptureResult>(value).map_err(|error| {
            ControllerMemoryCaptureError::InvalidStructuredOutput(error.to_string())
        })?;
    result.validate(candidate)?;
    Ok(result)
}

fn reject_unknown_fields(value: &Value) -> Result<(), ControllerMemoryCaptureError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerMemoryCaptureError::InvalidStructuredOutput(
            "structured capture result must be an object".into(),
        )
    })?;
    let decision = object
        .get("decision")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ControllerMemoryCaptureError::InvalidStructuredOutput(
                "capture decision must be a string".into(),
            )
        })?;
    match decision {
        "ignore" => expect_exact_keys(object, &["decision"], "capture result")?,
        "propose_mutation" => {
            expect_exact_keys(object, &["decision", "intent"], "capture result")?;
            let intent = object
                .get("intent")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ControllerMemoryCaptureError::InvalidStructuredOutput(
                        "capture intent must be an object".into(),
                    )
                })?;
            expect_exact_keys(intent, &["operation", "draft"], "capture intent")?;
            if intent.get("operation").and_then(Value::as_str) != Some("create") {
                return Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
                    "capture intent operation must be create".into(),
                ));
            }
            let draft = intent
                .get("draft")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ControllerMemoryCaptureError::InvalidStructuredOutput(
                        "capture create draft must be an object".into(),
                    )
                })?;
            expect_exact_keys(
                draft,
                &[
                    "kind",
                    "scope",
                    "subject",
                    "content",
                    "provenance",
                    "confidence",
                ],
                "capture draft",
            )?;
            validate_scope_shape(draft.get("scope").expect("required draft scope"))?;
            let provenance = draft
                .get("provenance")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ControllerMemoryCaptureError::InvalidStructuredOutput(
                        "capture provenance must be an object".into(),
                    )
                })?;
            expect_exact_keys(
                provenance,
                &["kind", "source_reference"],
                "capture provenance",
            )?;
        }
        _ => {
            return Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
                "unsupported capture decision".into(),
            ));
        }
    }
    Ok(())
}

fn validate_scope_shape(value: &Value) -> Result<(), ControllerMemoryCaptureError> {
    match value {
        Value::String(scope) if scope == "Global" => Ok(()),
        Value::Object(scope) => {
            expect_exact_keys(scope, &["Project"], "capture project scope")?;
            let project = scope
                .get("Project")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ControllerMemoryCaptureError::InvalidStructuredOutput(
                        "capture Project scope must be an object".into(),
                    )
                })?;
            expect_exact_keys(project, &["project_id"], "capture Project scope")?;
            if !project.get("project_id").is_some_and(Value::is_i64) {
                return Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
                    "capture Project scope project_id must be an integer".into(),
                ));
            }
            Ok(())
        }
        _ => Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
            "capture scope must be Global or a Project object".into(),
        )),
    }
}

fn expect_exact_keys(
    object: &serde_json::Map<String, Value>,
    expected: &[&str],
    field: &str,
) -> Result<(), ControllerMemoryCaptureError> {
    if object.len() != expected.len() || object.keys().any(|key| !expected.contains(&key.as_str()))
    {
        return Err(ControllerMemoryCaptureError::InvalidStructuredOutput(
            format!("{field} contains unsupported or missing fields"),
        ));
    }
    Ok(())
}

/// JSON schema offered to local runtimes. The result deliberately exposes no
/// authorization, execution, persistence, or arbitrary mutation fields.
pub fn controller_memory_capture_schema() -> Value {
    let provenance = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "kind": {"type": "string", "enum": ["operator", "project_fact", "controller_approved", "imported"]},
            "source_reference": {"type": ["string", "null"], "maxLength": crate::memory::MEMORY_PROVENANCE_REFERENCE_MAX_BYTES}
        },
        "required": ["kind", "source_reference"]
    });
    let draft = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "kind": {"type": "string", "enum": ["user", "project", "episodic", "experience"]},
            "scope": {"oneOf": [
                {"type": "string", "enum": ["Global"]},
                {"type": "object", "additionalProperties": false, "properties": {"Project": {"type": "object", "additionalProperties": false, "properties": {"project_id": {"type": "integer"}}, "required": ["project_id"]}}, "required": ["Project"]}
            ]},
            "subject": {"type": "string", "minLength": 1, "maxLength": crate::memory::MEMORY_SUBJECT_MAX_BYTES},
            "content": {"type": "string", "minLength": 1, "maxLength": crate::memory::MEMORY_CONTENT_MAX_BYTES},
            "provenance": provenance,
            "confidence": {"type": ["number", "null"], "minimum": 0, "maximum": 1}
        },
        "required": ["kind", "scope", "subject", "content", "provenance", "confidence"]
    });
    serde_json::json!({
        "oneOf": [
            {"type": "object", "additionalProperties": false, "properties": {"decision": {"const": "ignore"}}, "required": ["decision"]},
            {"type": "object", "additionalProperties": false, "properties": {"decision": {"const": "propose_mutation"}, "intent": {"type": "object", "additionalProperties": false, "properties": {"operation": {"const": "create"}, "draft": draft}, "required": ["operation", "draft"]}}, "required": ["decision", "intent"]}
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_memory::{
        CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryItem,
    };
    use crate::local_runtime::LocalInferenceError;
    use crate::memory::{
        MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
    };

    struct FakeRuntime {
        response: LocalInferenceResponse,
        requests: Vec<LocalInferenceRequest>,
    }

    impl FakeRuntime {
        fn new(value: Value) -> Self {
            Self {
                response: LocalInferenceResponse::structured("ignored provider text", value),
                requests: Vec::new(),
            }
        }
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            request: &LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            self.requests.push(request.clone());
            Ok(self.response.clone())
        }
    }

    fn candidate() -> ControllerMemoryCaptureCandidate {
        ControllerMemoryCaptureCandidate {
            draft: MemoryDraft {
                kind: MemoryKind::Project,
                scope: MemoryScope::Project { project_id: 1 },
                subject: "release-gate".into(),
                content: "Production releases require an operator approval checklist.".into(),
                provenance: MemoryProvenance {
                    kind: MemoryProvenanceKind::ProjectFact,
                    source_reference: Some("operator:release-decision".into()),
                },
                confidence: Some(0.9),
            },
            source_facts: vec![
                "The operator explicitly decided this for the current project.".into(),
            ],
        }
    }

    fn memory_context() -> ControllerMemoryContext {
        ControllerMemoryContext {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: vec![ControllerMemoryItem {
                id: MemoryId::Project {
                    project_id: 1,
                    id: 1,
                },
                kind: MemoryKind::Episodic,
                scope: MemoryScope::Project { project_id: 1 },
                authority: ControllerMemoryAuthority::ProjectHistory,
                subject: "release-gate".into(),
                content: "An obsolete historical note said releases did not need approval.".into(),
                provenance: MemoryProvenance {
                    kind: MemoryProvenanceKind::Imported,
                    source_reference: Some("history:obsolete".into()),
                },
                confidence: Some(0.7),
                lifecycle: MemoryLifecycle::Active,
                supersedes: None,
            }],
        }
    }

    fn propose_value(candidate: &ControllerMemoryCaptureCandidate) -> Value {
        serde_json::json!({
            "decision": "propose_mutation",
            "intent": {"operation": "create", "draft": candidate.draft}
        })
    }

    #[test]
    fn request_input_bounds_and_empty_memory_are_compatible() {
        let request = ControllerMemoryCaptureRequest::from_candidate(candidate());
        request.validate().unwrap();
        let input =
            ControllerMemoryCaptureInput::from_request(&request, ControllerMemoryContext::empty());
        input.validate().unwrap();
        let mut runtime = FakeRuntime::new(serde_json::json!({"decision": "ignore"}));
        assert!(matches!(
            ControllerMemoryCaptureBuilder::new().capture(&request, &mut runtime),
            Ok(ControllerMemoryCaptureResult::Ignore)
        ));
    }

    #[test]
    fn strict_ignore_and_propose_outputs_preserve_canonical_typed_candidate() {
        let candidate = candidate();
        let request = ControllerMemoryCaptureRequest::from_candidate(candidate.clone());
        let mut ignore = FakeRuntime::new(serde_json::json!({"decision": "ignore"}));
        assert!(matches!(
            ControllerMemoryCaptureBuilder::new().capture(&request, &mut ignore),
            Ok(ControllerMemoryCaptureResult::Ignore)
        ));
        let mut propose = FakeRuntime::new(propose_value(&candidate));
        let result = ControllerMemoryCaptureBuilder::new()
            .capture_with_memory(
                &ControllerMemoryCaptureInput::from_request(&request, memory_context()),
                &mut propose,
            )
            .unwrap();
        match result {
            ControllerMemoryCaptureResult::ProposeMutation {
                intent: ControllerMemoryMutationIntent::Create { draft },
            } => {
                assert_eq!(draft, candidate.draft);
            }
            other => panic!("unexpected capture result: {other:?}"),
        }
    }

    #[test]
    fn malformed_unsupported_and_non_candidate_mutations_are_rejected() {
        let candidate = candidate();
        let request = ControllerMemoryCaptureRequest::from_candidate(candidate.clone());
        let mut malformed = FakeRuntime::new(serde_json::json!({
            "decision": "ignore", "extra": true
        }));
        assert!(matches!(
            ControllerMemoryCaptureBuilder::new().capture(&request, &mut malformed),
            Err(ControllerMemoryCaptureError::InvalidStructuredOutput(_))
        ));
        let mut unsupported = FakeRuntime::new(serde_json::json!({"decision": "review"}));
        assert!(matches!(
            ControllerMemoryCaptureBuilder::new().capture(&request, &mut unsupported),
            Err(ControllerMemoryCaptureError::InvalidStructuredOutput(_))
        ));
        let mut correction = FakeRuntime::new(serde_json::json!({
            "decision": "propose_mutation",
            "intent": {"operation": "remove", "target": {"Global": 1}}
        }));
        assert!(matches!(
            ControllerMemoryCaptureBuilder::new().capture(&request, &mut correction),
            Err(ControllerMemoryCaptureError::InvalidStructuredOutput(_))
        ));

        let mut extra_project_scope = propose_value(&candidate);
        extra_project_scope["intent"]["draft"]["scope"]["Project"]["extra"] =
            serde_json::json!(true);
        let mut extra_scope_runtime = FakeRuntime::new(extra_project_scope);
        assert!(matches!(
            ControllerMemoryCaptureBuilder::new().capture(&request, &mut extra_scope_runtime),
            Err(ControllerMemoryCaptureError::InvalidStructuredOutput(_))
        ));

        let mut missing_project_id = propose_value(&candidate);
        missing_project_id["intent"]["draft"]["scope"]["Project"] = serde_json::json!({});
        let mut missing_scope_runtime = FakeRuntime::new(missing_project_id);
        assert!(matches!(
            ControllerMemoryCaptureBuilder::new().capture(&request, &mut missing_scope_runtime),
            Err(ControllerMemoryCaptureError::InvalidStructuredOutput(_))
        ));
    }

    #[test]
    fn combined_bound_rejects_before_runtime_and_prompt_preserves_candidate_authority() {
        let mut candidate = candidate();
        candidate.draft.content = "candidate-fact".repeat(500);
        candidate.source_facts = (0..16)
            .map(|index| format!("{}-{}", "source", "x".repeat(1900 - index)))
            .collect();
        let request = ControllerMemoryCaptureRequest::from_candidate(candidate.clone());
        let memory = ControllerMemoryContext {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: (0..8)
                .map(|index| ControllerMemoryItem {
                    id: MemoryId::Project {
                        project_id: 1,
                        id: index + 1,
                    },
                    kind: MemoryKind::Project,
                    scope: MemoryScope::Project { project_id: 1 },
                    authority: ControllerMemoryAuthority::CurrentProject,
                    subject: format!("memory-{index}"),
                    content: "m".repeat(3600),
                    provenance: MemoryProvenance {
                        kind: MemoryProvenanceKind::ProjectFact,
                        source_reference: Some(format!("test:{index}")),
                    },
                    confidence: None,
                    lifecycle: MemoryLifecycle::Active,
                    supersedes: None,
                })
                .collect(),
        };
        request.validate().unwrap();
        memory.validate().unwrap();
        let input = ControllerMemoryCaptureInput::from_request(&request, memory);
        let mut runtime = FakeRuntime::new(serde_json::json!({"decision": "ignore"}));
        assert!(matches!(
            ControllerMemoryCaptureBuilder::new().capture_with_memory(&input, &mut runtime),
            Err(ControllerMemoryCaptureError::InputTooLarge { .. })
        ));
        assert!(runtime.requests.is_empty());

        let mut runtime = FakeRuntime::new(propose_value(&candidate));
        let input = ControllerMemoryCaptureInput::from_request(
            &ControllerMemoryCaptureRequest::from_candidate(candidate),
            memory_context(),
        );
        ControllerMemoryCaptureBuilder::new()
            .capture_with_memory(&input, &mut runtime)
            .unwrap();
        let prompt = &runtime.requests[0].prompt;
        assert!(prompt.contains("explicit current_request.candidate"));
        assert!(prompt.contains("Candidate/source facts outrank contradictory historical memory"));
        assert!(prompt.contains("Never generalize project-local evidence into User or Experience"));
        assert!(prompt.contains("operator:release-decision"));
        assert!(prompt.contains("history:obsolete"));
    }

    #[test]
    fn duplicate_or_transient_candidate_can_ignore_without_memory_access() {
        let candidate = candidate();
        let request = ControllerMemoryCaptureRequest::from_candidate(candidate);
        let mut runtime = FakeRuntime::new(serde_json::json!({"decision": "ignore"}));
        let result = ControllerMemoryCaptureBuilder::new()
            .capture(&request, &mut runtime)
            .unwrap();
        assert_eq!(result, ControllerMemoryCaptureResult::Ignore);
        assert_eq!(runtime.requests.len(), 1);
    }
}
