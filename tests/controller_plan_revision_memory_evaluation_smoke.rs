//! Opt-in real-Qwen evaluation for Controller Plan-revision memory precedence.
//!
//! The evaluation exercises only bounded read-only revision generation. It
//! never persists a revision, selects lineage, authorizes, or applies a Plan.

#![cfg(feature = "llama-cpp")]

use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_plan_revision::{
    ControllerPlanRevisionBuilder, ControllerPlanRevisionInput, ControllerPlanRevisionRequest,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::protocol::{
    ExecutionHints, PROTOCOL_VERSION, PlanResponse, PlanResponseSchema, PlanningProjectState,
    PlanningRequest, TaskProposal,
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
    local_id: &str,
    title: &str,
    objective: &str,
    expected_changes: &[&str],
    acceptance: &[&str],
) -> TaskProposal {
    TaskProposal {
        local_id: local_id.into(),
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
        execution_hints: ExecutionHints {
            class: None,
            model: None,
            effort: Some("low".into()),
            effort_reason: Some("bounded revision evaluation".into()),
        },
        risk_factors: Vec::new(),
    }
}

fn plan(objective: &str, tasks: Vec<TaskProposal>) -> PersistedPlan {
    PersistedPlan {
        id: 7,
        project_id: 1,
        version: 1,
        parent_plan_id: None,
        provenance: PlanProvenance::controller(),
        status: PlanStatus::RevisionRequested,
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

fn planning_request() -> PlanningRequest {
    PlanningRequest {
        protocol_version: PROTOCOL_VERSION,
        kind: "project_plan_revision".into(),
        project: Some(orc::protocol::ReportProject {
            name: "controller-plan-revision-memory-evaluation".into(),
            repository: "omitted-from-controller-request".into(),
            branch: None,
            commit: None,
        }),
        engineering_contract: "Keep the revision bounded and read-only.".into(),
        objective: "Revise one bounded engineering Plan.".into(),
        constraints: vec!["Preserve the current Plan objective and constraints.".into()],
        target_platforms: Vec::new(),
        stack: vec!["Rust".into()],
        non_goals: vec!["Plan persistence".into(), "Task application".into()],
        deliverables: vec!["A validated PlanResponse revision.".into()],
        definition_of_done: vec!["The revised Plan addresses persisted feedback.".into()],
        response_schema: PlanResponseSchema::v1(),
        role_boundaries: vec!["Controller proposes only.".into()],
        planning_constraints: Vec::new(),
        approval_requirements: vec!["Operator review is required.".into()],
        current_state: Some(PlanningProjectState {
            task_counts: [("ready".into(), 1)].into_iter().collect(),
            ready_tasks: Vec::new(),
            active_tasks: Vec::new(),
            review_tasks: Vec::new(),
            blocked_tasks: Vec::new(),
            usable_agents: vec!["bounded-agent".into()],
            busy_agents: Vec::new(),
            quota_reserve_percent: 10,
        }),
        full_report: None,
        discovery_snapshot: None,
    }
}

fn request(plan: PersistedPlan, feedback: &str) -> ControllerPlanRevisionRequest {
    ControllerPlanRevisionRequest::from_canonical(&plan, feedback, &planning_request())
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
    request: ControllerPlanRevisionRequest,
    memory: ControllerMemoryContext,
    expected_objective: &str,
    expected_task_fragment: &str,
    runtime: &mut JsonRuntime,
) -> (bool, bool) {
    let input = ControllerPlanRevisionInput::from_request(&request, memory);
    let result = ControllerPlanRevisionBuilder::new().revise_with_memory(&input, runtime);
    match result {
        Ok(plan) => {
            let semantic_pass = plan.objective == expected_objective
                && plan.tasks.iter().any(|task| {
                    let task_text = format!(
                        "{} {} {} {}",
                        task.local_id,
                        task.title,
                        task.objective,
                        task.acceptance_criteria.join(" "),
                    )
                    .to_ascii_lowercase();
                    task_text.contains(&expected_task_fragment.to_ascii_lowercase())
                });
            println!(
                "scenario={name} strict_structured_output=pass semantic_authority={} objective_preserved={} feedback_task_present={} tasks={:?}",
                if semantic_pass { "pass" } else { "fail" },
                plan.objective == expected_objective,
                plan.tasks.iter().any(|task| {
                    format!(
                        "{} {} {} {}",
                        task.local_id,
                        task.title,
                        task.objective,
                        task.acceptance_criteria.join(" "),
                    )
                    .to_ascii_lowercase()
                    .contains(&expected_task_fragment.to_ascii_lowercase())
                }),
                plan.tasks
                    .iter()
                    .map(|task| task.local_id.as_str())
                    .collect::<Vec<_>>(),
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
fn qwen3_evaluates_controller_plan_revision_memory_precedence() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let mut runtime = JsonRuntime { inner: runtime };

    let objective = "Add a health endpoint and verify it with a focused test.";
    let test_feedback = "Add a dedicated task with local_id health-endpoint-test, title Health endpoint test, and an acceptance criterion that GET /health returns HTTP 200.";
    let migration_objective = "Add a database migration guard and verify startup safety.";
    let migration_feedback =
        "Add a dedicated migration-check task that verifies the migration guard during startup.";
    let cases = [
        (
            "persisted-feedback-beats-approval-memory",
            request(
                plan(
                    objective,
                    vec![task(
                        "health-endpoint",
                        "Add health endpoint",
                        objective,
                        &["src/health.rs"],
                        &["GET /health returns HTTP 200."],
                    )],
                ),
                test_feedback,
            ),
            memory_context(vec![
                memory_item(
                    orc::memory::MemoryId::Global(1),
                    orc::memory::MemoryKind::User,
                    orc::memory::MemoryScope::Global,
                    ControllerMemoryAuthority::DurableUser,
                    "approve-old-plan",
                    "Approve the existing Plan; do not request additional tasks.",
                    orc::memory::MemoryProvenanceKind::Operator,
                ),
                memory_item(
                    orc::memory::MemoryId::Global(2),
                    orc::memory::MemoryKind::Experience,
                    orc::memory::MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "historical-approval",
                    "Similar endpoint Plans were approved without test tasks.",
                    orc::memory::MemoryProvenanceKind::Imported,
                ),
            ]),
            objective,
            "health-endpoint-test",
        ),
        (
            "project-context-preserves-plan-objective",
            request(
                plan(
                    objective,
                    vec![task(
                        "health-endpoint",
                        "Add health endpoint",
                        objective,
                        &["src/health.rs"],
                        &["GET /health returns HTTP 200."],
                    )],
                ),
                "Add a focused Rust test task that verifies GET /health returns HTTP 200.",
            ),
            memory_context(vec![memory_item(
                orc::memory::MemoryId::Project {
                    project_id: 1,
                    id: 1,
                },
                orc::memory::MemoryKind::Project,
                orc::memory::MemoryScope::Project { project_id: 1 },
                ControllerMemoryAuthority::CurrentProject,
                "project-rust-context",
                "The service uses Rust and the current Plan objective and project constraints remain authoritative.",
                orc::memory::MemoryProvenanceKind::ProjectFact,
            )]),
            objective,
            "test",
        ),
        (
            "current-feedback-beats-obsolete-history",
            request(
                plan(
                    migration_objective,
                    vec![task(
                        "migration-guard",
                        "Add migration guard",
                        migration_objective,
                        &["src/migrations.rs"],
                        &["Startup checks the migration guard."],
                    )],
                ),
                migration_feedback,
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
                    "obsolete-docs-revision",
                    "An older review requested documentation changes instead of migration verification.",
                    orc::memory::MemoryProvenanceKind::Imported,
                ),
                memory_item(
                    orc::memory::MemoryId::Global(3),
                    orc::memory::MemoryKind::Experience,
                    orc::memory::MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "obsolete-revision",
                    "Past migration Plans often added documentation tasks first.",
                    orc::memory::MemoryProvenanceKind::ControllerApproved,
                ),
            ]),
            migration_objective,
            "migration-check",
        ),
    ];

    let mut strict_successes = 0;
    let mut semantic_successes = 0;
    for (name, request, memory, objective, task_fragment) in cases {
        let (strict, semantic) = evaluate_case(
            name,
            request,
            memory,
            objective,
            task_fragment,
            &mut runtime,
        );
        strict_successes += usize::from(strict);
        semantic_successes += usize::from(semantic);
    }
    println!("strict_structured_output={strict_successes}/3");
    println!("semantic_authority={semantic_successes}/3");
    assert_eq!(
        strict_successes, 3,
        "all revisions must satisfy PlanResponse"
    );
    assert_eq!(
        semantic_successes, 3,
        "all revisions must preserve feedback and current-fact authority"
    );
}
