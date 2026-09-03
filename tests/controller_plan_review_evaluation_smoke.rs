//! Opt-in Qwen3 evaluation for the read-only Controller Plan-review seam.
//!
//! This is intentionally ignored during normal validation. It uses the
//! existing local runtime and only evaluates the bounded typed result; it
//! never persists, approves, revises, or applies a Plan.

#![cfg(feature = "llama-cpp")]

use orc::controller_plan_review::{
    ControllerPlanReviewBuilder, ControllerPlanReviewDecision, ControllerPlanReviewRequest,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::protocol::{PROTOCOL_VERSION, PlanResponse, PlanningProjectState};
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

fn task(title: &str, objective: &str, expected_changes: &[&str]) -> orc::protocol::TaskProposal {
    orc::protocol::TaskProposal {
        local_id: "review-task".into(),
        title: title.into(),
        objective: objective.into(),
        role: "implementation".into(),
        priority: TaskPriority::Normal,
        depends_on: Vec::new(),
        capabilities: vec!["rust".into()],
        scope_mode: None,
        context_files: Vec::new(),
        expected_changes: expected_changes
            .iter()
            .map(|value| (*value).into())
            .collect(),
        unchanged: vec!["No unrelated behavior".into()],
        acceptance_criteria: vec![objective.into()],
        required_tests: vec!["cargo test".into()],
        validation: vec!["cargo test --lib".into()],
        execution_hints: orc::protocol::ExecutionHints::default(),
        risk_factors: Vec::new(),
    }
}

fn plan(objective: &str, tasks: Vec<orc::protocol::TaskProposal>) -> PersistedPlan {
    PersistedPlan {
        id: 7,
        project_id: 1,
        version: 1,
        parent_plan_id: None,
        provenance: PlanProvenance::controller(),
        status: PlanStatus::Proposed,
        response: PlanResponse {
            protocol_version: PROTOCOL_VERSION,
            objective: objective.into(),
            assumptions: Vec::new(),
            risks: Vec::new(),
            questions: Vec::new(),
            tasks,
        },
        created_at: "evaluation".into(),
        superseded_by_plan_id: None,
    }
}

fn state() -> PlanningProjectState {
    PlanningProjectState {
        task_counts: [("ready".into(), 1)].into_iter().collect(),
        ready_tasks: Vec::new(),
        active_tasks: Vec::new(),
        review_tasks: Vec::new(),
        blocked_tasks: Vec::new(),
        usable_agents: vec!["bounded-agent".into()],
        busy_agents: Vec::new(),
        quota_reserve_percent: 10,
    }
}

fn request(plan: PersistedPlan) -> ControllerPlanReviewRequest {
    ControllerPlanReviewRequest::from_persisted(&plan, Some("review-evaluation"), &state(), None)
        .expect("evaluation request is valid")
}

fn operator_plan() -> PersistedPlan {
    let mut plan = plan(
        "Choose the authentication strategy for the product: either managed identity or local credentials.",
        vec![task(
            "Choose authentication strategy",
            "Choose either managed identity or local credentials; do not implement either approach until the operator selects one.",
            &["operator decision required before implementation"],
        )],
    );
    plan.response.questions = vec![
        "Operator must choose managed identity or local credentials before implementation can proceed.".into(),
    ];
    plan
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF; see docs/orc-next/tasks/M05-004.md"]
fn qwen3_evaluates_read_only_controller_plan_review_contract() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let scenarios = [
        (
            "approve",
            "Add a small health endpoint and verify it with a focused test.",
            plan(
                "Add a small health endpoint and verify it with a focused test.",
                vec![task(
                    "Add health endpoint",
                    "Implement GET /health returning HTTP 200 and add a focused test.",
                    &["src/health.rs", "tests/health.rs"],
                )],
            ),
            ControllerPlanReviewDecision::Approve,
        ),
        (
            "revise",
            "Add a small health endpoint and verify it with a focused test.",
            plan(
                "Add a small health endpoint and verify it with a focused test.",
                vec![task(
                    "Update project documentation",
                    "Rewrite the project documentation.",
                    &["README.md"],
                )],
            ),
            ControllerPlanReviewDecision::RevisePlan,
        ),
        (
            "operator",
            "Choose the authentication strategy for the product: either managed identity or local credentials.",
            operator_plan(),
            ControllerPlanReviewDecision::OperatorDecisionRequired,
        ),
    ];
    let mut runtime = JsonRuntime { inner: runtime };
    let mut strict_passed = 0;
    let mut semantic_passed = 0;
    for (scenario_id, _objective, persisted, expected) in scenarios {
        let bounded = request(persisted);
        let result = ControllerPlanReviewBuilder::new().review(&bounded, &mut runtime);
        match result {
            Ok(result) => {
                strict_passed += 1;
                let semantic = result.decision == expected;
                if semantic {
                    semantic_passed += 1;
                }
                println!(
                    "{scenario_id} strict_contract=true expected={expected:?} observed={:?} semantic={semantic}",
                    result.decision
                );
            }
            Err(error) => {
                println!("{scenario_id} strict_contract=false expected={expected:?} error={error}")
            }
        }
    }
    println!("strict structured contract: {strict_passed}/3");
    println!("semantic decision result: {semantic_passed}/3");
    assert_eq!(
        strict_passed, 3,
        "all model responses must satisfy the strict contract"
    );
    assert_eq!(
        semantic_passed, 3,
        "all representative review decisions must match"
    );
}
