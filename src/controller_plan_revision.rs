//! Read-only Controller generation of a revised PlanResponse.
//!
//! Revision eligibility is derived from canonical persisted Plan and review
//! state. The model returns only a PlanResponse; trusted parent and review
//! identity is attached after inference for a later persistence task.

use crate::controller_memory::ControllerMemoryContext;
use crate::controller_planning::{ControllerPlanningError, ControllerPlanningRequest};
use crate::local_runtime::{
    LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest,
    LocalInferenceResponseFormat, LocalInferenceRuntime,
};
use crate::protocol::PlanResponse;
use crate::storage::db::{DbError, PersistedPlan, PlanReview};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTROLLER_PLAN_REVISION_REQUEST_VERSION: u32 = 1;
pub const MAX_CONTROLLER_PLAN_REVISION_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_PLAN_REVISION_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_PLAN_REVISION_RESULT_BYTES: usize = 64 * 1024;
const MAX_REVISION_FEEDBACK_BYTES: usize = 2048;

#[derive(Debug, Error)]
pub enum ControllerPlanRevisionError {
    #[error("Controller Plan revision has no active project")]
    NoActiveProject,
    #[error("Controller Plan {0} was not found")]
    PlanNotFound(i64),
    #[error("Plan {0} is not a Controller-origin Plan")]
    InvalidPlanOrigin(i64),
    #[error("Controller Plan {0} is not current and revision-requested")]
    PlanNotCurrent(i64),
    #[error("Controller Plan {0} has no latest review")]
    ReviewNotFound(i64),
    #[error("Controller Plan review {0} is not an actionable Controller revision")]
    ReviewNotActionable(i64),
    #[error("Controller Plan review {0} has invalid persisted feedback")]
    InvalidReview(i64),
    #[error("Controller Plan revision request is invalid: {0}")]
    InvalidRequest(String),
    #[error("Controller Plan revision memory context failed: {0}")]
    MemoryContext(String),
    #[error("Controller Plan revision request is {actual} bytes; maximum is {max}")]
    RequestTooLarge { actual: usize, max: usize },
    #[error("Controller Plan revision output is malformed: {0}")]
    InvalidStructuredOutput(String),
    #[error("Controller Plan revision output is not a valid PlanResponse: {0}")]
    InvalidPlan(String),
    #[error("Controller Plan revision inference failed: {0}")]
    Inference(#[source] LocalInferenceError),
    #[error("Controller Plan revision storage read failed: {0}")]
    Storage(#[source] DbError),
    #[error("Controller Plan revision planning context failed: {0}")]
    PlanningContext(String),
}

/// Bounded, model-independent input for one Controller Plan revision.
/// Parent/review IDs are intentionally absent: the model can revise content
/// but cannot select durable lineage or persistence metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanRevisionRequest {
    pub packet_version: u32,
    pub plan: PlanResponse,
    pub revision_feedback: String,
    pub planning_context: ControllerPlanningRequest,
}

impl ControllerPlanRevisionRequest {
    pub fn from_canonical(
        plan: &PersistedPlan,
        revision_feedback: &str,
        planning_request: &crate::protocol::PlanningRequest,
    ) -> Result<Self, ControllerPlanRevisionError> {
        let planning_context = ControllerPlanningRequest::from_canonical(planning_request)
            .map_err(|error| ControllerPlanRevisionError::InvalidRequest(error.to_string()))?;
        let request = Self {
            packet_version: CONTROLLER_PLAN_REVISION_REQUEST_VERSION,
            plan: plan.response.clone(),
            revision_feedback: revision_feedback.to_owned(),
            planning_context,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), ControllerPlanRevisionError> {
        if self.packet_version != CONTROLLER_PLAN_REVISION_REQUEST_VERSION {
            return Err(ControllerPlanRevisionError::InvalidRequest(
                "unsupported Controller Plan revision request version".into(),
            ));
        }
        self.plan
            .validate()
            .map_err(|error| ControllerPlanRevisionError::InvalidPlan(error.to_string()))?;
        validate_feedback(&self.revision_feedback)?;
        self.planning_context
            .validate()
            .map_err(|error| ControllerPlanRevisionError::InvalidRequest(error.to_string()))?;
        let size = serde_json::to_vec(self)
            .map_err(|error| ControllerPlanRevisionError::InvalidRequest(error.to_string()))?
            .len();
        if size > MAX_CONTROLLER_PLAN_REVISION_REQUEST_BYTES {
            return Err(ControllerPlanRevisionError::RequestTooLarge {
                actual: size,
                max: MAX_CONTROLLER_PLAN_REVISION_REQUEST_BYTES,
            });
        }
        Ok(())
    }
}

/// Capability-local Plan-revision inference input. The current revision
/// request remains authoritative; memory is separate bounded advisory context
/// with no lineage, persistence, authorization, or execution capability.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanRevisionInput {
    pub current_request: ControllerPlanRevisionRequest,
    pub memory: ControllerMemoryContext,
}

impl ControllerPlanRevisionInput {
    pub fn from_request(
        request: &ControllerPlanRevisionRequest,
        memory: ControllerMemoryContext,
    ) -> Self {
        Self {
            current_request: request.clone(),
            memory,
        }
    }

