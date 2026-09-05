//! Read-only Controller judgment for one explicitly selected memory record.
//!
//! Maintenance judgment may keep a record or return one exact canonical
//! M06-009 correction, supersession, or removal intent. It has no persistence,
//! authorization, execution, or broad-scanning capability.

use crate::controller_memory::ControllerMemoryContext;
use crate::controller_memory_mutation::ControllerMemoryMutationIntent;
use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::memory::{MemoryDraft, MemoryId, MemoryLifecycle, MemoryRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTROLLER_MEMORY_MAINTENANCE_REQUEST_VERSION: u32 = 1;
pub const MAX_CONTROLLER_MEMORY_MAINTENANCE_FACTS: usize = 16;
pub const MAX_CONTROLLER_MEMORY_MAINTENANCE_FACT_BYTES: usize = 2048;
pub const MAX_CONTROLLER_MEMORY_MAINTENANCE_REQUEST_BYTES: usize = 32 * 1024;
pub const MAX_CONTROLLER_MEMORY_MAINTENANCE_TARGET_BYTES: usize = 24 * 1024;
pub const MAX_CONTROLLER_MEMORY_MAINTENANCE_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_MEMORY_MAINTENANCE_RESULT_BYTES: usize = 32 * 1024;

#[derive(Debug, Error)]
pub enum ControllerMemoryMaintenanceError {
    #[error("Controller memory maintenance request is invalid: {0}")]
    InvalidRequest(String),
    #[error("Controller memory maintenance serialization failed: {0}")]
    Serialization(String),
    #[error("Controller memory maintenance memory context failed: {0}")]
    MemoryContext(String),
    #[error("Controller memory maintenance target is invalid: {0}")]
    InvalidTarget(String),
    #[error("Controller memory maintenance target was not found")]
    TargetNotFound,
    #[error("Controller memory maintenance target is not active")]
    TargetNotActive,
    #[error("Controller memory maintenance target is outside the current project")]
    CrossProjectTarget,
    #[error("Controller memory maintenance request is {actual} bytes; maximum is {max}")]
    RequestTooLarge { actual: usize, max: usize },
    #[error("Controller memory maintenance target is {actual} bytes; maximum is {max}")]
    TargetTooLarge { actual: usize, max: usize },
    #[error("Controller memory maintenance input is {actual} bytes; maximum is {max}")]
    InputTooLarge { actual: usize, max: usize },
    #[error("Controller memory maintenance output is malformed: {0}")]
    InvalidStructuredOutput(String),
    #[error("Controller memory maintenance inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
    #[error("Controller memory maintenance memory service failed: {0}")]
    MemoryService(String),
}

/// One explicit target and bounded current facts supplied by trusted Orc code.
/// It contains identity and declarative facts only, never a storage handle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemoryMaintenanceRequest {
    pub packet_version: u32,
    pub target: MemoryId,
    pub current_facts: Vec<String>,
}

impl ControllerMemoryMaintenanceRequest {
    pub fn new(target: MemoryId, current_facts: Vec<String>) -> Self {
        Self {
            packet_version: CONTROLLER_MEMORY_MAINTENANCE_REQUEST_VERSION,
            target,
            current_facts,
        }
    }

    pub fn target(&self) -> &MemoryId {
        &self.target
    }

    pub fn validate(&self) -> Result<(), ControllerMemoryMaintenanceError> {
        if self.packet_version != CONTROLLER_MEMORY_MAINTENANCE_REQUEST_VERSION {
            return Err(ControllerMemoryMaintenanceError::InvalidRequest(
                "unsupported Controller memory maintenance request version".into(),
            ));
        }
        validate_memory_id(&self.target)?;
        if self.current_facts.len() > MAX_CONTROLLER_MEMORY_MAINTENANCE_FACTS {
            return Err(ControllerMemoryMaintenanceError::InvalidRequest(
                "current_facts exceeds its item bound".into(),
            ));
        }
        for fact in &self.current_facts {
            if fact.trim().is_empty() || fact.len() > MAX_CONTROLLER_MEMORY_MAINTENANCE_FACT_BYTES {
                return Err(ControllerMemoryMaintenanceError::InvalidRequest(
                    "current facts must be non-empty and bounded".into(),
                ));
            }
        }
        let actual = serialized_size(self)?;
        if actual > MAX_CONTROLLER_MEMORY_MAINTENANCE_REQUEST_BYTES {
            return Err(ControllerMemoryMaintenanceError::RequestTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_MAINTENANCE_REQUEST_BYTES,
            });
        }
        Ok(())
    }
}

