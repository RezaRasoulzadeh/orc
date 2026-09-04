//! Opt-in real-model evaluation for the read-only Controller planning seam.
//!
//! This is intentionally ignored and requires a local Qwen3 GGUF. It only
//! validates the strict typed planning contract; it never persists or applies
//! the proposed plan.

#![cfg(feature = "llama-cpp")]

use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_planning::{
    ControllerPlanResult, ControllerPlanningBuilder, ControllerPlanningInput,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::protocol::{PROTOCOL_VERSION, PlanResponseSchema, PlanningRequest};
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

fn planning_request(objective: &str, constraints: &[&str]) -> PlanningRequest {
    let mut bounded_constraints = constraints
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    bounded_constraints.extend([
        "Return exactly one task.".into(),
        "Use at most one unique item in every array field.".into(),
        "Set task risk_factors to an empty array.".into(),
        "Keep generated task text and the rationale concise.".into(),
    ]);
    PlanningRequest {
        protocol_version: PROTOCOL_VERSION,
        kind: "project_plan".into(),
        project: Some(orc::protocol::ReportProject {
            name: "controller-planning-evaluation".into(),
            repository: "omitted-from-controller-request".into(),
            branch: None,
            commit: None,
        }),
        engineering_contract: "Keep the proposal concise, bounded, and read-only.".into(),
        objective: objective.into(),
        constraints: bounded_constraints,
        target_platforms: vec![],
        stack: vec!["Rust".into()],
        non_goals: vec!["Task creation".into(), "Dispatch".into()],
        deliverables: vec!["A typed PlanResponse proposal".into()],
        definition_of_done: vec!["The proposal is bounded and reviewable.".into()],
        response_schema: PlanResponseSchema::v1(),
        role_boundaries: vec!["Controller proposes only.".into()],
        planning_constraints: vec![],
        approval_requirements: vec!["Operator approval is required.".into()],
        current_state: None,
        full_report: None,
        discovery_snapshot: None,
    }
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
            source_reference: Some(format!("evaluation:{subject}")),
        },
        confidence: Some(0.9),
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
    }
}

fn memory_context(items: Vec<ControllerMemoryItem>) -> ControllerMemoryContext {
    ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items,
    }
}

fn plan_text(result: &ControllerPlanResult) -> String {
    serde_json::to_string(&result.plan)
        .expect("serialize evaluated plan")
        .to_ascii_lowercase()
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF; see docs/orc-next/tasks/M05-001.md"]
fn qwen3_evaluates_controller_planning_memory_precedence() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let mut runtime = JsonRuntime { inner: runtime };

    let conflict_objective =
        "Plan one Rust-only change to add a GET /health endpoint without changing database schema.";
    let project_objective = "Plan adding a GET /health endpoint with a focused test.";
    let history_objective = "Plan a documentation-only update describing the existing release command; do not modify runtime or database code.";
    let scenarios = vec![
        (
            "current_constraints_override_user_experience",
            planning_request(
                conflict_objective,
                &[
                    "Use Rust only; do not use Python.",
                    "Do not change database schema.",
                ],
            ),
            memory_context(vec![
                memory_item(
                    MemoryId::Global(1),
                    MemoryKind::User,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::DurableUser,
                    "preferred-language",
                    "Prefer Python for all new endpoints.",
                    MemoryProvenanceKind::Operator,
                ),
                memory_item(
                    MemoryId::Global(2),
                    MemoryKind::Experience,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "schema-guidance",
                    "Past endpoint work benefited from replacing the database schema.",
                    MemoryProvenanceKind::ControllerApproved,
                ),
            ]),
            conflict_objective,
            1_u8,
        ),
        (
            "project_memory_supplies_context",
            planning_request(
                project_objective,
                &["Keep the change limited to the health route."],
            ),
            memory_context(vec![memory_item(
                MemoryId::Project {
                    project_id: 1,
                    id: 1,
                },
                MemoryKind::Project,
                MemoryScope::Project { project_id: 1 },
                ControllerMemoryAuthority::CurrentProject,
                "http-layout",
                "HTTP routes live in src/http.rs and route tests live in tests/http.rs.",
                MemoryProvenanceKind::ProjectFact,
            )]),
            project_objective,
            2_u8,
        ),
        (
            "history_remains_guidance",
            planning_request(
                history_objective,
                &[
                    "Change documentation only.",
                    "Do not add migration or runtime work.",
                ],
            ),
            memory_context(vec![
                memory_item(
                    MemoryId::Project {
                        project_id: 1,
                        id: 2,
                    },
                    MemoryKind::Episodic,
                    MemoryScope::Project { project_id: 1 },
                    ControllerMemoryAuthority::ProjectHistory,
                    "prior-release",
                    "A previous release failed during a database migration.",
                    MemoryProvenanceKind::Imported,
                ),
                memory_item(
                    MemoryId::Global(3),
                    MemoryKind::Experience,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "migration-testing",
                    "Migration work should include rollback tests.",
                    MemoryProvenanceKind::ControllerApproved,
                ),
            ]),
            history_objective,
            3_u8,
        ),
    ];

    let mut strict_successes = 0;
    let mut semantic_successes = 0;
    for (name, request, memory, expected_objective, semantic_case) in scenarios {
        let input = ControllerPlanningInput::from_canonical(&request, memory)
            .expect("evaluation input is bounded");
        let result: Result<ControllerPlanResult, _> =
            ControllerPlanningBuilder::new().propose_with_memory(&input, &mut runtime);
        match result {
            Ok(result) => {
                strict_successes += 1;
                let text = plan_text(&result);
                let objective_preserved = result.plan.objective == expected_objective;
                let semantic_pass = match semantic_case {
                    1 => {
                        objective_preserved
                            && !result.plan.tasks.iter().any(|task| {
                                task.context_files.iter().any(|path| path.ends_with(".py"))
                                    || task
                                        .expected_changes
                                        .iter()
                                        .any(|change| change.contains(".py"))
                            })
                            && !result.plan.tasks.iter().any(|task| {
                                let task_text = format!("{} {}", task.title, task.objective)
                                    .to_ascii_lowercase();
                                task_text.contains("schema migration")
                                    || task_text.contains("replace the database")
                            })
                    }
                    2 => {
                        objective_preserved
                            && (text.contains("src/http.rs") || text.contains("tests/http.rs"))
                    }
                    3 => {
                        objective_preserved
                            && !result.plan.tasks.iter().any(|task| {
                                task.context_files.iter().any(|path| path.ends_with(".rs"))
                                    || task
                                        .expected_changes
                                        .iter()
                                        .any(|change| change.ends_with(".rs"))
                                    || format!("{} {}", task.title, task.objective)
                                        .to_ascii_lowercase()
                                        .contains("migration")
                            })
                    }
                    _ => unreachable!(),
                };
                semantic_successes += usize::from(semantic_pass);
                println!(
                    "scenario={name} strict_structured_output=pass semantic_precedence={} plan={}",
                    if semantic_pass { "pass" } else { "fail" },
                    serde_json::to_string(&result.plan).expect("serialize plan")
                );
            }
            Err(error) => println!(
                "scenario={name} strict_structured_output=fail semantic_precedence=fail error={error}"
            ),
        }
    }
    println!("strict_structured_output={strict_successes}/3");
    println!("semantic_precedence={semantic_successes}/3");
    assert_eq!(strict_successes, 3, "all scenarios must satisfy the schema");
    assert_eq!(
        semantic_successes, 3,
        "all scenarios must preserve memory authority precedence"
    );
}
