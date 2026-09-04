//! Read-only Controller planning boundary.
//!
//! This module projects the canonical planning protocol into a bounded,
//! model-independent request and returns a typed proposed [`PlanResponse`].
//! It deliberately has no database, Lead, task-application, workflow, or
//! provider-specific seam. Durable Planner execution remains in
//! [`crate::automated::run_plan`].

use crate::controller_memory::ControllerMemoryContext;
use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::protocol::{
    PlanResponse, PlanResponseSchema, PlanningProjectState, PlanningRequest, TaskProposal,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTROLLER_PLANNING_REQUEST_VERSION: u32 = 1;
pub const MAX_CONTROLLER_PLANNING_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_PLANNING_RESULT_BYTES: usize = 64 * 1024;

const MAX_TEXT_BYTES: usize = 2048;
const MAX_KIND_BYTES: usize = 256;
const MAX_LIST_ITEMS: usize = 16;
const MAX_STATE_ITEMS: usize = 32;
const MAX_PLAN_TASKS: usize = 16;
const MAX_RATIONALE_BYTES: usize = 1024;
const MAX_UNCERTAINTY_BYTES: usize = 1024;

/// Failures at the bounded read-only Controller planning boundary.
#[derive(Debug, Error)]
pub enum ControllerPlanningError {
    #[error("canonical planning request is invalid: {0}")]
    InvalidCanonicalRequest(String),
    #[error("controller planning request is invalid: {0}")]
    InvalidRequest(String),
    #[error("controller planning request serialization failed: {0}")]
    Serialization(String),
    #[error("controller planning memory context failed: {0}")]
    MemoryContext(String),
    #[error("controller planning request is {actual} bytes; maximum is {max}")]
    RequestTooLarge { actual: usize, max: usize },
    #[error("controller planning output is malformed: {0}")]
    InvalidStructuredOutput(String),
    #[error("controller planning inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
    #[error("controller planning output is not a valid PlanResponse: {0}")]
    InvalidPlan(String),
}

/// A bounded task summary copied from canonical [`PlanningProjectState`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanningTask {
    pub id: String,
    pub title: String,
    pub status: String,
}

impl ControllerPlanningTask {
    fn from_canonical(task: &crate::protocol::TaskSummary) -> Self {
        Self {
            id: bound_text(&task.id, MAX_TEXT_BYTES),
            title: bound_text(&task.title, MAX_TEXT_BYTES),
            status: bound_text(&task.status, MAX_TEXT_BYTES),
        }
    }
}

/// A bounded project-state projection for Controller planning.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanningState {
    pub task_counts: Vec<ControllerPlanningCount>,
    pub ready_tasks: Vec<ControllerPlanningTask>,
    pub active_tasks: Vec<ControllerPlanningTask>,
    pub review_tasks: Vec<ControllerPlanningTask>,
    pub blocked_tasks: Vec<ControllerPlanningTask>,
    pub usable_agents: Vec<String>,
    pub busy_agents: Vec<String>,
    pub quota_reserve_percent: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanningCount {
    pub status: String,
    pub count: usize,
}

impl ControllerPlanningState {
    fn from_canonical(state: &PlanningProjectState) -> Self {
        Self {
            task_counts: state
                .task_counts
                .iter()
                .take(MAX_STATE_ITEMS)
                .map(|(status, count)| ControllerPlanningCount {
                    status: bound_text(status, MAX_TEXT_BYTES),
                    count: *count,
                })
                .collect(),
            ready_tasks: bound_state_tasks(&state.ready_tasks),
            active_tasks: bound_state_tasks(&state.active_tasks),
            review_tasks: bound_state_tasks(&state.review_tasks),
            blocked_tasks: bound_state_tasks(&state.blocked_tasks),
            usable_agents: bound_strings(&state.usable_agents, MAX_STATE_ITEMS),
            busy_agents: bound_strings(&state.busy_agents, MAX_STATE_ITEMS),
            quota_reserve_percent: state.quota_reserve_percent,
        }
    }
}

fn bound_state_tasks(tasks: &[crate::protocol::TaskSummary]) -> Vec<ControllerPlanningTask> {
    tasks
        .iter()
        .take(MAX_STATE_ITEMS)
        .map(ControllerPlanningTask::from_canonical)
        .collect()
}

/// Controller-owned bounded planning input. It contains canonical planning
/// facts, but no repository path, discovery snapshot, full report, Lead
/// decision, provider configuration, or runtime object.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanningRequest {
    pub packet_version: u32,
    pub kind: String,
    pub project_name: Option<String>,
    pub engineering_contract: String,
    pub objective: String,
    pub constraints: Vec<String>,
    pub target_platforms: Vec<String>,
    pub stack: Vec<String>,
    pub non_goals: Vec<String>,
    pub deliverables: Vec<String>,
    pub definition_of_done: Vec<String>,
    pub response_schema: PlanResponseSchema,
    pub role_boundaries: Vec<String>,
    pub planning_constraints: Vec<String>,
    pub approval_requirements: Vec<String>,
    pub current_state: Option<ControllerPlanningState>,
}