/// Capability-local input carrying the trusted resolved target and canonical
/// bounded context. No persistence, authorization, or execution capability is
/// represented here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemoryMaintenanceInput {
    pub current_request: ControllerMemoryMaintenanceRequest,
    pub target: MemoryRecord,
    pub memory: ControllerMemoryContext,
}

impl ControllerMemoryMaintenanceInput {
    pub fn from_resolved_target(
        request: &ControllerMemoryMaintenanceRequest,
        target: MemoryRecord,
        memory: ControllerMemoryContext,
    ) -> Self {
        Self {
            current_request: request.clone(),
            target,
            memory,
        }
    }

    pub fn validate(&self) -> Result<(), ControllerMemoryMaintenanceError> {
        self.current_request.validate()?;
        validate_resolved_target(&self.current_request.target, &self.target)?;
        self.memory
            .validate()
            .map_err(|error| ControllerMemoryMaintenanceError::MemoryContext(error.to_string()))?;
        let target_size = serialized_size(&self.target)?;
        if target_size > MAX_CONTROLLER_MEMORY_MAINTENANCE_TARGET_BYTES {
            return Err(ControllerMemoryMaintenanceError::TargetTooLarge {
                actual: target_size,
                max: MAX_CONTROLLER_MEMORY_MAINTENANCE_TARGET_BYTES,
            });
        }
        let actual = serialized_size(self)?;
        if actual > MAX_CONTROLLER_MEMORY_MAINTENANCE_INPUT_BYTES {
            return Err(ControllerMemoryMaintenanceError::InputTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_MAINTENANCE_INPUT_BYTES,
            });
        }
        Ok(())
    }
}

/// The only maintenance outcomes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerMemoryMaintenanceResult {
    Keep,
    ProposeMutation {
        intent: ControllerMemoryMutationIntent,
    },
}