    pub fn validate(&self) -> Result<(), ControllerPlanRevisionError> {
        self.current_request.validate()?;
        self.memory
            .validate()
            .map_err(|error| ControllerPlanRevisionError::MemoryContext(error.to_string()))?;
        let actual = serde_json::to_vec(self)
            .map_err(|error| ControllerPlanRevisionError::InvalidRequest(error.to_string()))?
            .len();
        if actual > MAX_CONTROLLER_PLAN_REVISION_INPUT_BYTES {
            return Err(ControllerPlanRevisionError::RequestTooLarge {
                actual,
                max: MAX_CONTROLLER_PLAN_REVISION_INPUT_BYTES,
            });
        }
        Ok(())
    }
}

fn validate_feedback(value: &str) -> Result<(), ControllerPlanRevisionError> {
    if value.trim().is_empty() || value.len() > MAX_REVISION_FEEDBACK_BYTES {
        return Err(ControllerPlanRevisionError::InvalidRequest(
            "revision feedback must be non-empty and bounded".into(),
        ));
    }
    Ok(())
}

/// Typed read-only output. The identity fields are copied from canonical
/// state by OrcApp and never decoded from model output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlanRevisionResult {
    pub parent_plan_id: i64,
    pub parent_plan_version: i64,
    pub review_id: i64,
    pub plan: PlanResponse,
}

