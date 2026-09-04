//! Opt-in real-Qwen evaluation for Controller workflow-intake memory precedence.
//!
//! This evaluation only obtains a typed intake judgment. It never routes a
//! workflow, persists a result, applies tasks or Plans, or writes memory.

#![cfg(feature = "llama-cpp")]

use orc::controller_intake::{
    ControllerIntakeBuilder, ControllerIntakeDecision, ControllerIntakeInput,
    ControllerIntakeRequest,
};
use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::discovery::{
    ArchitectureSnapshot, ProjectDiscoverySnapshot, ProjectMetadata, RepositorySnapshot,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::protocol::PlanningProjectState;
use std::collections::BTreeMap;
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

fn request(objective: &str, operator_resolution: Option<&str>) -> ControllerIntakeRequest {
    let snapshot = ProjectDiscoverySnapshot {
        repository: RepositorySnapshot {
            root: "omitted".into(),
            branch: None,
            commit: None,
            changed_files: vec![],
        },
        project: ProjectMetadata {
            name: "controller-intake-memory-evaluation".into(),
            description: Some("small Rust service".into()),
            engineering_contract: Some("Keep the objective and contract unchanged.".into()),
        },
        architecture: ArchitectureSnapshot {
            entry_points: vec!["src/main.rs".into()],
            source_directories: vec!["src".into()],
        },
        technology_stack: vec!["Rust".into()],
        important_files: vec!["Cargo.toml".into(), "src/main.rs".into()],
        manifests: vec!["Cargo.toml".into()],
        test_locations: vec!["tests/".into()],
        architecture_boundaries: vec!["src".into(), "tests".into()],
        unknowns_and_risks: vec!["The change crosses multiple boundaries.".into()],
        fingerprint: "intake-memory-evaluation".into(),
        validation_commands: vec!["cargo test --lib".into()],
        task_state: PlanningProjectState {
            task_counts: BTreeMap::new(),
            ready_tasks: vec![],
            active_tasks: vec![],
            review_tasks: vec![],
            blocked_tasks: vec![],
            usable_agents: vec![],
            busy_agents: vec![],
            quota_reserve_percent: 10,
        },
    };
    ControllerIntakeRequest::from_canonical(
        "controller-intake-memory-evaluation",
        "Keep the objective and contract unchanged.",
        objective,
        &BTreeMap::from([("canonical_fact".into(), "current fact".into())]),
        &snapshot,
        operator_resolution,
    )
    .expect("evaluation request is bounded")
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
        confidence: Some(0.8),
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

fn evaluate_case(
    name: &str,
    request: ControllerIntakeRequest,
    memory: ControllerMemoryContext,
    expected: ControllerIntakeDecision,
    runtime: &mut JsonRuntime,
) -> (bool, bool) {
    let input = ControllerIntakeInput::from_request(&request, memory);
    let result = ControllerIntakeBuilder::new().classify_with_memory(
        &input.current_request,
        input.memory,
        runtime,
    );
    match result {
        Ok(result) => {
            let semantic_pass = result.decision == expected;
            println!(
                "scenario={name} strict_structured_output=pass semantic_authority={} observed={:?} expected={expected:?} details={:?}",
                if semantic_pass { "pass" } else { "fail" },
                result.decision,
                result.details,
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
fn qwen3_evaluates_controller_intake_memory_precedence() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let mut runtime = JsonRuntime { inner: runtime };

    let cases = [
        (
            "objective-beats-direct-memory",
            request(
                "Introduce a new authentication subsystem with database migrations, API changes, compatibility handling, and integration tests. This requires decomposition, sequencing, and a supervised Plan.",
                None,
            ),
            memory_context(vec![
                memory_item(
                    MemoryId::Global(1),
                    MemoryKind::User,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::DurableUser,
                    "prefer-direct",
                    "Prefer direct tasks even when work is large.",
                    MemoryProvenanceKind::Operator,
                ),
                memory_item(
                    MemoryId::Global(2),
                    MemoryKind::Experience,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "old-direct-routing",
                    "Historical projects were often routed as direct tasks.",
                    MemoryProvenanceKind::Imported,
                ),
            ]),
            ControllerIntakeDecision::PlanRequired,
        ),
        (
            "project-context-preserves-contract",
            request(
                "This is a supervised PlanRequired request for a multi-step Rust integration. Do not route it as DirectTasks; preserve the engineering contract and return plan_required.",
                None,
            ),
            memory_context(vec![memory_item(
                MemoryId::Project {
                    project_id: 1,
                    id: 1,
                },
                MemoryKind::Project,
                MemoryScope::Project { project_id: 1 },
                ControllerMemoryAuthority::CurrentProject,
                "rust-context",
                "The project uses Rust and the current engineering contract must remain unchanged.",
                MemoryProvenanceKind::ProjectFact,
            )]),
            ControllerIntakeDecision::PlanRequired,
        ),
        (
            "operator-resolution-beats-old-routing",
            request(
                "This objective requires a supervised Plan for a multi-step repository change; no further operator choice is needed.",
                Some("The operator resolved that this intake must route to PlanRequired."),
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
                    "old-routing",
                    "An old workflow routed similar work to direct tasks.",
                    MemoryProvenanceKind::Imported,
                ),
                memory_item(
                    MemoryId::Global(3),
                    MemoryKind::Experience,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "old-decision",
                    "Past experience sometimes asked the user to decide before planning.",
                    MemoryProvenanceKind::ControllerApproved,
                ),
            ]),
            ControllerIntakeDecision::PlanRequired,
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
        "all cases must preserve intake authority precedence"
    );
}