impl ControllerMemoryMaintenanceResult {
    pub fn validate(
        &self,
        input: &ControllerMemoryMaintenanceInput,
    ) -> Result<(), ControllerMemoryMaintenanceError> {
        if let Self::ProposeMutation { intent } = self {
            intent.validate().map_err(|error| {
                ControllerMemoryMaintenanceError::InvalidStructuredOutput(error.to_string())
            })?;
            validate_maintenance_intent(intent, input)?;
        }
        let actual = serialized_size(self)?;
        if actual > MAX_CONTROLLER_MEMORY_MAINTENANCE_RESULT_BYTES {
            return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                "structured maintenance result exceeds its bound".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerMemoryMaintenanceBuilder;

impl ControllerMemoryMaintenanceBuilder {
    pub const fn new() -> Self {
        Self
    }

    pub fn maintain(
        &self,
        input: &ControllerMemoryMaintenanceInput,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerMemoryMaintenanceResult, ControllerMemoryMaintenanceError> {
        input.validate()?;
        let input_json = serde_json::to_string(input)
            .map_err(|error| ControllerMemoryMaintenanceError::Serialization(error.to_string()))?;
        let prompt = format!(
            "You are a read-only supervised memory-maintenance judge. Use only this bounded typed maintenance input. Return exactly one JSON object with decision keep or propose_mutation. The explicit target identity and current target record are canonical and authoritative. The explicit current_request.current_facts are authoritative current operator/project evidence. Controller memory context is bounded advisory context: current project facts and durable user facts outrank Episodic history and cross-project Experience, while historical/advisory memory never retargets the selected record or overrides current facts. Apply this decision procedure: when current_facts explicitly say the selected target is factually wrong or incorrect and provide its corrected value, output a Correct proposal; when current_facts explicitly establish a newer durable value or decision replacing a target that was valid historically, output a Supersede proposal; when current_facts explicitly establish that the target is obsolete and has no continuing durable value or replacement, output a Remove proposal; otherwise output Keep. Correct takes precedence when the facts explicitly call the current record wrong; use Supersede only when the old record was valid historically. Do not choose Keep when one of those three conditions is explicit. Keep ambiguous evidence. Project-local evidence must not rewrite unrelated global User or Experience memory. If proposing, return exactly one canonical M06-009 intent for the exact selected target: Correct or Supersede must preserve the target kind, scope, and subject in replacement; Remove has no replacement. Never create, target another identity, invent unsupported facts, authorize, execute, persist, or access storage, databases, registries, files, workflow, or mutation capabilities. The application separately passes any proposal through the existing deterministic M06-009 legality boundary.\n\n{input_json}"
        );
        let parameters = LocalInferenceParameters {
            max_output_tokens: 1024,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: Vec::new(),
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: controller_memory_maintenance_schema(),
            },
        };
        let request = LocalInferenceRequest::new(prompt, parameters)
            .map_err(ControllerMemoryMaintenanceError::Inference)?;
        let response = runtime
            .infer(&request)
            .map_err(ControllerMemoryMaintenanceError::Inference)?;
        parse_result(response, input)
    }
}

fn parse_result(
    response: LocalInferenceResponse,
    input: &ControllerMemoryMaintenanceInput,
) -> Result<ControllerMemoryMaintenanceResult, ControllerMemoryMaintenanceError> {
    let value = response.structured_output.ok_or_else(|| {
        ControllerMemoryMaintenanceError::InvalidStructuredOutput(
            "structured output is required".into(),
        )
    })?;
    let size = serde_json::to_vec(&value)
        .map_err(|error| {
            ControllerMemoryMaintenanceError::InvalidStructuredOutput(error.to_string())
        })?
        .len();
    if size > MAX_CONTROLLER_MEMORY_MAINTENANCE_RESULT_BYTES {
        return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
            "structured maintenance result exceeds its bound".into(),
        ));
    }
    reject_unknown_fields(&value)?;
    let result =
        serde_json::from_value::<ControllerMemoryMaintenanceResult>(value).map_err(|error| {
            ControllerMemoryMaintenanceError::InvalidStructuredOutput(error.to_string())
        })?;
    result.validate(input)?;
    Ok(result)
}

fn validate_maintenance_intent(
    intent: &ControllerMemoryMutationIntent,
    input: &ControllerMemoryMaintenanceInput,
) -> Result<(), ControllerMemoryMaintenanceError> {
    let target = &input.target;
    match intent {
        ControllerMemoryMutationIntent::Correct {
            target: intent_target,
            replacement,
        }
        | ControllerMemoryMutationIntent::Supersede {
            target: intent_target,
            replacement,
        } => {
            if intent_target != &target.id {
                return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                    "maintenance intent retargets a different memory identity".into(),
                ));
            }
            validate_replacement(replacement, target)?;
        }
        ControllerMemoryMutationIntent::Remove {
            target: intent_target,
        } => {
            if intent_target != &target.id {
                return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                    "maintenance intent retargets a different memory identity".into(),
                ));
            }
        }
        ControllerMemoryMutationIntent::Create { .. } => {
            return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                "maintenance cannot propose a create intent".into(),
            ));
        }
    }
    Ok(())
}

fn validate_replacement(
    replacement: &MemoryDraft,
    target: &MemoryRecord,
) -> Result<(), ControllerMemoryMaintenanceError> {
    if replacement.kind != target.kind
        || replacement.scope != target.scope
        || replacement.subject != target.subject
    {
        return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
            "maintenance replacement must preserve target kind, scope, and subject".into(),
        ));
    }
    Ok(())
}

fn validate_resolved_target(
    request_target: &MemoryId,
    target: &MemoryRecord,
) -> Result<(), ControllerMemoryMaintenanceError> {
    if target.id != *request_target {
        return Err(ControllerMemoryMaintenanceError::InvalidTarget(
            "resolved target identity does not match request".into(),
        ));
    }
    target
        .validate()
        .map_err(|error| ControllerMemoryMaintenanceError::InvalidTarget(error.to_string()))?;
    if target.lifecycle != MemoryLifecycle::Active {
        return Err(ControllerMemoryMaintenanceError::TargetNotActive);
    }
    Ok(())
}

fn validate_memory_id(id: &MemoryId) -> Result<(), ControllerMemoryMaintenanceError> {
    match id {
        MemoryId::Global(value) if *value > 0 => Ok(()),
        MemoryId::Project { project_id, id } if *project_id > 0 && *id > 0 => Ok(()),
        _ => Err(ControllerMemoryMaintenanceError::InvalidRequest(
            "memory target identity must use positive IDs".into(),
        )),
    }
}

