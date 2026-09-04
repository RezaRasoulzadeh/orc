//! Read-only Controller judgment for normal workflow intake.
//!
//! Intake is deliberately separate from the legacy Lead domain. It produces
//! only a bounded semantic outcome and, for `DirectTasks`, canonical task
//! proposals. The workflow kernel remains responsible for routing, gates,
//! persistence, and application.

use crate::discovery::ProjectDiscoverySnapshot;
use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::protocol::{PlanResponse, PlanningProjectState, TaskProposal};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTROLLER_INTAKE_REQUEST_VERSION: u32 = 1;
pub const MAX_CONTROLLER_INTAKE_REQUEST_BYTES: usize = 48 * 1024;
pub const MAX_CONTROLLER_INTAKE_RESULT_BYTES: usize = 48 * 1024;

const MAX_TEXT_BYTES: usize = 2048;
const MAX_LIST_ITEMS: usize = 24;
const MAX_TASKS: usize = 16;
const MAX_DETAILS_BYTES: usize = 2048;

#[derive(Debug, Error)]
pub enum ControllerIntakeError {
    #[error("controller intake request is invalid: {0}")]
    InvalidRequest(String),
    #[error("controller intake request serialization failed: {0}")]
    Serialization(String),
    #[error("controller intake request is {actual} bytes; maximum is {max}")]
    RequestTooLarge { actual: usize, max: usize },
    #[error("controller intake output is malformed: {0}")]
    InvalidStructuredOutput(String),
    #[error("controller intake inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerIntakeTask {
    pub id: String,
    pub title: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerIntakeState {
    pub task_counts: Vec<ControllerIntakeCount>,
    pub ready_tasks: Vec<ControllerIntakeTask>,
    pub active_tasks: Vec<ControllerIntakeTask>,
    pub review_tasks: Vec<ControllerIntakeTask>,
    pub blocked_tasks: Vec<ControllerIntakeTask>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerIntakeCount {
    pub status: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerIntakeDiscovery {
    pub fingerprint: String,
    pub technology_stack: Vec<String>,
    pub important_files: Vec<String>,
    pub architecture_boundaries: Vec<String>,
    pub unknowns_and_risks: Vec<String>,
    pub validation_commands: Vec<String>,
    pub state: ControllerIntakeState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerIntakeFact {
    pub key: String,
    pub value: String,
}

/// Bounded, model-independent intake input. It contains only canonical
/// objective/project/discovery facts and optional operator context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerIntakeRequest {
    pub packet_version: u32,
    pub kind: String,
    pub project_name: String,
    pub engineering_contract: String,
    pub objective: String,
    pub project_facts: Vec<ControllerIntakeFact>,
    pub discovery: ControllerIntakeDiscovery,
    pub operator_resolution: Option<String>,
}

impl ControllerIntakeRequest {
    pub fn from_canonical(
        project_name: &str,
        engineering_contract: &str,
        objective: &str,
        project_facts: &std::collections::BTreeMap<String, String>,
        snapshot: &ProjectDiscoverySnapshot,
        operator_resolution: Option<&str>,
    ) -> Result<Self, ControllerIntakeError> {
        let request = Self {
            packet_version: CONTROLLER_INTAKE_REQUEST_VERSION,
            kind: "workflow_intake".into(),
            project_name: bound_text(project_name),
            engineering_contract: bound_text(engineering_contract),
            objective: bound_text(objective),
            project_facts: project_facts
                .iter()
                .take(MAX_LIST_ITEMS)
                .map(|(key, value)| ControllerIntakeFact {
                    key: bound_text(key),
                    value: bound_text(value),
                })
                .collect(),
            discovery: ControllerIntakeDiscovery::from_snapshot(snapshot),
            operator_resolution: operator_resolution.map(bound_text),
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), ControllerIntakeError> {
        if self.packet_version != CONTROLLER_INTAKE_REQUEST_VERSION {
            return Err(ControllerIntakeError::InvalidRequest(
                "unsupported Controller intake request version".into(),
            ));
        }
        for (field, value) in [
            ("kind", &self.kind),
            ("project_name", &self.project_name),
            ("engineering_contract", &self.engineering_contract),
            ("objective", &self.objective),
            ("discovery.fingerprint", &self.discovery.fingerprint),
        ] {
            validate_text(value, field)?;
        }
        if self.objective.trim().is_empty() {
            return Err(ControllerIntakeError::InvalidRequest(
                "objective must not be empty".into(),
            ));
        }
        for fact in &self.project_facts {
            validate_text(&fact.key, "project_facts.key")?;
            validate_text(&fact.value, "project_facts.value")?;
        }
        validate_discovery(&self.discovery)?;
        if let Some(resolution) = &self.operator_resolution {
            validate_text(resolution, "operator_resolution")?;
            if resolution.trim().is_empty() {
                return Err(ControllerIntakeError::InvalidRequest(
                    "operator_resolution must not be empty".into(),
                ));
            }
        }
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerIntakeError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_INTAKE_REQUEST_BYTES {
            return Err(ControllerIntakeError::RequestTooLarge {
                actual,
                max: MAX_CONTROLLER_INTAKE_REQUEST_BYTES,
            });
        }
        Ok(())
    }
}

impl ControllerIntakeDiscovery {
    fn from_snapshot(snapshot: &ProjectDiscoverySnapshot) -> Self {
        Self {
            fingerprint: bound_text(&snapshot.fingerprint),
            technology_stack: bound_strings(&snapshot.technology_stack),
            important_files: bound_strings(&snapshot.important_files),
            architecture_boundaries: bound_strings(&snapshot.architecture_boundaries),
            unknowns_and_risks: bound_strings(&snapshot.unknowns_and_risks),
            validation_commands: bound_strings(&snapshot.validation_commands),
            state: ControllerIntakeState::from_canonical(&snapshot.task_state),
        }
    }
}

impl ControllerIntakeState {
    fn from_canonical(state: &PlanningProjectState) -> Self {
        Self {
            task_counts: state
                .task_counts
                .iter()
                .take(MAX_LIST_ITEMS)
                .map(|(status, count)| ControllerIntakeCount {
                    status: bound_text(status),
                    count: *count,
                })
                .collect(),
            ready_tasks: bound_tasks(&state.ready_tasks),
            active_tasks: bound_tasks(&state.active_tasks),
            review_tasks: bound_tasks(&state.review_tasks),
            blocked_tasks: bound_tasks(&state.blocked_tasks),
        }
    }
}

fn bound_tasks(tasks: &[crate::protocol::TaskSummary]) -> Vec<ControllerIntakeTask> {
    tasks
        .iter()
        .take(MAX_LIST_ITEMS)
        .map(|task| ControllerIntakeTask {
            id: bound_text(&task.id),
            title: bound_text(&task.title),
            status: bound_text(&task.status),
        })
        .collect()
}

fn validate_discovery(discovery: &ControllerIntakeDiscovery) -> Result<(), ControllerIntakeError> {
    for (field, values) in [
        ("technology_stack", &discovery.technology_stack),
        ("important_files", &discovery.important_files),
        (
            "architecture_boundaries",
            &discovery.architecture_boundaries,
        ),
        ("unknowns_and_risks", &discovery.unknowns_and_risks),
        ("validation_commands", &discovery.validation_commands),
    ] {
        validate_strings(values, field)?;
    }
    if discovery.state.task_counts.len() > MAX_LIST_ITEMS {
        return Err(ControllerIntakeError::InvalidRequest(
            "discovery.state.task_counts exceeds its item bound".into(),
        ));
    }
    for count in &discovery.state.task_counts {
        validate_text(&count.status, "discovery.state.task_counts.status")?;
    }
    for (field, tasks) in [
        ("ready_tasks", &discovery.state.ready_tasks),
        ("active_tasks", &discovery.state.active_tasks),
        ("review_tasks", &discovery.state.review_tasks),
        ("blocked_tasks", &discovery.state.blocked_tasks),
    ] {
        if tasks.len() > MAX_LIST_ITEMS {
            return Err(ControllerIntakeError::InvalidRequest(format!(
                "discovery.state.{field} exceeds its item bound"
            )));
        }
        for task in tasks {
            validate_text(&task.id, "discovery.state.task.id")?;
            validate_text(&task.title, "discovery.state.task.title")?;
            validate_text(&task.status, "discovery.state.task.status")?;
        }
    }
    Ok(())
}

/// The only semantic intake outcomes. Controller output cannot name a
/// workflow stage, status, persistence identity, or lineage record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerIntakeDecision {
    DirectTasks,
    PlanRequired,
    UserDecisionRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerIntakeResult {
    pub decision: ControllerIntakeDecision,
    pub details: String,
    pub direct_tasks: Vec<TaskProposal>,
}

impl ControllerIntakeResult {
    pub fn validate(&self) -> Result<(), ControllerIntakeError> {
        validate_text(&self.details, "details")?;
        if self.details.trim().is_empty() {
            return Err(ControllerIntakeError::InvalidStructuredOutput(
                "details must not be empty".into(),
            ));
        }
        if self.direct_tasks.len() > MAX_TASKS {
            return Err(ControllerIntakeError::InvalidStructuredOutput(
                "direct_tasks exceeds its item bound".into(),
            ));
        }
        let direct_response = PlanResponse {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            objective: "Controller direct tasks".into(),
            assumptions: Vec::new(),
            risks: Vec::new(),
            questions: Vec::new(),
            tasks: self.direct_tasks.clone(),
        };
        direct_response
            .validate()
            .map_err(|error| ControllerIntakeError::InvalidStructuredOutput(error.to_string()))?;
        match self.decision {
            ControllerIntakeDecision::DirectTasks if self.direct_tasks.is_empty() => {
                Err(ControllerIntakeError::InvalidStructuredOutput(
                    "direct_tasks decision requires at least one task".into(),
                ))
            }
            ControllerIntakeDecision::DirectTasks => Ok(()),
            ControllerIntakeDecision::PlanRequired
            | ControllerIntakeDecision::UserDecisionRequired
                if !self.direct_tasks.is_empty() =>
            {
                Err(ControllerIntakeError::InvalidStructuredOutput(
                    "direct_tasks are allowed only for the direct_tasks decision".into(),
                ))
            }
            ControllerIntakeDecision::PlanRequired
            | ControllerIntakeDecision::UserDecisionRequired => Ok(()),
        }?;
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerIntakeError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_INTAKE_RESULT_BYTES {
            return Err(ControllerIntakeError::InvalidStructuredOutput(
                "structured intake result exceeds its bound".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerIntakeBuilder;

impl ControllerIntakeBuilder {
    pub const fn new() -> Self {
        Self
    }

    pub fn classify(
        &self,
        request: &ControllerIntakeRequest,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerIntakeResult, ControllerIntakeError> {
        request.validate()?;
        let input = serde_json::to_string(request)
            .map_err(|error| ControllerIntakeError::Serialization(error.to_string()))?;
        let prompt = format!(
            "You are Orc's read-only Controller intake classifier. Use only the bounded canonical JSON below. Return exactly one JSON object with decision, details, and direct_tasks. decision must be exactly direct_tasks, plan_required, or user_decision_required. Choose direct_tasks only when the objective can be completed by a small set of immediately actionable task proposals; include complete canonical task proposals in direct_tasks. Choose plan_required when the work needs decomposition, sequencing, or a supervised Plan. Choose user_decision_required when an explicit operator choice or unresolved ambiguity must be answered before routing. direct_tasks must be an empty array for plan_required and user_decision_required. Do not create tasks, persist decisions, apply plans, invoke other workflow stages, or invent facts. Controller judgment is advisory; Orc's workflow kernel owns routing and mutation. Respond with only the strict structured JSON object.\n\n{input}"
        );
        let parameters = LocalInferenceParameters {
            max_output_tokens: 2048,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: Vec::new(),
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: controller_intake_schema(),
            },
        };
        let inference_request = LocalInferenceRequest::new(prompt, parameters)
            .map_err(ControllerIntakeError::Inference)?;
        let response = runtime
            .infer(&inference_request)
            .map_err(ControllerIntakeError::Inference)?;
        parse_result(response)
    }
}

pub fn controller_intake_schema() -> Value {
    let task_schema =
        crate::automated::canonical_plan_response_schema()["properties"]["tasks"]["items"].clone();
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "decision": {"type": "string", "enum": ["direct_tasks", "plan_required", "user_decision_required"]},
            "details": {"type": "string", "minLength": 1, "maxLength": MAX_DETAILS_BYTES},
            "direct_tasks": {"type": "array", "maxItems": MAX_TASKS, "items": task_schema}
        },
        "required": ["decision", "details", "direct_tasks"]
    })
}

fn parse_result(
    response: LocalInferenceResponse,
) -> Result<ControllerIntakeResult, ControllerIntakeError> {
    let value = response.structured_output.ok_or_else(|| {
        ControllerIntakeError::InvalidStructuredOutput("structured output is required".into())
    })?;
    let size = serde_json::to_vec(&value)
        .map_err(|error| ControllerIntakeError::InvalidStructuredOutput(error.to_string()))?
        .len();
    if size > MAX_CONTROLLER_INTAKE_RESULT_BYTES {
        return Err(ControllerIntakeError::InvalidStructuredOutput(
            "structured output exceeds its bound".into(),
        ));
    }
    reject_unknown_fields(&value)?;
    let result = serde_json::from_value::<ControllerIntakeResult>(value)
        .map_err(|error| ControllerIntakeError::InvalidStructuredOutput(error.to_string()))?;
    result.validate()?;
    Ok(result)
}

fn reject_unknown_fields(value: &Value) -> Result<(), ControllerIntakeError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerIntakeError::InvalidStructuredOutput("structured output must be an object".into())
    })?;
    const RESULT_FIELDS: &[&str] = &["decision", "details", "direct_tasks"];
    if let Some(field) = object
        .keys()
        .find(|key| !RESULT_FIELDS.contains(&key.as_str()))
    {
        return Err(ControllerIntakeError::InvalidStructuredOutput(format!(
            "unsupported intake result field '{field}'"
        )));
    }
    let Some(tasks) = object.get("direct_tasks") else {
        return Ok(());
    };
    let Some(tasks) = tasks.as_array() else {
        return Ok(());
    };
    const TASK_FIELDS: &[&str] = &[
        "local_id",
        "title",
        "objective",
        "role",
        "priority",
        "depends_on",
        "capabilities",
        "scope_mode",
        "context_files",
        "expected_changes",
        "unchanged",
        "acceptance_criteria",
        "required_tests",
        "validation",
        "execution_hints",
        "risk_factors",
    ];
    const EXECUTION_HINT_FIELDS: &[&str] = &["class", "model", "effort", "effort_reason"];
    for task in tasks {
        let Some(task) = task.as_object() else {
            continue;
        };
        if let Some(field) = task.keys().find(|key| !TASK_FIELDS.contains(&key.as_str())) {
            return Err(ControllerIntakeError::InvalidStructuredOutput(format!(
                "unsupported direct task field '{field}'"
            )));
        }
        if let Some(hints) = task.get("execution_hints").and_then(Value::as_object)
            && let Some(field) = hints
                .keys()
                .find(|key| !EXECUTION_HINT_FIELDS.contains(&key.as_str()))
        {
            return Err(ControllerIntakeError::InvalidStructuredOutput(format!(
                "unsupported execution hint field '{field}'"
            )));
        }
    }
    Ok(())
}

fn validate_strings(values: &[String], field: &str) -> Result<(), ControllerIntakeError> {
    if values.len() > MAX_LIST_ITEMS {
        return Err(ControllerIntakeError::InvalidRequest(format!(
            "{field} exceeds its item bound"
        )));
    }
    for value in values {
        validate_text(value, field)?;
    }
    Ok(())
}

fn validate_text(value: &str, field: &str) -> Result<(), ControllerIntakeError> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(ControllerIntakeError::InvalidRequest(format!(
            "{field} exceeds its byte bound"
        )));
    }
    Ok(())
}

fn bound_strings(values: &[String]) -> Vec<String> {
    values
        .iter()
        .take(MAX_LIST_ITEMS)
        .map(|value| bound_text(value))
        .collect()
}

fn bound_text(value: &str) -> String {
    if value.len() <= MAX_TEXT_BYTES {
        return value.to_owned();
    }
    value
        .char_indices()
        .take_while(|(index, _)| *index < MAX_TEXT_BYTES)
        .map(|(_, character)| character)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_runtime::LocalInferenceRequest;

    struct FakeRuntime {
        value: Value,
        requests: usize,
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            _: &LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            self.requests += 1;
            Ok(LocalInferenceResponse::structured(
                "ignored",
                self.value.clone(),
            ))
        }
    }

