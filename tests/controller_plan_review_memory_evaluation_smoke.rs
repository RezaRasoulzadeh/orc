//! Opt-in real-Qwen evaluation for Controller Plan-review memory precedence.
//!
//! The evaluation only exercises the bounded read-only review judgment. It
//! never persists a review, approves or revises a Plan, or mutates workflow or
//! memory state.

#![cfg(feature = "llama-cpp")]

use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_plan_review::{
    ControllerPlanReviewBuilder, ControllerPlanReviewDecision, ControllerPlanReviewInput,
    ControllerPlanReviewRequest,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::protocol::{PROTOCOL_VERSION, PlanResponse, PlanningProjectState, TaskProposal};
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
        response.structured_output =
            Some(serde_json::from_str(response.text.trim()).map_err(|error| {
                LocalInferenceError::InvalidStructuredOutput {
                    raw_output: response.text.clone(),
                    parse_error: error.to_string(),
                }
            })?);
        Ok(response)
    }
}

fn task(
    title: &str,
    objective: &str,
    expected_changes: &[&str],
    acceptance: &[&str],
) -> TaskProposal {
    TaskProposal {
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
        acceptance_criteria: acceptance.iter().map(|value| (*value).into()).collect(),
        required_tests: vec!["cargo test --lib".into()],
        validation: vec!["cargo test --lib".into()],
        execution_hints: orc::protocol::ExecutionHints {
            class: None,
            model: None,
            effort: Some("low".into()),
            effort_reason: Some("bounded review evaluation".into()),
        },
        risk_factors: Vec::new(),
    }
}

fn plan(
    objective: &str,
    tasks: Vec<TaskProposal>,
    assumptions: Vec<&str>,
    risks: Vec<&str>,
    questions: Vec<&str>,
) -> PersistedPlan {
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
            assumptions: assumptions.into_iter().map(str::to_owned).collect(),
            risks: risks.into_iter().map(str::to_owned).collect(),
            questions: questions.into_iter().map(str::to_owned).collect(),
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

fn request(plan: PersistedPlan, operator_resolution: Option<&str>) -> ControllerPlanReviewRequest {
    ControllerPlanReviewRequest::from_persisted(
        &plan,
        Some("plan-review-memory-evaluation"),
        &state(),
        operator_resolution,
    )
    .expect("evaluation request is valid")
}

fn memory_item(
    id: orc::memory::MemoryId,
    kind: orc::memory::MemoryKind,
    scope: orc::memory::MemoryScope,
    authority: ControllerMemoryAuthority,
    subject: &str,
    content: &str,
    provenance: orc::memory::MemoryProvenanceKind,
) -> ControllerMemoryItem {
    ControllerMemoryItem {
        id,
        kind,
        scope,
        authority,
        subject: subject.into(),
        content: content.into(),
        provenance: orc::memory::MemoryProvenance {
            kind: provenance,
            source_reference: Some(format!("evaluation:{subject}")),
        },
        confidence: Some(0.8),
        lifecycle: orc::memory::MemoryLifecycle::Active,
        supersedes: None,
    }
}

fn memory_context(items: Vec<ControllerMemoryItem>) -> ControllerMemoryContext {
    ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items,
    }
}