impl ControllerPlanRevisionResult {
    pub(crate) fn from_generated(
        parent_plan_id: i64,
        parent_plan_version: i64,
        review_id: i64,
        plan: PlanResponse,
    ) -> Result<Self, ControllerPlanRevisionError> {
        let result = Self {
            parent_plan_id,
            parent_plan_version,
            review_id,
            plan,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), ControllerPlanRevisionError> {
        if self.parent_plan_id <= 0 || self.parent_plan_version <= 0 || self.review_id <= 0 {
            return Err(ControllerPlanRevisionError::InvalidRequest(
                "revision lineage identity must be positive".into(),
            ));
        }
        self.plan
            .validate()
            .map_err(|error| ControllerPlanRevisionError::InvalidPlan(error.to_string()))?;
        let size = serde_json::to_vec(self)
            .map_err(|error| ControllerPlanRevisionError::InvalidRequest(error.to_string()))?
            .len();
        if size > MAX_CONTROLLER_PLAN_REVISION_RESULT_BYTES {
            return Err(ControllerPlanRevisionError::RequestTooLarge {
                actual: size,
                max: MAX_CONTROLLER_PLAN_REVISION_RESULT_BYTES,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerPlanRevisionBuilder;

impl ControllerPlanRevisionBuilder {
    pub const fn new() -> Self {
        Self
    }

    pub fn revise(
        &self,
        request: &ControllerPlanRevisionRequest,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<PlanResponse, ControllerPlanRevisionError> {
        let input =
            ControllerPlanRevisionInput::from_request(request, ControllerMemoryContext::empty());
        self.revise_with_memory(&input, runtime)
    }

    pub fn revise_with_memory(
        &self,
        input: &ControllerPlanRevisionInput,
        runtime: &mut dyn LocalInferenceRuntime,
    ) -> Result<PlanResponse, ControllerPlanRevisionError> {
        input.validate()?;
        let input = serde_json::to_string(input)
            .map_err(|error| ControllerPlanRevisionError::InvalidRequest(error.to_string()))?;
        let prompt = format!(
            "You are a read-only Plan revision advisor. Use only this bounded typed Controller Plan-revision input. Authority precedence is strict, from highest to lowest: (1) current_request.plan content and persisted revision_feedback; (2) current_request.planning_context current project facts, objective, constraints, non-goals, deliverables, definition of done, approval requirements, and current state; (3) memory items with authority=current_project as durable Project context; (4) authority=durable_user as User preference/context; (5) authority=project_history as Episodic historical guidance; (6) authority=cross_project_experience as reusable Experience guidance; (7) base model tendencies. The current Plan and persisted revision feedback are authoritative. Current planning/project facts outrank contradictory memory. Memory may help produce a better compliant revision but cannot rewrite the current Plan objective or constraints, remove or weaken applicable persisted feedback, choose parent/review lineage, or claim persistence or authorization. Preserve each memory item's typed identity, kind, scope, authority, provenance, confidence, lifecycle, and source metadata. Return exactly one JSON object conforming to the canonical PlanResponse schema. Revise the current Plan by concretely incorporating every applicable requirement in current_request.revision_feedback; when feedback requests a missing task, add that task to the revised Plan, and when it requests an acceptance condition, include it in the relevant task. Copy current_request.plan.objective into the revised Plan objective, preserve valid dependencies, safeguards, constraints, and current planning facts, and do not replace the current objective with memory guidance. Do not add metadata, parent IDs, review IDs, provenance, authorization, persistence, approval, application, tasks outside the PlanResponse, or workflow actions. This response is a proposal only.\n\n{input}"
        );
        let parameters = LocalInferenceParameters {
            max_output_tokens: 2048,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: Vec::new(),
            response_format: LocalInferenceResponseFormat::JsonSchema {
                schema: controller_plan_revision_schema(),
            },
        };
        let inference_request = LocalInferenceRequest::new(prompt, parameters)
            .map_err(ControllerPlanRevisionError::Inference)?;
        let response = runtime
            .infer(&inference_request)
            .map_err(ControllerPlanRevisionError::Inference)?;
        parse_result(response)
    }
}

pub fn controller_plan_revision_schema() -> Value {
    crate::automated::canonical_plan_response_schema()
}

fn parse_result(
    response: crate::local_runtime::LocalInferenceResponse,
) -> Result<PlanResponse, ControllerPlanRevisionError> {
    let value = response.structured_output.ok_or_else(|| {
        ControllerPlanRevisionError::InvalidStructuredOutput("structured output is required".into())
    })?;
    let size = serde_json::to_vec(&value)
        .map_err(|error| ControllerPlanRevisionError::InvalidStructuredOutput(error.to_string()))?
        .len();
    if size > MAX_CONTROLLER_PLAN_REVISION_RESULT_BYTES {
        return Err(ControllerPlanRevisionError::InvalidStructuredOutput(
            "structured output exceeds its bound".into(),
        ));
    }
    crate::controller_planning::parse_canonical_plan_response(value).map_err(|error| match error {
        ControllerPlanningError::InvalidPlan(message) => {
            ControllerPlanRevisionError::InvalidPlan(message)
        }
        other => ControllerPlanRevisionError::InvalidStructuredOutput(other.to_string()),
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedControllerReviewDetails {
    #[serde(rename = "details")]
    _details: String,
    revision_feedback: Option<String>,
}

pub(crate) fn persisted_revision_feedback(
    review: &PlanReview,
) -> Result<String, ControllerPlanRevisionError> {
    let details = serde_json::from_str::<PersistedControllerReviewDetails>(&review.details)
        .map_err(|_| ControllerPlanRevisionError::InvalidReview(review.id))?;
    let Some(feedback) = details.revision_feedback else {
        return Err(ControllerPlanRevisionError::InvalidReview(review.id));
    };
    validate_feedback(&feedback)
        .map_err(|_| ControllerPlanRevisionError::InvalidReview(review.id))?;
    Ok(feedback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::OrcApp;
    use crate::controller_memory::{
        CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryItem,
    };
    use crate::memory::{
        MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
        MemoryScope,
    };
    use crate::storage::db::{AgentRunExecution, LeadDecisionMetadata, PlanOrigin, PlanStatus};
    use crate::task::TaskPriority;

    struct FakeRuntime {
        response: crate::local_runtime::LocalInferenceResponse,
        requests: Vec<crate::local_runtime::LocalInferenceRequest>,
    }

    impl FakeRuntime {
        fn new(response: crate::local_runtime::LocalInferenceResponse) -> Self {
            Self {
                response,
                requests: Vec::new(),
            }
        }
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            request: &crate::local_runtime::LocalInferenceRequest,
        ) -> Result<crate::local_runtime::LocalInferenceResponse, LocalInferenceError> {
            self.requests.push(request.clone());
            Ok(self.response.clone())
        }
    }

    fn plan_response(objective: &str) -> PlanResponse {
        PlanResponse {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            objective: objective.into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks: vec![],
        }
    }

    fn app_with_controller_review(
        decision: crate::storage::db::PlanReviewDecision,
    ) -> (tempfile::TempDir, OrcApp, i64, PersistedPlan) {
        let directory = tempfile::tempdir().unwrap();
        let orc_dir = directory.path().join(".orc");
        std::fs::create_dir_all(&orc_dir).unwrap();
        std::fs::write(orc_dir.join("engineering.md"), "Keep changes focused.\n").unwrap();
        let path = orc_dir.join("orc.db");
        let database = crate::storage::Database::init(&path).unwrap();
        let project_id = database.create_project("Controller revision").unwrap();
        let plan_id = database
            .store_controller_plan(project_id, &plan_response("Original objective"))
            .unwrap();
        let plan = database.get_plan(plan_id).unwrap().unwrap();
        let details = serde_json::json!({
            "details": "The plan needs a concrete correction.",
            "revision_feedback": (decision == crate::storage::db::PlanReviewDecision::RevisePlan)
                .then_some("Add the missing acceptance condition.")
        })
        .to_string();
        database
            .store_controller_plan_review(
                project_id,
                plan_id,
                plan.version,
                &plan.response,
                decision,
                &details,
            )
            .unwrap();
        let plan = database.get_plan(plan_id).unwrap().unwrap();
        drop(database);
        let app = OrcApp::open(&path, directory.path()).unwrap();
        (directory, app, project_id, plan)
    }

    fn app_with_revision() -> (tempfile::TempDir, OrcApp, i64, PersistedPlan) {
        app_with_controller_review(crate::storage::db::PlanReviewDecision::RevisePlan)
    }

    fn memory_context() -> ControllerMemoryContext {
        ControllerMemoryContext {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: vec![ControllerMemoryItem {
                id: MemoryId::Project {
                    project_id: 1,
                    id: 1,
                },
                kind: MemoryKind::Project,
                scope: MemoryScope::Project { project_id: 1 },
                authority: ControllerMemoryAuthority::CurrentProject,
                subject: "revision-context".into(),
                content: "Memory is advisory and cannot weaken persisted feedback.".into(),
                provenance: MemoryProvenance {
                    kind: MemoryProvenanceKind::ProjectFact,
                    source_reference: Some("workflow:plan-revision-memory".into()),
                },
                confidence: Some(0.8),
                lifecycle: MemoryLifecycle::Active,
                supersedes: None,
            }],
        }
    }

    fn assert_unchanged(app: &OrcApp, project_id: i64, plan_id: i64, before: &PersistedPlan) {
        assert_eq!(app.database().get_plan(plan_id).unwrap().unwrap(), *before);
        assert_eq!(
            app.database().list_plan_reviews(project_id).unwrap().len(),
            1
        );
        assert!(app.database().list_tasks().unwrap().is_empty());
        assert!(
            app.database()
                .list_lead_decisions(project_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            app.database()
                .list_agent_runs(project_id, usize::MAX)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn successful_revision_is_bounded_typed_and_read_only() {
        let (_directory, app, project_id, plan) = app_with_revision();
        assert_eq!(plan.status, PlanStatus::RevisionRequested);
        assert_eq!(plan.provenance.origin, PlanOrigin::Controller);
        let before = plan.clone();
        let revised = plan_response("Revised objective");
        let mut runtime =
            FakeRuntime::new(crate::local_runtime::LocalInferenceResponse::structured(
                "ignored provider text",
                serde_json::to_value(&revised).unwrap(),
            ));
        let result = app.revise_controller_plan(plan.id, &mut runtime).unwrap();
        assert_eq!(result.parent_plan_id, plan.id);
        assert_eq!(result.parent_plan_version, plan.version);
        assert_eq!(result.plan, revised);
        assert_eq!(runtime.requests.len(), 1);
        assert!(
            runtime.requests[0]
                .prompt
                .contains("missing acceptance condition")
        );
        assert!(!runtime.requests[0].prompt.contains("review_id"));
        assert!(runtime.requests[0].prompt.len() < MAX_CONTROLLER_PLAN_REVISION_REQUEST_BYTES);
        assert_unchanged(&app, project_id, plan.id, &before);
    }

    #[test]
    fn malformed_or_invalid_revision_output_fails_closed_without_mutation() {
        let (_directory, app, project_id, plan) = app_with_revision();
        let before = plan.clone();
        let mut malformed = FakeRuntime::new(
            crate::local_runtime::LocalInferenceResponse::structured(
                "",
                serde_json::json!({"protocol_version": 1, "objective": "new", "assumptions": [], "risks": [], "questions": [], "tasks": [], "extra": true}),
            ),
        );
        assert!(matches!(
            app.revise_controller_plan(plan.id, &mut malformed),
            Err(ControllerPlanRevisionError::InvalidStructuredOutput(_))
        ));
        assert_unchanged(&app, project_id, plan.id, &before);

        let mut invalid = FakeRuntime::new(
            crate::local_runtime::LocalInferenceResponse::structured(
                "",
                serde_json::json!({"protocol_version": 99, "objective": "new", "assumptions": [], "risks": [], "questions": [], "tasks": []}),
            ),
        );
        assert!(matches!(
            app.revise_controller_plan(plan.id, &mut invalid),
            Err(ControllerPlanRevisionError::InvalidPlan(_))
        ));
        assert_unchanged(&app, project_id, plan.id, &before);
    }

    #[test]
    fn ineligible_review_and_plan_states_are_rejected_before_inference() {
        let (_directory, app, project_id, plan) = app_with_revision();
        let database = app.database();
        let task = database
            .insert_task(
                project_id,
                "legacy-review-task",
                "legacy review task",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let decision = database
            .record_lead_decision(
                project_id,
                &crate::lead::LeadDecisionKind::RevisePlan,
                &serde_json::json!({"review":"legacy"}),
                LeadDecisionMetadata {
                    snapshot: "snapshot",
                    run_id: None,
                    source_request: "request",
                    summary: "summary",
                },
            )
            .unwrap();
        let run = database
            .create_agent_run_with_execution(
                project_id,
                &task,
                "planner",
                crate::registry::AUTOMATED,
                AgentRunExecution {
                    class: "plan",
                    model: None,
                    effort: None,
                    source: "test",
                },
            )
            .unwrap();
        database
            .record_plan_review(
                plan.id,
                run,
                decision,
                &crate::lead::LeadDecisionKind::RevisePlan,
                "legacy review",
            )
            .unwrap();
        let mut runtime =
            FakeRuntime::new(crate::local_runtime::LocalInferenceResponse::text("unused"));
        assert!(matches!(
            app.revise_controller_plan(plan.id, &mut runtime),
            Err(ControllerPlanRevisionError::ReviewNotActionable(_))
        ));
        assert!(runtime.requests.is_empty());

        let second = database
            .store_controller_plan(project_id, &plan.response)
            .unwrap();
        assert_ne!(second, plan.id);
        let mut stale_runtime =
            FakeRuntime::new(crate::local_runtime::LocalInferenceResponse::text("unused"));
        assert!(matches!(
            app.revise_controller_plan(plan.id, &mut stale_runtime),
            Err(ControllerPlanRevisionError::PlanNotCurrent(id)) if id == plan.id
        ));
        assert!(stale_runtime.requests.is_empty());

        for decision in [
            crate::storage::db::PlanReviewDecision::Approve,
            crate::storage::db::PlanReviewDecision::UserDecisionRequired,
        ] {
            let (_directory, app, _project_id, plan) = app_with_controller_review(decision);
            let mut runtime =
                FakeRuntime::new(crate::local_runtime::LocalInferenceResponse::text("unused"));
            let outcome = app.revise_controller_plan(plan.id, &mut runtime);
            assert!(
                matches!(
                    outcome,
                    Err(ControllerPlanRevisionError::PlanNotCurrent(id)) if id == plan.id
                ),
                "unexpected ineligible review result: {outcome:?}"
            );
            assert!(runtime.requests.is_empty());
        }
    }

    #[test]
    fn request_has_no_trusted_lineage_metadata() {
        let (_directory, app, _project_id, plan) = app_with_revision();
        let reviews = app.database().list_plan_reviews(plan.project_id).unwrap();
        let feedback = persisted_revision_feedback(&reviews[0]).unwrap();
        let canonical = app.planning_request().unwrap();
        let request =
            ControllerPlanRevisionRequest::from_canonical(&plan, &feedback, &canonical).unwrap();
        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("parent_plan_id").is_none());
        assert!(value.get("review_id").is_none());
        assert!(value.get("provenance").is_none());
        assert!(value.get("authorization").is_none());
        assert!(value.to_string().len() <= MAX_CONTROLLER_PLAN_REVISION_REQUEST_BYTES);
    }

    #[test]
    fn output_result_carries_only_canonical_lineage_and_plan() {
        let result =
            ControllerPlanRevisionResult::from_generated(7, 2, 9, plan_response("revised"))
                .unwrap();
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert!(value.get("lead_decision_id").is_none());
        assert!(value.get("planner_run_id").is_none());
    }

    #[test]
    fn revision_input_preserves_typed_memory_and_prompt_authority() {
        let (_directory, app, _project_id, plan) = app_with_revision();
        let reviews = app.database().list_plan_reviews(plan.project_id).unwrap();
        let feedback = persisted_revision_feedback(&reviews[0]).unwrap();
        let planning_request = app.planning_request().unwrap();
        let request =
            ControllerPlanRevisionRequest::from_canonical(&plan, &feedback, &planning_request)
                .unwrap();
        let input = ControllerPlanRevisionInput::from_request(&request, memory_context());
        input.validate().unwrap();
        let serialized = serde_json::to_value(&input).unwrap();
        assert_eq!(
            serialized["current_request"]["plan"]["objective"],
            "Original objective"
        );
        assert_eq!(
            serialized["current_request"]["revision_feedback"],
            "Add the missing acceptance condition."
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
            "workflow:plan-revision-memory"
        );
        assert!(
            serialized["current_request"]
                .get("parent_plan_id")
                .is_none()
        );
        assert!(serialized["current_request"].get("review_id").is_none());

        let mut runtime =
            FakeRuntime::new(crate::local_runtime::LocalInferenceResponse::structured(
                "ignored provider text",
                serde_json::to_value(plan_response("Revised objective")).unwrap(),
            ));
        ControllerPlanRevisionBuilder::new()
            .revise_with_memory(&input, &mut runtime)
            .unwrap();
        let prompt = &runtime.requests[0].prompt;
        assert!(prompt.contains("current_request.plan content and persisted revision_feedback"));
        assert!(prompt.contains("current_request.planning_context current project facts"));
        assert!(prompt.contains("authority=current_project"));
        assert!(prompt.contains("authority=durable_user"));
        assert!(prompt.contains("authority=project_history"));
        assert!(prompt.contains("authority=cross_project_experience"));
        assert!(prompt.contains("current Plan and persisted revision feedback are authoritative"));
        assert!(prompt.contains("cannot rewrite the current Plan objective or constraints"));
        assert!(prompt.contains("Add the missing acceptance condition."));
        assert!(prompt.contains("workflow:plan-revision-memory"));
        assert!(!prompt.contains("parent_plan_id"));
        assert!(!prompt.contains("review_id"));
    }

    #[test]
    fn combined_revision_input_bound_preserves_independent_request_and_memory_bounds() {
        let (_directory, app, _project_id, plan) = app_with_revision();
        let reviews = app.database().list_plan_reviews(plan.project_id).unwrap();
        let feedback = persisted_revision_feedback(&reviews[0]).unwrap();
        let planning_request = app.planning_request().unwrap();

        let mut oversized_request =
            ControllerPlanRevisionRequest::from_canonical(&plan, &feedback, &planning_request)
                .unwrap();
        oversized_request.plan.objective = "x".repeat(MAX_CONTROLLER_PLAN_REVISION_REQUEST_BYTES);
        assert!(matches!(
            oversized_request.validate(),
            Err(ControllerPlanRevisionError::RequestTooLarge {
                max: MAX_CONTROLLER_PLAN_REVISION_REQUEST_BYTES,
                ..
            })
        ));

        let mut current_request =
            ControllerPlanRevisionRequest::from_canonical(&plan, &feedback, &planning_request)
                .unwrap();
        current_request.plan.objective = "x".repeat(40_000);
        current_request.validate().unwrap();
        let memory = ControllerMemoryContext {
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
                    subject: format!("bound-{index}"),
                    content: "m".repeat(3600),
                    provenance: MemoryProvenance {
                        kind: MemoryProvenanceKind::ProjectFact,
                        source_reference: Some(format!("test:bound-{index}")),
                    },
                    confidence: None,
                    lifecycle: MemoryLifecycle::Active,
                    supersedes: None,
                })
                .collect(),
        };
        memory.validate().unwrap();
        let input = ControllerPlanRevisionInput::from_request(&current_request, memory);
        assert!(matches!(
            input.validate(),
            Err(ControllerPlanRevisionError::RequestTooLarge {
                max: MAX_CONTROLLER_PLAN_REVISION_INPUT_BYTES,
                ..
            })
        ));
    }

    #[test]
    fn contradictory_memory_cannot_suppress_feedback_or_change_revision_lineage() {
        let (_directory, app, _project_id, plan) = app_with_revision();
        let reviews = app.database().list_plan_reviews(plan.project_id).unwrap();
        let feedback = persisted_revision_feedback(&reviews[0]).unwrap();
        let request = ControllerPlanRevisionRequest::from_canonical(
            &plan,
            &feedback,
            &app.planning_request().unwrap(),
        )
        .unwrap();
        let input = ControllerPlanRevisionInput::from_request(&request, memory_context());
        let revised = plan_response("Revised objective");
        let mut runtime =
            FakeRuntime::new(crate::local_runtime::LocalInferenceResponse::structured(
                "ignored provider text",
                serde_json::to_value(&revised).unwrap(),
            ));
        let output = ControllerPlanRevisionBuilder::new()
            .revise_with_memory(&input, &mut runtime)
            .unwrap();
        assert_eq!(output, revised);
        let prompt = &runtime.requests[0].prompt;
        assert!(prompt.contains(&feedback));
        assert!(prompt.contains("Memory may help produce a better compliant revision"));
        assert!(!prompt.contains("review_id"));
        assert!(!prompt.contains("parent_plan_id"));

        let result = ControllerPlanRevisionResult::from_generated(
            plan.id,
            plan.version,
            reviews[0].id,
            output,
        )
        .unwrap();
        assert_eq!(result.parent_plan_id, plan.id);
        assert_eq!(result.parent_plan_version, plan.version);
        assert_eq!(result.review_id, reviews[0].id);
    }

    #[test]
    fn app_revision_retrieves_canonical_memory_read_only_after_eligibility_checks() {
        let (_directory, app, project_id, plan) = app_with_revision();
        let memory = app
            .database()
            .create_memory(&MemoryDraft {
                kind: MemoryKind::Project,
                scope: MemoryScope::Project { project_id },
                subject: "revision-guidance".into(),
                content: "Use current persisted feedback; this is advisory context only.".into(),
                provenance: MemoryProvenance {
                    kind: MemoryProvenanceKind::ProjectFact,
                    source_reference: Some("test:plan-revision-memory".into()),
                },
                confidence: Some(0.9),
            })
            .unwrap();
        let before = app.memories().unwrap().history(&memory.id).unwrap();
        let revised = plan_response("Revised objective");
        let mut runtime =
            FakeRuntime::new(crate::local_runtime::LocalInferenceResponse::structured(
                "ignored provider text",
                serde_json::to_value(&revised).unwrap(),
            ));
        let result = app.revise_controller_plan(plan.id, &mut runtime).unwrap();
        assert_eq!(result.parent_plan_id, plan.id);
        assert_eq!(result.parent_plan_version, plan.version);
        assert_eq!(result.review_id, 1);
        assert_eq!(result.plan, revised);
        let prompt = &runtime.requests[0].prompt;
        assert!(prompt.contains("test:plan-revision-memory"));
        assert!(prompt.contains("Add the missing acceptance condition."));
        assert_eq!(app.memories().unwrap().history(&memory.id).unwrap(), before);
    }
}