    fn request() -> ControllerIntakeRequest {
        let snapshot = ProjectDiscoverySnapshot {
            repository: crate::discovery::RepositorySnapshot {
                root: "omitted".into(),
                branch: None,
                commit: None,
                changed_files: Vec::new(),
            },
            project: crate::discovery::ProjectMetadata {
                name: "intake-test".into(),
                description: None,
                engineering_contract: None,
            },
            architecture: crate::discovery::ArchitectureSnapshot::default(),
            technology_stack: vec!["Rust".into()],
            important_files: vec!["Cargo.toml".into()],
            manifests: vec!["Cargo.toml".into()],
            test_locations: vec!["tests/".into()],
            architecture_boundaries: vec!["src".into()],
            unknowns_and_risks: Vec::new(),
            fingerprint: "discovery-test".into(),
            validation_commands: vec!["cargo test".into()],
            task_state: PlanningProjectState {
                task_counts: [("ready".into(), 1)].into_iter().collect(),
                ready_tasks: Vec::new(),
                active_tasks: Vec::new(),
                review_tasks: Vec::new(),
                blocked_tasks: Vec::new(),
                usable_agents: Vec::new(),
                busy_agents: Vec::new(),
                quota_reserve_percent: 10,
            },
        };
        ControllerIntakeRequest::from_canonical(
            "intake-test",
            "Keep the change bounded.",
            "Inspect the project state.",
            &std::collections::BTreeMap::new(),
            &snapshot,
            None,
        )
        .unwrap()
    }