/// Capability-local inference input for Controller Plan generation. Keeping
/// the current request and reusable memory context as separate typed fields
/// preserves their authority boundary and avoids changing other Controller
/// capability request/state types.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanningInput {
    pub current_request: ControllerPlanningRequest,
    pub memory: ControllerMemoryContext,
}

impl ControllerPlanningInput {
    pub fn from_canonical(
        request: &PlanningRequest,
        memory: ControllerMemoryContext,
    ) -> Result<Self, ControllerPlanningError> {
        let input = Self {
            current_request: ControllerPlanningRequest::from_canonical(request)?,
            memory,
        };
        input.validate()?;
        Ok(input)
    }

    pub fn validate(&self) -> Result<(), ControllerPlanningError> {
        self.current_request.validate()?;
        self.memory
            .validate()
            .map_err(|error| ControllerPlanningError::MemoryContext(error.to_string()))?;
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerPlanningError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_PLANNING_REQUEST_BYTES {
            return Err(ControllerPlanningError::RequestTooLarge {
                actual,
                max: MAX_CONTROLLER_PLANNING_REQUEST_BYTES,
            });
        }
        Ok(())
    }
}

impl ControllerPlanningRequest {
    /// Construct the bounded Controller request from canonical planning data.
    /// Full reports and discovery snapshots are intentionally not copied.
    pub fn from_canonical(request: &PlanningRequest) -> Result<Self, ControllerPlanningError> {
        request
            .validate()
            .map_err(|error| ControllerPlanningError::InvalidCanonicalRequest(error.to_string()))?;
        let canonical_schema = PlanResponseSchema::v1();
        if !is_canonical_schema(&request.response_schema) {
            return Err(ControllerPlanningError::InvalidCanonicalRequest(
                "unsupported PlanResponse schema".into(),
            ));
        }
        let bounded = Self {
            packet_version: CONTROLLER_PLANNING_REQUEST_VERSION,
            kind: bound_text(&request.kind, MAX_KIND_BYTES),
            project_name: request
                .project
                .as_ref()
                .map(|project| bound_text(&project.name, MAX_TEXT_BYTES)),
            engineering_contract: bound_text(&request.engineering_contract, MAX_TEXT_BYTES),
            objective: bound_text(&request.objective, MAX_TEXT_BYTES),
            constraints: bound_strings(&request.constraints, MAX_LIST_ITEMS),
            target_platforms: bound_strings(&request.target_platforms, MAX_LIST_ITEMS),
            stack: bound_strings(&request.stack, MAX_LIST_ITEMS),
            non_goals: bound_strings(&request.non_goals, MAX_LIST_ITEMS),
            deliverables: bound_strings(&request.deliverables, MAX_LIST_ITEMS),
            definition_of_done: bound_strings(&request.definition_of_done, MAX_LIST_ITEMS),
            response_schema: canonical_schema,
            role_boundaries: bound_strings(&request.role_boundaries, MAX_LIST_ITEMS),
            planning_constraints: bound_strings(&request.planning_constraints, MAX_LIST_ITEMS),
            approval_requirements: bound_strings(&request.approval_requirements, MAX_LIST_ITEMS),
            current_state: request
                .current_state
                .as_ref()
                .map(ControllerPlanningState::from_canonical),
        };
        bounded.validate()?;
        Ok(bounded)
    }

    pub fn validate(&self) -> Result<(), ControllerPlanningError> {
        if self.packet_version != CONTROLLER_PLANNING_REQUEST_VERSION {
            return Err(ControllerPlanningError::InvalidRequest(
                "unsupported Controller planning request version".into(),
            ));
        }
        if self.objective.trim().is_empty() {
            return Err(ControllerPlanningError::InvalidRequest(
                "objective must not be empty".into(),
            ));
        }
        if !is_canonical_schema(&self.response_schema) {
            return Err(ControllerPlanningError::InvalidRequest(
                "unsupported PlanResponse schema".into(),
            ));
        }
        validate_text(&self.kind, MAX_KIND_BYTES, "kind")?;
        if let Some(project_name) = &self.project_name {
            validate_text(project_name, MAX_TEXT_BYTES, "project_name")?;
        }
        for (name, value) in [
            ("engineering_contract", &self.engineering_contract),
            ("objective", &self.objective),
        ] {
            validate_text(value, MAX_TEXT_BYTES, name)?;
        }
        for (name, values) in [
            ("constraints", &self.constraints),
            ("target_platforms", &self.target_platforms),
            ("stack", &self.stack),
            ("non_goals", &self.non_goals),
            ("deliverables", &self.deliverables),
            ("definition_of_done", &self.definition_of_done),
            ("role_boundaries", &self.role_boundaries),
            ("planning_constraints", &self.planning_constraints),
            ("approval_requirements", &self.approval_requirements),
        ] {
            validate_strings(values, name, MAX_LIST_ITEMS, MAX_TEXT_BYTES)?;
        }
        if let Some(state) = &self.current_state {
            validate_state(state)?;
        }
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerPlanningError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_PLANNING_REQUEST_BYTES {
            return Err(ControllerPlanningError::RequestTooLarge {
                actual,
                max: MAX_CONTROLLER_PLANNING_REQUEST_BYTES,
            });
        }
        Ok(())
    }
}

