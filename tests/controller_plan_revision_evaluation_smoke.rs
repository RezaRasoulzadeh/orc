//! Opt-in Qwen3 evaluation for the read-only Controller Plan revision seam.
//!
//! This evaluation uses only the bounded revision request and typed output.
//! It never persists a Plan, review, task, or workflow state.

#![cfg(feature = "llama-cpp")]

use orc::controller_plan_revision::{ControllerPlanRevisionBuilder, ControllerPlanRevisionRequest};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::protocol::{
    ExecutionHints, PROTOCOL_VERSION, PlanResponse, PlanResponseSchema, PlanningRequest,
    TaskProposal,
};
use orc::storage::db::{PersistedPlan, PlanProvenance, PlanStatus};
use orc::task::TaskPriority;
use std::env;
use std::path::PathBuf;

struct JsonRuntime {
    inner: LlamaCppRuntime,
}

impl LocalInferenceRuntime for JsonRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        let mut response = self.inner.infer(request)?;
        let value =
            serde_json::from_str::<serde_json::Value>(response.text.trim()).map_err(|error| {
                LocalInferenceError::InvalidStructuredOutput {
                    raw_output: response.text.clone(),
                    parse_error: error.to_string(),
                }
            })?;
        response.structured_output = Some(value);
        Ok(response)
    }
}

fn task() -> TaskProposal {
    TaskProposal {
        local_id: "health-endpoint".into(),
        title: "Add health endpoint".into(),
        objective: "Implement GET /health returning HTTP 200.".into(),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on: Vec::new(),
        capabilities: vec!["rust".into()],
        scope_mode: None,
        context_files: vec!["src/health.rs".into()],
        expected_changes: vec!["Implement the health handler.".into()],
        unchanged: vec!["No unrelated behavior.".into()],
        acceptance_criteria: vec!["GET /health returns HTTP 200.".into()],
        required_tests: vec!["Add a focused endpoint test.".into()],
        validation: vec!["cargo test".into()],
        execution_hints: ExecutionHints::default(),
        risk_factors: Vec::new(),
    }
}

fn plan() -> PlanResponse {
    PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: "Add and verify a health endpoint.".into(),
        assumptions: Vec::new(),
        risks: Vec::new(),
        questions: Vec::new(),
        tasks: vec![task()],
    }
}

fn planning_request() -> PlanningRequest {
    PlanningRequest {
        protocol_version: PROTOCOL_VERSION,
        kind: "project_plan_revision".into(),
        project: Some(orc::protocol::ReportProject {
            name: "controller-plan-revision-evaluation".into(),
            repository: "omitted-from-controller-request".into(),
            branch: None,
            commit: None,
        }),
        engineering_contract: "Keep the revision bounded and read-only.".into(),
        objective: "Revise one small engineering Plan.".into(),
        constraints: vec!["Preserve the endpoint objective.".into()],
        target_platforms: Vec::new(),
        stack: vec!["Rust".into()],
        non_goals: vec!["Plan persistence".into(), "Task application".into()],
        deliverables: vec!["A validated PlanResponse revision.".into()],
        definition_of_done: vec!["The revised Plan addresses the feedback.".into()],
        response_schema: PlanResponseSchema::v1(),
        role_boundaries: vec!["Controller proposes only.".into()],
        planning_constraints: Vec::new(),
        approval_requirements: vec!["Operator review is required.".into()],
        current_state: None,
        full_report: None,
        discovery_snapshot: None,
    }
}

fn persisted_plan() -> PersistedPlan {
    PersistedPlan {
        id: 7,
        project_id: 1,
        version: 1,
        parent_plan_id: None,
        provenance: PlanProvenance::controller(),
        status: PlanStatus::RevisionRequested,
        response: plan(),
        created_at: "evaluation".into(),
        superseded_by_plan_id: None,
    }
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF; see docs/orc-next/tasks/M05-006.md"]
fn qwen3_evaluates_read_only_controller_plan_revision_contract() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let request = ControllerPlanRevisionRequest::from_canonical(
        &persisted_plan(),
        "The current Plan is missing a dedicated task for testing the health endpoint. Add a new task with local_id health-endpoint-test, title Health endpoint test, and an acceptance criterion that GET /health returns HTTP 200.",
        &planning_request(),
    )
    .expect("revision request is bounded");
    let mut runtime = JsonRuntime { inner: runtime };
    let result = ControllerPlanRevisionBuilder::new().revise(&request, &mut runtime);
    match result {
        Ok(revised) => {
            let semantic = revised.tasks.iter().any(|task| {
                task.local_id == "health-endpoint-test"
                    || (task.title.to_ascii_lowercase().contains("health")
                        && task.title.to_ascii_lowercase().contains("test"))
            });
            println!("strict_contract=true semantic_revision={semantic} revised={revised:?}");
            assert!(
                semantic,
                "revision must address the persisted test feedback"
            );
        }
        Err(error) => panic!("strict_contract=false result=Fail error={error}"),
    }
    println!("strict structured contract: 1/1");
    println!("semantic revision result: 1/1");
}