    fn output(decision: &str, tasks: Vec<TaskProposal>) -> Value {
        serde_json::json!({
            "decision": decision,
            "details": "bounded intake judgment",
            "direct_tasks": tasks,
        })
    }

    #[test]
    fn strict_classifier_accepts_all_three_semantic_outcomes_without_mutation() {
        for decision in ["direct_tasks", "plan_required", "user_decision_required"] {
            let tasks = if decision == "direct_tasks" {
                vec![crate::protocol::TaskProposal {
                    local_id: "inspect".into(),
                    title: "Inspect project".into(),
                    objective: "Inspect the project state.".into(),
                    role: "developer".into(),
                    priority: crate::task::TaskPriority::Normal,
                    depends_on: Vec::new(),
                    capabilities: Vec::new(),
                    scope_mode: None,
                    context_files: Vec::new(),
                    expected_changes: vec!["inspection result".into()],
                    unchanged: vec!["project behavior".into()],
                    acceptance_criteria: vec!["The state is documented.".into()],
                    required_tests: vec!["cargo test".into()],
                    validation: vec!["cargo test --lib".into()],
                    execution_hints: crate::protocol::ExecutionHints::default(),
                    risk_factors: Vec::new(),
                }]
            } else {
                Vec::new()
            };
            let mut runtime = FakeRuntime {
                value: output(decision, tasks),
                requests: 0,
            };
            let result = ControllerIntakeBuilder::new()
                .classify(&request(), &mut runtime)
                .unwrap();
            assert_eq!(
                format!("{:?}", result.decision).to_ascii_lowercase(),
                decision.replace('_', "")
            );
            assert_eq!(runtime.requests, 1);
        }
    }