fn serialized_size<T: Serialize>(value: &T) -> Result<usize, ControllerMemoryMaintenanceError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| ControllerMemoryMaintenanceError::Serialization(error.to_string()))
}

fn reject_unknown_fields(value: &Value) -> Result<(), ControllerMemoryMaintenanceError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerMemoryMaintenanceError::InvalidStructuredOutput(
            "structured maintenance result must be an object".into(),
        )
    })?;
    let decision = object
        .get("decision")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                "maintenance decision must be a string".into(),
            )
        })?;
    match decision {
        "keep" => expect_exact_keys(object, &["decision"], "maintenance result")?,
        "propose_mutation" => {
            expect_exact_keys(object, &["decision", "intent"], "maintenance result")?;
            let intent = object
                .get("intent")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                        "maintenance intent must be an object".into(),
                    )
                })?;
            let operation = intent
                .get("operation")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                        "maintenance operation must be a string".into(),
                    )
                })?;
            match operation {
                "correct" | "supersede" => {
                    expect_exact_keys(
                        intent,
                        &["operation", "target", "replacement"],
                        "maintenance replacement intent",
                    )?;
                    validate_memory_id_shape(intent.get("target").expect("required target"))?;
                    validate_draft_shape(intent.get("replacement").expect("required replacement"))?;
                }
                "remove" => {
                    expect_exact_keys(
                        intent,
                        &["operation", "target"],
                        "maintenance remove intent",
                    )?;
                    validate_memory_id_shape(intent.get("target").expect("required target"))?;
                }
                _ => {
                    return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                        "maintenance operation must be correct, supersede, or remove".into(),
                    ));
                }
            }
        }
        _ => {
            return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                "unsupported maintenance decision".into(),
            ));
        }
    }
    Ok(())
}

fn validate_draft_shape(value: &Value) -> Result<(), ControllerMemoryMaintenanceError> {
    let draft = value.as_object().ok_or_else(|| {
        ControllerMemoryMaintenanceError::InvalidStructuredOutput(
            "maintenance replacement must be an object".into(),
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
        "maintenance replacement",
    )?;
    validate_scope_shape(draft.get("scope").expect("required scope"))?;
    let provenance = draft
        .get("provenance")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                "maintenance provenance must be an object".into(),
            )
        })?;
    expect_exact_keys(
        provenance,
        &["kind", "source_reference"],
        "maintenance provenance",
    )?;
    Ok(())
}

fn validate_scope_shape(value: &Value) -> Result<(), ControllerMemoryMaintenanceError> {
    match value {
        Value::String(scope) if scope == "Global" => Ok(()),
        Value::Object(scope) => {
            expect_exact_keys(scope, &["Project"], "maintenance project scope")?;
            let project = scope
                .get("Project")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                        "maintenance Project scope must be an object".into(),
                    )
                })?;
            expect_exact_keys(project, &["project_id"], "maintenance Project scope")?;
            if !project.get("project_id").is_some_and(Value::is_i64) {
                return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                    "maintenance Project scope project_id must be an integer".into(),
                ));
            }
            Ok(())
        }
        _ => Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
            "maintenance scope must be Global or a Project object".into(),
        )),
    }
}

fn validate_memory_id_shape(value: &Value) -> Result<(), ControllerMemoryMaintenanceError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerMemoryMaintenanceError::InvalidStructuredOutput(
            "maintenance target must be an object".into(),
        )
    })?;
    match object.keys().next().map(String::as_str) {
        Some("Global") => {
            expect_exact_keys(object, &["Global"], "maintenance global target")?;
            if !object.get("Global").is_some_and(Value::is_i64) {
                return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                    "maintenance Global target must be an integer".into(),
                ));
            }
        }
        Some("Project") => {
            expect_exact_keys(object, &["Project"], "maintenance project target")?;
            let project = object
                .get("Project")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                        "maintenance Project target must be an object".into(),
                    )
                })?;
            expect_exact_keys(project, &["project_id", "id"], "maintenance Project target")?;
            if !project.get("project_id").is_some_and(Value::is_i64)
                || !project.get("id").is_some_and(Value::is_i64)
            {
                return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                    "maintenance Project target IDs must be integers".into(),
                ));
            }
        }
        _ => {
            return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
                "maintenance target must be Global or Project".into(),
            ));
        }
    }
    Ok(())
}