fn is_canonical_schema(schema: &PlanResponseSchema) -> bool {
    let canonical = PlanResponseSchema::v1();
    schema.name == canonical.name
        && schema.protocol_version == canonical.protocol_version
        && schema.fields == canonical.fields
        && schema.task_fields == canonical.task_fields
}

fn validate_state(state: &ControllerPlanningState) -> Result<(), ControllerPlanningError> {
    if state.task_counts.len() > MAX_STATE_ITEMS {
        return Err(ControllerPlanningError::InvalidRequest(
            "too many task-count entries".into(),
        ));
    }
    for count in &state.task_counts {
        validate_text(
            &count.status,
            MAX_TEXT_BYTES,
            "current_state.task_counts.status",
        )?;
    }
    for (name, tasks) in [
        ("ready_tasks", &state.ready_tasks),
        ("active_tasks", &state.active_tasks),
        ("review_tasks", &state.review_tasks),
        ("blocked_tasks", &state.blocked_tasks),
    ] {
        if tasks.len() > MAX_STATE_ITEMS {
            return Err(ControllerPlanningError::InvalidRequest(format!(
                "too many current_state.{name}"
            )));
        }
        for task in tasks {
            validate_text(&task.id, MAX_TEXT_BYTES, "current_state.task.id")?;
            validate_text(&task.title, MAX_TEXT_BYTES, "current_state.task.title")?;
            validate_text(&task.status, MAX_TEXT_BYTES, "current_state.task.status")?;
        }
    }
    validate_strings(
        &state.usable_agents,
        "current_state.usable_agents",
        MAX_STATE_ITEMS,
        MAX_TEXT_BYTES,
    )?;
    validate_strings(
        &state.busy_agents,
        "current_state.busy_agents",
        MAX_STATE_ITEMS,
        MAX_TEXT_BYTES,
    )?;
    Ok(())
}

/// Typed proposed plan returned by the read-only Controller capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanResult {
    pub plan: PlanResponse,
    pub rationale: String,
    pub uncertainty: Option<String>,
}