    #[test]
    fn malformed_output_is_rejected_closed() {
        let mut runtime = FakeRuntime {
            value: serde_json::json!({
                "decision": "plan_required",
                "details": "bounded",
                "direct_tasks": [],
                "unsupported": true
            }),
            requests: 0,
        };
        assert!(matches!(
            ControllerIntakeBuilder::new().classify(&request(), &mut runtime),
            Err(ControllerIntakeError::InvalidStructuredOutput(_))
        ));
    }

    #[test]
    fn malformed_nested_task_fields_are_rejected_closed() {
        let mut task = serde_json::to_value(TaskProposal {
            local_id: "inspect".into(),
            title: "Inspect project".into(),
            objective: "Inspect the project state.".into(),
            role: "developer".into(),
            priority: crate::task::TaskPriority::Normal,
            depends_on: Vec::new(),
            capabilities: Vec::new(),
            scope_mode: None,
            context_files: Vec::new(),
            expected_changes: vec!["inspection result".into()],
            unchanged: vec!["project behavior".into()],
            acceptance_criteria: vec!["The state is documented.".into()],
            required_tests: vec!["cargo test".into()],
            validation: vec!["cargo test --lib".into()],
            execution_hints: crate::protocol::ExecutionHints::default(),
            risk_factors: Vec::new(),
        })
        .unwrap();
        task.as_object_mut()
            .unwrap()
            .insert("untrusted_route".into(), Value::String("apply".into()));
        let mut runtime = FakeRuntime {
            value: serde_json::json!({
                "decision": "direct_tasks",
                "details": "bounded",
                "direct_tasks": [task]
            }),
            requests: 0,
        };
        assert!(matches!(
            ControllerIntakeBuilder::new().classify(&request(), &mut runtime),
            Err(ControllerIntakeError::InvalidStructuredOutput(_))
        ));
    }
}