fn evaluate_case(
    name: &str,
    request: ControllerPlanReviewRequest,
    memory: ControllerMemoryContext,
    expected: ControllerPlanReviewDecision,
    runtime: &mut JsonRuntime,
) -> (bool, bool) {
    let input = ControllerPlanReviewInput::from_request(&request, memory);
    let result = ControllerPlanReviewBuilder::new().review_with_memory(&input, runtime);
    match result {
        Ok(result) => {
            let semantic_pass = result.decision == expected;
            println!(
                "scenario={name} strict_structured_output=pass semantic_authority={} observed={:?} expected={expected:?} details={:?} feedback={:?}",
                if semantic_pass { "pass" } else { "fail" },
                result.decision,
                result.details,
                result.revision_feedback,
            );
            (true, semantic_pass)
        }
        Err(error) => {
            println!(
                "scenario={name} strict_structured_output=fail semantic_authority=fail error={error}"
            );
            (false, false)
        }
    }
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF"]
fn qwen3_evaluates_controller_plan_review_memory_precedence() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let mut runtime = JsonRuntime { inner: runtime };

    let defect_objective = "Add a small health endpoint and verify it with a focused test.";
    let coherent_objective = "Add a bounded health endpoint with a focused Rust test.";
    let operator_objective =
        "Choose the deployment provider before implementing the deployment integration.";
    let cases = [
        (
            "current-plan-defect-beats-approval-memory",
            request(
                plan(
                    defect_objective,
                    vec![],
                    vec![],
                    vec![
                        "Concrete defect: this Plan contains no implementation task for the required health endpoint or focused test.",
                    ],
                    vec![],
                ),
                None,
            ),
            memory_context(vec![
                memory_item(
                    orc::memory::MemoryId::Global(1),
                    orc::memory::MemoryKind::User,
                    orc::memory::MemoryScope::Global,
                    ControllerMemoryAuthority::DurableUser,
                    "approve-small-plans",
                    "Approve small Plans without requesting changes.",
                    orc::memory::MemoryProvenanceKind::Operator,
                ),
                memory_item(
                    orc::memory::MemoryId::Global(2),
                    orc::memory::MemoryKind::Experience,
                    orc::memory::MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "historical-approval",
                    "Similar health endpoint Plans were approved in earlier projects.",
                    orc::memory::MemoryProvenanceKind::Imported,
                ),
            ]),
            ControllerPlanReviewDecision::RevisePlan,
        ),
        (
            "project-context-preserves-current-plan",
            request(
                plan(
                    coherent_objective,
                    vec![task(
                        "Add health endpoint",
                        coherent_objective,
                        &["src/health.rs", "tests/health.rs"],
                        &[
                            "GET /health returns HTTP 200.",
                            "The focused Rust test passes.",
                        ],
                    )],
                    vec!["The endpoint remains bounded to the service."],
                    vec![],
                    vec![],
                ),
                None,
            ),
            memory_context(vec![memory_item(
                orc::memory::MemoryId::Project {
                    project_id: 1,
                    id: 1,
                },
                orc::memory::MemoryKind::Project,
                orc::memory::MemoryScope::Project { project_id: 1 },
                ControllerMemoryAuthority::CurrentProject,
                "project-stack",
                "The current service is Rust and its existing project/task state remains authoritative.",
                orc::memory::MemoryProvenanceKind::ProjectFact,
            )]),
            ControllerPlanReviewDecision::Approve,
        ),
        (
            "operator-resolution-beats-old-review-history",
            request(
                plan(
                    operator_objective,
                    vec![task(
                        "Choose deployment provider",
                        operator_objective,
                        &["docs/deployment-choice.md"],
                        &["The operator's provider choice is recorded before implementation."],
                    )],
                    vec![],
                    vec![],
                    vec!["The operator must choose managed or self-hosted deployment."],
                ),
                Some(
                    "Current operator resolution: require operator_decision_required until the provider is chosen; do not approve or revise yet.",
                ),
            ),
            memory_context(vec![
                memory_item(
                    orc::memory::MemoryId::Project {
                        project_id: 1,
                        id: 2,
                    },
                    orc::memory::MemoryKind::Episodic,
                    orc::memory::MemoryScope::Project { project_id: 1 },
                    ControllerMemoryAuthority::ProjectHistory,
                    "old-approval",
                    "An earlier deployment Plan was approved without an operator choice.",
                    orc::memory::MemoryProvenanceKind::Imported,
                ),
                memory_item(
                    orc::memory::MemoryId::Global(3),
                    orc::memory::MemoryKind::Experience,
                    orc::memory::MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "old-revision",
                    "Past deployment Plans were sometimes revised directly.",
                    orc::memory::MemoryProvenanceKind::ControllerApproved,
                ),
            ]),
            ControllerPlanReviewDecision::OperatorDecisionRequired,
        ),
    ];

    let mut strict_successes = 0;
    let mut semantic_successes = 0;
    for (name, request, memory, expected) in cases {
        let (strict, semantic) = evaluate_case(name, request, memory, expected, &mut runtime);
        strict_successes += usize::from(strict);
        semantic_successes += usize::from(semantic);
    }
    println!("strict_structured_output={strict_successes}/3");
    println!("semantic_authority={semantic_successes}/3");
    assert_eq!(
        strict_successes, 3,
        "all cases must satisfy the strict schema"
    );
    assert_eq!(
        semantic_successes, 3,
        "all cases must preserve Plan-review authority precedence"
    );
}