impl ControllerPlanResult {
    pub fn validate(&self) -> Result<(), ControllerPlanningError> {
        validate_controller_plan(&self.plan)?;
        output_check(validate_text(
            &self.rationale,
            MAX_RATIONALE_BYTES,
            "rationale",
        ))?;
        if self.rationale.trim().is_empty() {
            return Err(ControllerPlanningError::InvalidStructuredOutput(
                "rationale must not be empty".into(),
            ));
        }
        if let Some(uncertainty) = &self.uncertainty {
            output_check(validate_text(
                uncertainty,
                MAX_UNCERTAINTY_BYTES,
                "uncertainty",
            ))?;
        }
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerPlanningError::Serialization(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_PLANNING_RESULT_BYTES {
            return Err(ControllerPlanningError::InvalidStructuredOutput(
                "structured planning result exceeds its bound".into(),
            ));
        }
        Ok(())
    }
}

fn validate_controller_plan(plan: &PlanResponse) -> Result<(), ControllerPlanningError> {
    plan.validate()
        .map_err(|error| ControllerPlanningError::InvalidPlan(error.to_string()))?;
    if plan.tasks.len() > MAX_PLAN_TASKS {
        return Err(ControllerPlanningError::InvalidPlan(
            "too many proposed tasks".into(),
        ));
    }
    output_check(validate_strings(
        &plan.assumptions,
        "plan.assumptions",
        MAX_LIST_ITEMS,
        MAX_TEXT_BYTES,
    ))?;
    output_check(validate_strings(
        &plan.risks,
        "plan.risks",
        MAX_LIST_ITEMS,
        MAX_TEXT_BYTES,
    ))?;
    output_check(validate_strings(
        &plan.questions,
        "plan.questions",
        MAX_LIST_ITEMS,
        MAX_TEXT_BYTES,
    ))?;
    output_check(validate_text(
        &plan.objective,
        MAX_TEXT_BYTES,
        "plan.objective",
    ))?;
    for task in &plan.tasks {
        output_check(validate_task(task))?;
    }
    Ok(())
}

fn output_check(
    result: Result<(), ControllerPlanningError>,
) -> Result<(), ControllerPlanningError> {
    result.map_err(|error| match error {
        ControllerPlanningError::InvalidRequest(message) => {
            ControllerPlanningError::InvalidStructuredOutput(message)
        }
        other => other,
    })
}

fn validate_task(task: &TaskProposal) -> Result<(), ControllerPlanningError> {
    validate_text(&task.local_id, MAX_TEXT_BYTES, "task.local_id")?;
    validate_text(&task.title, MAX_TEXT_BYTES, "task.title")?;
    validate_text(&task.objective, MAX_TEXT_BYTES, "task.objective")?;
    validate_text(&task.role, MAX_TEXT_BYTES, "task.role")?;
    for (name, values) in [
        ("depends_on", &task.depends_on),
        ("capabilities", &task.capabilities),
        ("context_files", &task.context_files),
        ("expected_changes", &task.expected_changes),
        ("unchanged", &task.unchanged),
        ("acceptance_criteria", &task.acceptance_criteria),
        ("required_tests", &task.required_tests),
        ("validation", &task.validation),
    ] {
        validate_strings(values, name, MAX_LIST_ITEMS, MAX_TEXT_BYTES)?;
    }
    if let Some(model) = &task.execution_hints.model {
        validate_text(model, MAX_TEXT_BYTES, "task.execution_hints.model")?;
    }
    if let Some(class) = &task.execution_hints.class {
        validate_text(class, MAX_TEXT_BYTES, "task.execution_hints.class")?;
    }
    if let Some(effort) = &task.execution_hints.effort {
        validate_text(effort, MAX_TEXT_BYTES, "task.execution_hints.effort")?;
    }
    if let Some(reason) = &task.execution_hints.effort_reason {
        validate_text(reason, 240, "task.execution_hints.effort_reason")?;
    }
    Ok(())
}

fn validate_strings(
    values: &[String],
    field: &str,
    max_items: usize,
    max_bytes: usize,
) -> Result<(), ControllerPlanningError> {
    if values.len() > max_items {
        return Err(ControllerPlanningError::InvalidRequest(format!(
            "{field} exceeds its item bound"
        )));
    }
    for value in values {
        validate_text(value, max_bytes, field)?;
    }
    Ok(())
}

fn validate_text(
    value: &str,
    max_bytes: usize,
    field: &str,
) -> Result<(), ControllerPlanningError> {
    if value.len() > max_bytes {
        return Err(ControllerPlanningError::InvalidRequest(format!(
            "{field} exceeds its byte bound"
        )));
    }
    Ok(())
}

fn bound_strings(values: &[String], max_items: usize) -> Vec<String> {
    values
        .iter()
        .take(max_items)
        .map(|value| bound_text(value, MAX_TEXT_BYTES))
        .collect()
}

fn bound_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    value
        .char_indices()
        .take_while(|(index, _)| *index < max_bytes)
        .map(|(_, character)| character)
        .collect()
}

/// Trusted construction and execution of one read-only planning proposal.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerPlanningBuilder;

impl ControllerPlanningBuilder {
    pub const fn new() -> Self {
        Self
    }