fn expect_exact_keys(
    object: &serde_json::Map<String, Value>,
    expected: &[&str],
    field: &str,
) -> Result<(), ControllerMemoryMaintenanceError> {
    if object.len() != expected.len() || object.keys().any(|key| !expected.contains(&key.as_str()))
    {
        return Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(
            format!("{field} contains unsupported or missing fields"),
        ));
    }
    Ok(())
}

pub fn controller_memory_maintenance_schema() -> Value {
    let memory_id = serde_json::json!({
        "oneOf": [
            {"type": "object", "additionalProperties": false, "properties": {"Global": {"type": "integer"}}, "required": ["Global"]},
            {"type": "object", "additionalProperties": false, "properties": {"Project": {"type": "object", "additionalProperties": false, "properties": {"project_id": {"type": "integer"}, "id": {"type": "integer"}}, "required": ["project_id", "id"]}}, "required": ["Project"]}
        ]
    });
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
    let replacement_intent = |operation: &str| {
        serde_json::json!({
            "type": "object", "additionalProperties": false,
            "properties": {"operation": {"const": operation}, "target": memory_id, "replacement": draft},
            "required": ["operation", "target", "replacement"]
        })
    };
    let remove_intent = serde_json::json!({
        "type": "object", "additionalProperties": false,
        "properties": {"operation": {"const": "remove"}, "target": memory_id},
        "required": ["operation", "target"]
    });
    serde_json::json!({
        "oneOf": [
            {"type": "object", "additionalProperties": false, "properties": {"decision": {"const": "keep"}}, "required": ["decision"]},
            {"type": "object", "additionalProperties": false, "properties": {"decision": {"const": "propose_mutation"}, "intent": {"oneOf": [replacement_intent("correct"), replacement_intent("supersede"), remove_intent]}}, "required": ["decision", "intent"]}
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
    use crate::memory::{MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope};

    struct FakeRuntime {
        response: LocalInferenceResponse,
        requests: Vec<LocalInferenceRequest>,
    }

    impl FakeRuntime {
        fn new(value: Value) -> Self {
            Self {
                response: LocalInferenceResponse::structured("ignored", value),
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

    fn target() -> MemoryRecord {
        MemoryRecord {
            id: MemoryId::Project {
                project_id: 1,
                id: 1,
            },
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id: 1 },
            subject: "release-gate".into(),
            content: "Releases used to require manual approval.".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::Imported,
                source_reference: Some("history:release-gate".into()),
            },
            confidence: Some(0.7),
            lifecycle: MemoryLifecycle::Active,
            supersedes: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn request() -> ControllerMemoryMaintenanceRequest {
        ControllerMemoryMaintenanceRequest::new(
            target().id,
            vec!["The operator now requires two-person approval.".into()],
        )
    }

    fn input() -> ControllerMemoryMaintenanceInput {
        ControllerMemoryMaintenanceInput::from_resolved_target(
            &request(),
            target(),
            ControllerMemoryContext::empty(),
        )
    }

    fn replacement() -> MemoryDraft {
        MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id: 1 },
            subject: "release-gate".into(),
            content: "Releases require two-person approval.".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::Operator,
                source_reference: Some("operator:release-gate".into()),
            },
            confidence: Some(0.95),
        }
    }

    fn proposal(operation: &str) -> Value {
        let target = serde_json::json!({"Project": {"project_id": 1, "id": 1}});
        match operation {
            "remove" => {
                serde_json::json!({"decision": "propose_mutation", "intent": {"operation": "remove", "target": target}})
            }
            _ => {
                serde_json::json!({"decision": "propose_mutation", "intent": {"operation": operation, "target": target, "replacement": replacement()}})
            }
        }
    }

    #[test]
    fn keep_and_all_maintenance_operations_are_strictly_supported() {
        let input = input();
        let mut keep = FakeRuntime::new(serde_json::json!({"decision": "keep"}));
        assert_eq!(
            ControllerMemoryMaintenanceBuilder::new()
                .maintain(&input, &mut keep)
                .unwrap(),
            ControllerMemoryMaintenanceResult::Keep
        );
        for operation in ["correct", "supersede", "remove"] {
            let mut runtime = FakeRuntime::new(proposal(operation));
            let result = ControllerMemoryMaintenanceBuilder::new()
                .maintain(&input, &mut runtime)
                .unwrap();
            assert!(matches!(
                result,
                ControllerMemoryMaintenanceResult::ProposeMutation { .. }
            ));
        }
    }

    #[test]
    fn malformed_nested_outputs_and_retargeting_are_rejected() {
        let input = input();
        let mut extra = proposal("correct");
        extra["intent"]["replacement"]["scope"]["Project"]["extra"] = serde_json::json!(true);
        let mut runtime = FakeRuntime::new(extra);
        assert!(matches!(
            ControllerMemoryMaintenanceBuilder::new().maintain(&input, &mut runtime),
            Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(_))
        ));
        let mut missing_scope_field = proposal("correct");
        missing_scope_field["intent"]["replacement"]["scope"]["Project"] = serde_json::json!({});
        let mut runtime = FakeRuntime::new(missing_scope_field);
        assert!(matches!(
            ControllerMemoryMaintenanceBuilder::new().maintain(&input, &mut runtime),
            Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(_))
        ));
        let mut retargeted = proposal("remove");
        retargeted["intent"]["target"] = serde_json::json!({"Project": {"project_id": 1, "id": 2}});
        let mut runtime = FakeRuntime::new(retargeted);
        assert!(matches!(
            ControllerMemoryMaintenanceBuilder::new().maintain(&input, &mut runtime),
            Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(_))
        ));
        let mut extra_target_field = proposal("remove");
        extra_target_field["intent"]["target"]["Project"]["extra"] = serde_json::json!(true);
        let mut runtime = FakeRuntime::new(extra_target_field);
        assert!(matches!(
            ControllerMemoryMaintenanceBuilder::new().maintain(&input, &mut runtime),
            Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(_))
        ));
    }

    #[test]
    fn replacement_identity_mismatch_and_combined_bound_reject_before_runtime() {
        let input = input();
        let mut wrong_subject = proposal("correct");
        wrong_subject["intent"]["replacement"]["subject"] = serde_json::json!("other");
        let mut runtime = FakeRuntime::new(wrong_subject);
        assert!(matches!(
            ControllerMemoryMaintenanceBuilder::new().maintain(&input, &mut runtime),
            Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(_))
        ));
        let mut wrong_kind = proposal("correct");
        wrong_kind["intent"]["replacement"]["kind"] = serde_json::json!("user");
        wrong_kind["intent"]["replacement"]["scope"] = serde_json::json!("Global");
        let mut runtime = FakeRuntime::new(wrong_kind);
        assert!(matches!(
            ControllerMemoryMaintenanceBuilder::new().maintain(&input, &mut runtime),
            Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(_))
        ));
        let mut request = request();
        request.current_facts = (0..16).map(|_| "x".repeat(1900)).collect();
        let mut large_target = target();
        large_target.content = "t".repeat(16_000);
        let oversized = ControllerMemoryMaintenanceInput::from_resolved_target(
            &request,
            large_target,
            ControllerMemoryContext {
                context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
                items: (0..8)
                    .map(|index| ControllerMemoryItem {
                        id: MemoryId::Project {
                            project_id: 1,
                            id: index + 2,
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
            },
        );
        let mut runtime = FakeRuntime::new(serde_json::json!({"decision": "keep"}));
        assert!(matches!(
            ControllerMemoryMaintenanceBuilder::new().maintain(&oversized, &mut runtime),
            Err(ControllerMemoryMaintenanceError::InputTooLarge { .. })
        ));
        assert!(runtime.requests.is_empty());
    }

    #[test]
    fn prompt_preserves_target_and_current_fact_authority() {
        let mut runtime = FakeRuntime::new(serde_json::json!({"decision": "keep"}));
        ControllerMemoryMaintenanceBuilder::new()
            .maintain(&input(), &mut runtime)
            .unwrap();
        let prompt = &runtime.requests[0].prompt;
        assert!(prompt.contains("explicit target identity and current target record"));
        assert!(prompt.contains("current_request.current_facts"));
        assert!(prompt.contains("historical/advisory memory never retargets"));
        assert!(prompt.contains(
            "Project-local evidence must not rewrite unrelated global User or Experience memory"
        ));
    }
}
