//! Read-only Controller judgment for a current persisted Plan.
//!
//! This is deliberately separate from the legacy Lead review path. It
//! produces no Lead decision, PlanReview, status transition, approval,
//! revision, task, workflow, or worker mutation.

use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::protocol::{PlanResponse, PlanningProjectState, TaskSummary};
use crate::storage::db::{PersistedPlan, PlanOrigin, PlanStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTROLLER_PLAN_REVIEW_REQUEST_VERSION: u32 = 1;
const MAX_REVIEW_REQUEST_BYTES: usize = 64 * 1024;
const MAX_REVIEW_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_TEXT_BYTES: usize = 2048;
const MAX_STATE_ITEMS: usize = 16;

#[derive(Debug, Error)]
pub enum ControllerPlanReviewError {
    #[error("Controller Plan review request is invalid: {0}")]
    InvalidRequest(String),
    #[error("Controller Plan review request is {actual} bytes; maximum is {max}")]
    RequestTooLarge { actual: usize, max: usize },
    #[error("Controller Plan review output is malformed: {0}")]
    MalformedStructuredOutput(String),
    #[error("Controller Plan review inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
    #[error("Controller Plan review storage read failed: {0}")]
    Storage(#[source] crate::storage::db::DbError),
    #[error("Controller Plan {0} was not found")]
    PlanNotFound(i64),
    #[error("Controller Plan {0} is not the current valid Plan")]
    PlanNotCurrent(i64),
    #[error("Controller Plan review has no active project")]
    NoActiveProject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanReviewTask {
    pub id: String,
    pub title: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanReviewState {
    pub task_counts: Vec<ControllerPlanReviewCount>,
    pub ready_tasks: Vec<ControllerPlanReviewTask>,
    pub active_tasks: Vec<ControllerPlanReviewTask>,
    pub review_tasks: Vec<ControllerPlanReviewTask>,
    pub blocked_tasks: Vec<ControllerPlanReviewTask>,
    pub usable_agent_count: usize,
    pub busy_agent_count: usize,
    pub quota_reserve_percent: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanReviewCount {
    pub status: String,
    pub count: usize,
}

impl ControllerPlanReviewState {
    fn from_canonical(state: &PlanningProjectState) -> Self {
        Self {
            task_counts: state
                .task_counts
                .iter()
                .take(MAX_STATE_ITEMS)
                .map(|(status, count)| ControllerPlanReviewCount {
                    status: bound_text(status),
                    count: *count,
                })
                .collect(),
            ready_tasks: bound_tasks(&state.ready_tasks),
            active_tasks: bound_tasks(&state.active_tasks),
            review_tasks: bound_tasks(&state.review_tasks),
            blocked_tasks: bound_tasks(&state.blocked_tasks),
            usable_agent_count: state.usable_agents.len(),
            busy_agent_count: state.busy_agents.len(),
            quota_reserve_percent: state.quota_reserve_percent,
        }
    }
}

fn bound_tasks(tasks: &[TaskSummary]) -> Vec<ControllerPlanReviewTask> {
    tasks
        .iter()
        .take(MAX_STATE_ITEMS)
        .map(|task| ControllerPlanReviewTask {
            id: bound_text(&task.id),
            title: bound_text(&task.title),
            status: bound_text(&task.status),
        })
        .collect()
}

/// Bounded model-independent review input. It contains no repository path,
/// Git state, SQLite handle, worker, Lead decision, provider data, or runtime
/// object. Origin is sufficient provenance for semantic review; legacy source
/// IDs are intentionally not copied into the model packet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanReviewRequest {
    pub packet_version: u32,
    pub plan_id: i64,
    pub plan_version: i64,
    pub plan_status: PlanStatus,
    pub plan_origin: PlanOrigin,
    pub plan: PlanResponse,
    pub project_name: Option<String>,
    pub current_state: ControllerPlanReviewState,
    pub operator_resolution: Option<String>,
}

impl ControllerPlanReviewRequest {
    pub fn from_persisted(
        plan: &PersistedPlan,
        project_name: Option<&str>,
        state: &PlanningProjectState,
        operator_resolution: Option<&str>,
    ) -> Result<Self, ControllerPlanReviewError> {
        let request = Self {
            packet_version: CONTROLLER_PLAN_REVIEW_REQUEST_VERSION,
            plan_id: plan.id,
            plan_version: plan.version,
            plan_status: plan.status,
            plan_origin: plan.provenance.origin,
            plan: plan.response.clone(),
            project_name: project_name.map(bound_text),
            current_state: ControllerPlanReviewState::from_canonical(state),
            operator_resolution: operator_resolution.map(bound_text),
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<String, ControllerPlanReviewError> {
        if self.packet_version != CONTROLLER_PLAN_REVIEW_REQUEST_VERSION {
            return Err(ControllerPlanReviewError::InvalidRequest(
                "unsupported Controller Plan review request version".into(),
            ));
        }
        if self.plan_id <= 0 || self.plan_version <= 0 {
            return Err(ControllerPlanReviewError::InvalidRequest(
                "Plan identity must be positive".into(),
            ));
        }
        self.plan
            .validate()
            .map_err(|error| ControllerPlanReviewError::InvalidRequest(error.to_string()))?;
        for (field, value) in [
            ("project_name", self.project_name.as_deref()),
            ("operator_resolution", self.operator_resolution.as_deref()),
        ] {
            if let Some(value) = value {
                validate_text(value, field)?;
            }
        }
        let serialized = serde_json::to_vec(self)
            .map_err(|error| ControllerPlanReviewError::InvalidRequest(error.to_string()))?;
        if serialized.len() > MAX_REVIEW_REQUEST_BYTES {
            return Err(ControllerPlanReviewError::RequestTooLarge {
                actual: serialized.len(),
                max: MAX_REVIEW_REQUEST_BYTES,
            });
        }
        String::from_utf8(serialized)
            .map_err(|error| ControllerPlanReviewError::InvalidRequest(error.to_string()))
    }
}

/// The only semantic decisions exposed by this Controller review boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanReviewDecision {
    Approve,
    RevisePlan,
    OperatorDecisionRequired,
}

/// Bounded typed Controller review output. It is advisory judgment only and
/// contains no persistence, authorization, or execution capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanReviewResult {
    pub decision: ControllerPlanReviewDecision,
    pub details: String,
    pub revision_feedback: Option<String>,
}

impl ControllerPlanReviewResult {
    fn validate(&self) -> Result<(), ControllerPlanReviewError> {
        validate_text(&self.details, "details")?;
        if self.details.trim().is_empty() {
            return Err(ControllerPlanReviewError::MalformedStructuredOutput(
                "details must not be empty".into(),
            ));
        }
        if let Some(feedback) = &self.revision_feedback {
            validate_text(feedback, "revision_feedback")?;
            if feedback.trim().is_empty() {
                return Err(ControllerPlanReviewError::MalformedStructuredOutput(
                    "revision_feedback must not be empty when supplied".into(),
                ));
            }
        }
        if matches!(self.decision, ControllerPlanReviewDecision::RevisePlan)
            && self.revision_feedback.is_none()
        {
            return Err(ControllerPlanReviewError::MalformedStructuredOutput(
                "revise_plan requires revision_feedback".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerPlanReviewBuilder;

impl ControllerPlanReviewBuilder {
    pub const fn new() -> Self {
        Self
    }

    pub fn review(
        &self,
        request: &ControllerPlanReviewRequest,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<ControllerPlanReviewResult, ControllerPlanReviewError> {
        let request_json = request.validate()?;
        let prompt = format!(
            "You are a read-only semantic Plan reviewer. Use only the bounded JSON below. Return exactly one JSON object with decision, details, and revision_feedback. decision must be exactly approve, revise_plan, or operator_decision_required. Approve only when the Plan is coherent and satisfies its stated objective and constraints. Choose revise_plan only for a concrete correctable defect and put the bounded correction in revision_feedback. Choose operator_decision_required when the supplied facts are ambiguous or require a human decision; if the Plan contains an unresolved question explicitly requiring an operator choice, choose operator_decision_required even if the Plan also has defects. Do not apply, persist, approve, revise, or execute anything; this response is advisory judgment only. Do not invent facts outside the JSON.\n\n{request_json}"
        );
        let parameters = LocalInferenceParameters {
            max_output_tokens: 512,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: Vec::new(),
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: controller_plan_review_schema(),
            },
        };
        let inference_request = LocalInferenceRequest::new(prompt, parameters)
            .map_err(ControllerPlanReviewError::Inference)?;
        let response = runtime
            .infer(&inference_request)
            .map_err(ControllerPlanReviewError::Inference)?;
        parse_result(response)
    }
}

pub fn controller_plan_review_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "decision": {"type": "string", "enum": ["approve", "revise_plan", "operator_decision_required"]},
            "details": {"type": "string", "minLength": 1, "maxLength": MAX_TEXT_BYTES},
            "revision_feedback": {"type": ["string", "null"], "maxLength": MAX_TEXT_BYTES}
        },
        "required": ["decision", "details", "revision_feedback"]
    })
}

fn parse_result(
    response: crate::local_runtime::LocalInferenceResponse,
) -> Result<ControllerPlanReviewResult, ControllerPlanReviewError> {
    let value = response.structured_output.ok_or_else(|| {
        ControllerPlanReviewError::MalformedStructuredOutput("structured output is required".into())
    })?;
    let size = serde_json::to_vec(&value)
        .map_err(|error| ControllerPlanReviewError::MalformedStructuredOutput(error.to_string()))?
        .len();
    if size > MAX_REVIEW_RESPONSE_BYTES {
        return Err(ControllerPlanReviewError::MalformedStructuredOutput(
            "structured output exceeds its bound".into(),
        ));
    }
    expect_keys(&value, &["decision", "details", "revision_feedback"])?;
    let result = serde_json::from_value::<ControllerPlanReviewResult>(value)
        .map_err(|error| ControllerPlanReviewError::MalformedStructuredOutput(error.to_string()))?;
    result.validate()?;
    Ok(result)
}

fn expect_keys(value: &Value, expected: &[&str]) -> Result<(), ControllerPlanReviewError> {
    let object = value.as_object().ok_or_else(|| {
        ControllerPlanReviewError::MalformedStructuredOutput(
            "review result must be an object".into(),
        )
    })?;
    if object.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err(ControllerPlanReviewError::MalformedStructuredOutput(
            "review result contains an unsupported field".into(),
        ));
    }
    Ok(())
}

fn validate_text(value: &str, field: &str) -> Result<(), ControllerPlanReviewError> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(ControllerPlanReviewError::InvalidRequest(format!(
            "{field} exceeds its byte bound"
        )));
    }
    Ok(())
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
    use crate::app::OrcApp;
    use crate::storage::db::PlanProvenance;

    struct FakeRuntime {
        response: crate::local_runtime::LocalInferenceResponse,
        requests: Vec<crate::local_runtime::LocalInferenceRequest>,
    }

    impl FakeRuntime {
        fn new(value: Value) -> Self {
            Self {
                response: crate::local_runtime::LocalInferenceResponse::structured(
                    "structured review",
                    value,
                ),
                requests: Vec::new(),
            }
        }
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            request: &LocalInferenceRequest,
        ) -> Result<crate::local_runtime::LocalInferenceResponse, LocalInferenceError> {
            self.requests.push(request.clone());
            Ok(self.response.clone())
        }
    }

    fn plan() -> PersistedPlan {
        PersistedPlan {
            id: 7,
            project_id: 1,
            version: 2,
            parent_plan_id: Some(6),
            provenance: PlanProvenance::controller(),
            status: PlanStatus::Proposed,
            response: PlanResponse {
                protocol_version: crate::protocol::PROTOCOL_VERSION,
                objective: "review this plan".into(),
                assumptions: vec![],
                risks: vec![],
                questions: vec![],
                tasks: vec![],
            },
            created_at: "now".into(),
            superseded_by_plan_id: None,
        }
    }

    fn state() -> PlanningProjectState {
        PlanningProjectState {
            task_counts: [("ready".into(), 1)].into_iter().collect(),
            ready_tasks: vec![],
            active_tasks: vec![],
            review_tasks: vec![],
            blocked_tasks: vec![],
            usable_agents: vec!["agent".into()],
            busy_agents: vec![],
            quota_reserve_percent: 10,
        }
    }

    fn request() -> ControllerPlanReviewRequest {
        ControllerPlanReviewRequest::from_persisted(
            &plan(),
            Some("project"),
            &state(),
            Some("operator context"),
        )
        .unwrap()
    }

    fn output(decision: &str, feedback: Option<&str>) -> Value {
        serde_json::json!({
            "decision": decision,
            "details": "bounded review details",
            "revision_feedback": feedback,
        })
    }

    fn app_with_current_controller_plan() -> (tempfile::TempDir, OrcApp, i64, i64) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("orc.db");
        let database = crate::storage::Database::init(&path).unwrap();
        let project_id = database.create_project("review project").unwrap();
        let plan_id = database
            .store_controller_plan(
                project_id,
                &PlanResponse {
                    protocol_version: crate::protocol::PROTOCOL_VERSION,
                    objective: "review this plan".into(),
                    assumptions: vec![],
                    risks: vec![],
                    questions: vec![],
                    tasks: vec![],
                },
            )
            .unwrap();
        drop(database);
        let app = OrcApp::open(&path, directory.path()).unwrap();
        (directory, app, project_id, plan_id)
    }

    #[test]
    fn fake_runtime_covers_all_three_typed_review_decisions() {
        for (decision, feedback, expected) in [
            ("approve", None, ControllerPlanReviewDecision::Approve),
            (
                "revise_plan",
                Some("add the missing acceptance evidence"),
                ControllerPlanReviewDecision::RevisePlan,
            ),
            (
                "operator_decision_required",
                None,
                ControllerPlanReviewDecision::OperatorDecisionRequired,
            ),
        ] {
            let mut runtime = FakeRuntime::new(output(decision, feedback));
            let result = ControllerPlanReviewBuilder::new()
                .review(&request(), &mut runtime)
                .unwrap();
            assert_eq!(result.decision, expected);
            assert_eq!(runtime.requests.len(), 1);
        }
    }

    #[test]
    fn malformed_output_fails_closed() {
        let mut runtime = FakeRuntime::new(serde_json::json!({
            "decision": "approve",
            "details": "ok",
            "revision_feedback": null,
            "unexpected": "must reject"
        }));
        assert!(matches!(
            ControllerPlanReviewBuilder::new().review(&request(), &mut runtime),
            Err(ControllerPlanReviewError::MalformedStructuredOutput(_))
        ));
    }

    #[test]
    fn request_projection_is_bounded_and_contains_no_runtime_fields() {
        let mut canonical = state();
        canonical.ready_tasks = (0..32)
            .map(|index| TaskSummary {
                id: format!("T-{index}"),
                title: "task".into(),
                status: "ready".into(),
            })
            .collect();
        canonical.usable_agents = (0..32).map(|index| format!("agent-{index}")).collect();
        let request = ControllerPlanReviewRequest::from_persisted(
            &plan(),
            Some(&"project".repeat(MAX_TEXT_BYTES)),
            &canonical,
            Some(&"resolution".repeat(MAX_TEXT_BYTES)),
        )
        .unwrap();
        assert_eq!(request.current_state.ready_tasks.len(), MAX_STATE_ITEMS);
        assert_eq!(request.current_state.usable_agent_count, 32);
        let serialized = serde_json::to_value(request).unwrap();
        assert!(serialized.get("runtime").is_none());
        assert!(serialized.get("worker").is_none());
    }

    #[test]
    fn app_gate_rejects_missing_and_non_current_plans_before_inference() {
        let (_directory, app, project_id, current_plan_id) = app_with_current_controller_plan();
        let mut missing_runtime = FakeRuntime::new(output("approve", None));
        assert!(matches!(
            app.review_controller_plan(999, None, &mut missing_runtime),
            Err(ControllerPlanReviewError::PlanNotFound(999))
        ));
        assert!(missing_runtime.requests.is_empty());

        let second_plan = app
            .database()
            .store_controller_plan(
                project_id,
                &PlanResponse {
                    protocol_version: crate::protocol::PROTOCOL_VERSION,
                    objective: "newer plan".into(),
                    assumptions: vec![],
                    risks: vec![],
                    questions: vec![],
                    tasks: vec![],
                },
            )
            .unwrap();
        assert!(second_plan > current_plan_id);
        let mut stale_runtime = FakeRuntime::new(output("approve", None));
        assert!(matches!(
            app.review_controller_plan(current_plan_id, None, &mut stale_runtime),
            Err(ControllerPlanReviewError::PlanNotCurrent(id)) if id == current_plan_id
        ));
        assert!(stale_runtime.requests.is_empty());
    }

    #[test]
    fn app_controller_review_is_read_only() {
        let (_directory, app, project_id, plan_id) = app_with_current_controller_plan();
        let before = (
            serde_json::to_value(app.database().get_plan(plan_id).unwrap()).unwrap(),
            serde_json::to_value(app.database().list_plan_history(project_id).unwrap()).unwrap(),
            serde_json::to_value(app.database().list_tasks().unwrap()).unwrap(),
            serde_json::to_value(app.database().list_lead_decisions(project_id).unwrap()).unwrap(),
            serde_json::to_value(
                app.database()
                    .list_agent_runs(project_id, usize::MAX)
                    .unwrap(),
            )
            .unwrap(),
            serde_json::to_value(app.database().list_plan_reviews(project_id).unwrap()).unwrap(),
            serde_json::to_value(app.workflow_state().unwrap()).unwrap(),
        );
        let mut runtime = FakeRuntime::new(output("revise_plan", Some("clarify the objective")));
        let result = app
            .review_controller_plan(plan_id, Some("operator context"), &mut runtime)
            .unwrap();
        assert_eq!(result.decision, ControllerPlanReviewDecision::RevisePlan);
        assert_eq!(runtime.requests.len(), 1);
        let after = (
            serde_json::to_value(app.database().get_plan(plan_id).unwrap()).unwrap(),
            serde_json::to_value(app.database().list_plan_history(project_id).unwrap()).unwrap(),
            serde_json::to_value(app.database().list_tasks().unwrap()).unwrap(),
            serde_json::to_value(app.database().list_lead_decisions(project_id).unwrap()).unwrap(),
            serde_json::to_value(
                app.database()
                    .list_agent_runs(project_id, usize::MAX)
                    .unwrap(),
            )
            .unwrap(),
            serde_json::to_value(app.database().list_plan_reviews(project_id).unwrap()).unwrap(),
            serde_json::to_value(app.workflow_state().unwrap()).unwrap(),
        );
        assert_eq!(before, after);
    }
}