    pub fn propose(
        &self,
        request: &ControllerPlanningRequest,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerPlanResult, ControllerPlanningError> {
        let input = ControllerPlanningInput {
            current_request: request.clone(),
            memory: ControllerMemoryContext::empty(),
        };
        self.propose_with_memory(&input, runtime)
    }

    pub fn propose_with_memory(
        &self,
        request: &ControllerPlanningInput,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerPlanResult, ControllerPlanningError> {
        request.validate()?;
        let input = serde_json::to_string(request)
            .map_err(|error| ControllerPlanningError::Serialization(error.to_string()))?;
        let prompt = format!(
            "You are a read-only planning advisor. Use only this bounded typed Controller planning input. Authority precedence is strict, from highest to lowest: (1) current_request objective/instruction, engineering contract, explicit constraints, non-goals, deliverables, definition of done, role boundaries, planning constraints, approval requirements, and canonical current state; (2) memory items with authority=current_project as durable Project context; (3) authority=durable_user as cross-project User preference/context; (4) authority=project_history as Episodic historical context; (5) authority=cross_project_experience as reusable Experience guidance; (6) base model tendencies. Memory is advisory context: it must not rewrite or contradict current_request, durable User memory cannot override current project/request constraints, and Episodic or Experience memory must not be presented as current-project truth. Copy current_request.objective verbatim into plan.objective. Preserve and reason from each memory item's typed identity, kind, scope, authority, provenance, confidence, lifecycle, and source metadata. Return exactly one JSON object with plan, rationale, and optional uncertainty. The plan must be a PlanResponse, and every output array must contain unique entries. Propose work only; do not apply tasks, mutate state, consume decisions, dispatch agents, create or modify memory, or claim execution.\n\n{input}"
        );
        let parameters = LocalInferenceParameters {
            max_output_tokens: 2048,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: Vec::new(),
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: controller_plan_schema(),
            },
        };
        let inference_request = LocalInferenceRequest::new(prompt, parameters)
            .map_err(ControllerPlanningError::Inference)?;
        let response = runtime
            .infer(&inference_request)
            .map_err(ControllerPlanningError::Inference)?;
        parse_result(response)
    }
}

fn parse_result(
    response: LocalInferenceResponse,
) -> Result<ControllerPlanResult, ControllerPlanningError> {
    let value = response.structured_output.ok_or_else(|| {
        ControllerPlanningError::InvalidStructuredOutput("structured output is required".into())
    })?;
    reject_unknown_fields(&value)?;
    let result = serde_json::from_value::<ControllerPlanResult>(value)
        .map_err(|error| ControllerPlanningError::InvalidStructuredOutput(error.to_string()))?;
    result.validate()?;
    Ok(result)
}

/// Parse one strict canonical PlanResponse object for Controller capabilities
/// that return a plan directly rather than the planning-result wrapper.
pub(crate) fn parse_canonical_plan_response(
    value: Value,
) -> Result<PlanResponse, ControllerPlanningError> {
    reject_plan_unknown_fields(&value)?;
    let plan = serde_json::from_value::<PlanResponse>(value)
        .map_err(|error| ControllerPlanningError::InvalidStructuredOutput(error.to_string()))?;
    validate_controller_plan(&plan).map_err(|error| match error {
        ControllerPlanningError::InvalidPlan(message) => {
            ControllerPlanningError::InvalidPlan(message)
        }
        ControllerPlanningError::InvalidStructuredOutput(message) => {
            ControllerPlanningError::InvalidStructuredOutput(message)
        }
        other => ControllerPlanningError::InvalidStructuredOutput(other.to_string()),
    })?;
    Ok(plan)
}

fn reject_unknown_fields(value: &Value) -> Result<(), ControllerPlanningError> {
    expect_keys(value, &["plan", "rationale", "uncertainty"], "result")?;
    let plan = value
        .get("plan")
        .ok_or_else(|| ControllerPlanningError::InvalidStructuredOutput("missing plan".into()))?;
    reject_plan_unknown_fields(plan)
}

fn reject_plan_unknown_fields(value: &Value) -> Result<(), ControllerPlanningError> {
    expect_keys(
        value,
        &[
            "protocol_version",
            "objective",
            "assumptions",
            "risks",
            "questions",
            "tasks",
        ],
        "plan",
    )?;
    let tasks = value
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ControllerPlanningError::InvalidStructuredOutput("tasks must be an array".into())
        })?;
    for task in tasks {
        expect_keys(
            task,
            &[
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
            ],
            "plan.tasks[]",
        )?;
        let hints = task.get("execution_hints").ok_or_else(|| {
            ControllerPlanningError::InvalidStructuredOutput("missing task.execution_hints".into())
        })?;
        expect_keys(
            hints,
            &["class", "model", "effort", "effort_reason"],
            "plan.tasks[].execution_hints",
        )?;
    }
    Ok(())
}

fn expect_keys(
    value: &Value,
    expected: &[&str],
    field: &str,
) -> Result<(), ControllerPlanningError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerPlanningError::InvalidStructuredOutput(format!("{field} must be an object"))
    })?;
    if object.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err(ControllerPlanningError::InvalidStructuredOutput(format!(
            "{field} contains an unsupported field"
        )));
    }
    Ok(())
}

/// Strict JSON schema for the Controller wrapper around the canonical plan
/// schema. The nested plan schema is the existing Planner schema.
pub fn controller_plan_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "plan": crate::automated::canonical_plan_response_schema(),
            "rationale": {"type": "string", "minLength": 1, "maxLength": MAX_RATIONALE_BYTES},
            "uncertainty": {"type": ["string", "null"], "maxLength": MAX_UNCERTAINTY_BYTES}
        },
        "required": ["plan", "rationale", "uncertainty"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_memory::{
        ControllerMemoryAuthority, ControllerMemoryItem, MAX_CONTROLLER_MEMORY_CONTENT_BYTES,
        MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND,
    };
    use crate::memory::{
        MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
        MemoryScope,
    };
    use crate::protocol::{PROTOCOL_VERSION, PlanResponseSchema};

    struct FakeRuntime {
        response: LocalInferenceResponse,
        requests: Vec<LocalInferenceRequest>,
    }

    impl FakeRuntime {
        fn new(response: LocalInferenceResponse) -> Self {
            Self {
                response,
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

    fn request() -> PlanningRequest {
        PlanningRequest {
            protocol_version: PROTOCOL_VERSION,
            kind: "project_plan".into(),
            project: Some(crate::protocol::ReportProject {
                name: "controller-plan-test".into(),
                repository: "/private/repository/path".into(),
                branch: Some("main".into()),
                commit: Some("abc".into()),
            }),
            engineering_contract: "Keep planning read-only.".into(),
            objective: "Plan one bounded change.".into(),
            constraints: vec!["Do not mutate state.".into()],
            target_platforms: vec![],
            stack: vec!["Rust".into()],
            non_goals: vec!["Applying the plan".into()],
            deliverables: vec!["A PlanResponse".into()],
            definition_of_done: vec!["The plan is reviewable.".into()],
            response_schema: PlanResponseSchema::v1(),
            role_boundaries: vec!["Controller proposes only.".into()],
            planning_constraints: vec![],
            approval_requirements: vec!["Operator approval is required.".into()],
            current_state: Some(PlanningProjectState {
                task_counts: [("ready".into(), 1)].into_iter().collect(),
                ready_tasks: vec![crate::protocol::TaskSummary {
                    id: "T-1".into(),
                    title: "Existing".into(),
                    status: "ready".into(),
                }],
                active_tasks: vec![],
                review_tasks: vec![],
                blocked_tasks: vec![],
                usable_agents: vec!["agent-a".into()],
                busy_agents: vec![],
                quota_reserve_percent: 10,
            }),
            full_report: None,
            discovery_snapshot: None,
        }
    }

    fn plan_value() -> Value {
        serde_json::json!({
            "protocol_version": 1,
            "objective": "Plan one bounded change.",
            "assumptions": [],
            "risks": [],
            "questions": [],
            "tasks": [{
                "local_id": "one",
                "title": "One change",
                "objective": "Implement one bounded change.",
                "role": "developer",
                "priority": "normal",
                "depends_on": [],
                "capabilities": [],
                "scope_mode": null,
                "context_files": [],
                "expected_changes": ["one change"],
                "unchanged": ["other behavior"],
                "acceptance_criteria": ["It is reviewable."],
                "required_tests": ["focused test"],
                "validation": ["cargo test"],
                "execution_hints": {"class": null, "model": null, "effort": "low", "effort_reason": "isolated"},
                "risk_factors": []
            }]
        })
    }

    fn response() -> LocalInferenceResponse {
        LocalInferenceResponse::structured(
            "ignored provider text",
            serde_json::json!({
                "plan": plan_value(),
                "rationale": "The proposal is bounded.",
                "uncertainty": null
            }),
        )
    }

    fn memory_item(
        id: MemoryId,
        kind: MemoryKind,
        scope: MemoryScope,
        authority: ControllerMemoryAuthority,
        subject: &str,
        content: &str,
        provenance_kind: MemoryProvenanceKind,
    ) -> ControllerMemoryItem {
        ControllerMemoryItem {
            id,
            kind,
            scope,
            authority,
            subject: subject.into(),
            content: content.into(),
            provenance: MemoryProvenance {
                kind: provenance_kind,
                source_reference: Some("workflow:memory-source".into()),
            },
            confidence: Some(0.8),
            lifecycle: MemoryLifecycle::Active,
            supersedes: None,
        }
    }

    fn memory_context() -> ControllerMemoryContext {
        let context = ControllerMemoryContext {
            context_version: crate::controller_memory::CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: vec![
                memory_item(
                    MemoryId::Project {
                        project_id: 1,
                        id: 1,
                    },
                    MemoryKind::Project,
                    MemoryScope::Project { project_id: 1 },
                    ControllerMemoryAuthority::CurrentProject,
                    "http-layout",
                    "Health routes live in src/http.rs.",
                    MemoryProvenanceKind::ProjectFact,
                ),
                memory_item(
                    MemoryId::Global(1),
                    MemoryKind::User,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::DurableUser,
                    "language-preference",
                    "Prefer concise Rust changes.",
                    MemoryProvenanceKind::Operator,
                ),
                memory_item(
                    MemoryId::Project {
                        project_id: 1,
                        id: 2,
                    },
                    MemoryKind::Episodic,
                    MemoryScope::Project { project_id: 1 },
                    ControllerMemoryAuthority::ProjectHistory,
                    "prior-release",
                    "A prior release needed an extra migration check.",
                    MemoryProvenanceKind::Imported,
                ),
                memory_item(
                    MemoryId::Global(2),
                    MemoryKind::Experience,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "review-guidance",
                    "Prefer focused validation before broad validation.",
                    MemoryProvenanceKind::ControllerApproved,
                ),
            ],
        };
        context.validate().unwrap();
        context
    }

    #[test]
    fn successful_proposal_returns_canonical_typed_plan() {
        let bounded = ControllerPlanningRequest::from_canonical(&request()).unwrap();
        let mut runtime = FakeRuntime::new(response());
        let result = ControllerPlanningBuilder::new()
            .propose(&bounded, &mut runtime)
            .unwrap();
        assert_eq!(result.plan.tasks[0].local_id, "one");
        assert_eq!(result.rationale, "The proposal is bounded.");
        assert_eq!(runtime.requests.len(), 1);
        assert!(runtime.requests[0].prompt.contains("controller-plan-test"));
        assert!(
            !runtime.requests[0]
                .prompt
                .contains("/private/repository/path")
        );
        assert!(runtime.requests[0].prompt.len() < MAX_CONTROLLER_PLANNING_REQUEST_BYTES);
        assert!(
            runtime.requests[0]
                .prompt
                .contains("\"memory\":{\"context_version\":1,\"items\":[]}")
        );
    }

    #[test]
    fn planning_input_preserves_typed_memory_and_prompt_precedence() {
        let input = ControllerPlanningInput::from_canonical(&request(), memory_context()).unwrap();
        let serialized = serde_json::to_value(&input).unwrap();
        assert_eq!(
            serialized["current_request"]["objective"],
            "Plan one bounded change."
        );
        assert_eq!(serialized["memory"]["items"][0]["kind"], "project");
        assert_eq!(
            serialized["memory"]["items"][0]["scope"]["Project"]["project_id"],
            1
        );
        assert_eq!(
            serialized["memory"]["items"][0]["authority"],
            "current_project"
        );
        assert_eq!(
            serialized["memory"]["items"][0]["provenance"]["kind"],
            "project_fact"
        );
        assert_eq!(
            serialized["memory"]["items"][0]["provenance"]["source_reference"],
            "workflow:memory-source"
        );
        assert_eq!(serialized["memory"]["items"][0]["confidence"], 0.8);

        let mut runtime = FakeRuntime::new(response());
        ControllerPlanningBuilder::new()
            .propose_with_memory(&input, &mut runtime)
            .unwrap();
        let prompt = &runtime.requests[0].prompt;
        assert!(prompt.contains("Authority precedence is strict"));
        assert!(prompt.contains("current_request objective/instruction"));
        assert!(prompt.contains("authority=current_project"));
        assert!(prompt.contains("authority=durable_user"));
        assert!(prompt.contains("authority=project_history"));
        assert!(prompt.contains("authority=cross_project_experience"));
        assert!(prompt.contains("must not rewrite or contradict current_request"));
        assert!(prompt.contains("Copy current_request.objective verbatim"));
        assert!(prompt.contains("every output array must contain unique entries"));
        assert!(prompt.contains("\"subject\":\"http-layout\""));
    }

    #[test]
    fn combined_request_bound_includes_memory_context() {
        let mut canonical = request();
        canonical.constraints = vec!["c".repeat(MAX_TEXT_BYTES); MAX_LIST_ITEMS];
        canonical.deliverables = vec!["d".repeat(1000); 4];
        let current_request = ControllerPlanningRequest::from_canonical(&canonical).unwrap();
        let memory = ControllerMemoryContext {
            context_version: crate::controller_memory::CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: (0..MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND)
                .map(|index| {
                    memory_item(
                        MemoryId::Project {
                            project_id: 1,
                            id: index as i64 + 1,
                        },
                        MemoryKind::Project,
                        MemoryScope::Project { project_id: 1 },
                        ControllerMemoryAuthority::CurrentProject,
                        &format!("large-{index}"),
                        &"m".repeat(MAX_CONTROLLER_MEMORY_CONTENT_BYTES - 896),
                        MemoryProvenanceKind::ProjectFact,
                    )
                })
                .collect(),
        };
        current_request.validate().unwrap();
        memory.validate().unwrap();
        let combined = ControllerPlanningInput {
            current_request,
            memory,
        };
        assert!(
            serde_json::to_vec(&combined).unwrap().len() > MAX_CONTROLLER_PLANNING_REQUEST_BYTES
        );
        assert!(matches!(
            combined.validate(),
            Err(ControllerPlanningError::RequestTooLarge {
                max: MAX_CONTROLLER_PLANNING_REQUEST_BYTES,
                ..
            })
        ));
    }

    #[test]
    fn malformed_or_unsupported_structured_output_is_rejected() {
        let bounded = ControllerPlanningRequest::from_canonical(&request()).unwrap();
        let mut malformed = FakeRuntime::new(LocalInferenceResponse::structured(
            "",
            serde_json::json!({"plan": plan_value(), "rationale": "ok", "uncertainty": null, "extra": true}),
        ));
        assert!(matches!(
            ControllerPlanningBuilder::new().propose(&bounded, &mut malformed),
            Err(ControllerPlanningError::InvalidStructuredOutput(_))
        ));

        let invalid_plan = serde_json::json!({
            "plan": {"protocol_version": 99, "objective": "x", "assumptions": [], "risks": [], "questions": [], "tasks": []},
            "rationale": "ok",
            "uncertainty": null
        });
        let mut invalid = FakeRuntime::new(LocalInferenceResponse::structured("", invalid_plan));
        assert!(matches!(
            ControllerPlanningBuilder::new().propose(&bounded, &mut invalid),
            Err(ControllerPlanningError::InvalidPlan(_))
        ));
    }

    #[test]
    fn canonical_projection_bounds_input_without_copying_report_or_discovery() {
        let mut canonical = request();
        canonical.engineering_contract = "x".repeat(MAX_TEXT_BYTES * 2);
        canonical.constraints = (0..MAX_LIST_ITEMS + 4)
            .map(|index| format!("constraint-{index}"))
            .collect();
        canonical.current_state.as_mut().unwrap().usable_agents = (0..MAX_STATE_ITEMS + 4)
            .map(|index| format!("agent-{index}"))
            .collect();
        canonical.full_report = Some(crate::protocol::ProjectReport {
            protocol_version: 1,
            project: canonical.project.clone().unwrap(),
            engineering_contract: "should not be copied".into(),
            architecture: Default::default(),
            lifecycle: crate::protocol::ReportLifecycle {
                counts: Default::default(),
                tasks: vec![],
            },
            agents: vec![],
            queue: Default::default(),
            recent_work: vec![],
            risks: vec![],
            open_questions: vec![],
            role_boundaries: vec![],
            planning_constraints: vec![],
            approval_requirements: vec![],
        });
        let bounded = ControllerPlanningRequest::from_canonical(&canonical).unwrap();
        assert_eq!(bounded.constraints.len(), MAX_LIST_ITEMS);
        assert_eq!(bounded.engineering_contract.len(), MAX_TEXT_BYTES);
        assert_eq!(
            bounded.current_state.as_ref().unwrap().usable_agents.len(),
            MAX_STATE_ITEMS
        );
        let serialized = serde_json::to_vec(&bounded).unwrap();
        assert!(serialized.len() <= MAX_CONTROLLER_PLANNING_REQUEST_BYTES);
        assert!(
            !String::from_utf8(serialized)
                .unwrap()
                .contains("should not be copied")
        );
    }

    #[test]
    fn schema_reuses_canonical_plan_schema() {
        let schema = controller_plan_schema();
        assert!(schema.to_string().contains("expected_changes"));
        assert!(schema["properties"].get("memory").is_none());
    }

    #[test]
    fn app_api_proposes_without_persisting_planning_state() {
        let directory = tempfile::tempdir().unwrap();
        let database = crate::storage::Database::init(directory.path().join("orc.db")).unwrap();
        let project_id = database.create_project("read-only-planning").unwrap();
        let memory = database
            .create_memory(&MemoryDraft {
                kind: MemoryKind::Project,
                scope: MemoryScope::Project { project_id },
                subject: "planning-layout".into(),
                content: "Planning code lives in src/controller_planning.rs.".into(),
                provenance: MemoryProvenance {
                    kind: MemoryProvenanceKind::ProjectFact,
                    source_reference: Some("project:fact:planning-layout".into()),
                },
                confidence: Some(1.0),
            })
            .unwrap();
        let before_memory = database.memory_history(&memory.id).unwrap();
        let before = serde_json::to_value(database.planning_project_state().unwrap()).unwrap();
        let before_decisions = database.list_lead_decisions(project_id).unwrap();
        let app =
            crate::app::OrcApp::open(directory.path().join("orc.db"), directory.path()).unwrap();
        let mut runtime = FakeRuntime::new(response());
        let result = app
            .propose_controller_plan(&request(), &mut runtime)
            .unwrap();
        assert_eq!(result.plan.tasks.len(), 1);
        assert!(runtime.requests[0].prompt.contains("planning-layout"));
        assert!(runtime.requests[0].prompt.contains("current_project"));
        let after_database =
            crate::storage::Database::open(directory.path().join("orc.db")).unwrap();
        assert_eq!(
            before,
            serde_json::to_value(after_database.planning_project_state().unwrap()).unwrap()
        );
        assert_eq!(
            before_decisions,
            after_database.list_lead_decisions(project_id).unwrap()
        );
        assert!(
            after_database
                .list_plan_history(project_id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            after_database.memory_history(&memory.id).unwrap(),
            before_memory
        );
    }

    #[test]
    fn app_api_preserves_empty_memory_compatibility() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("orc.db");
        let database = crate::storage::Database::init(&path).unwrap();
        database.create_project("empty-memory-planning").unwrap();
        drop(database);
        let app = crate::app::OrcApp::open(&path, directory.path()).unwrap();
        let mut runtime = FakeRuntime::new(response());
        let result = app
            .propose_controller_plan(&request(), &mut runtime)
            .unwrap();
        assert_eq!(result.plan.tasks.len(), 1);
        assert!(
            runtime.requests[0]
                .prompt
                .contains("\"memory\":{\"context_version\":1,\"items\":[]}")
        );
    }
}
