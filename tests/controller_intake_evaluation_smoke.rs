//! Opt-in real-model evaluation for the bounded Controller intake seam.
//!
//! This ignored test requires a local Qwen3 GGUF and reports both strict
//! structured-contract validity and semantic outcome accuracy. It never
//! persists, applies, or routes the returned judgment.

#![cfg(feature = "llama-cpp")]

use orc::controller_intake::{
    ControllerIntakeBuilder, ControllerIntakeDecision, ControllerIntakeRequest,
};
use orc::discovery::{
    ArchitectureSnapshot, ProjectDiscoverySnapshot, ProjectMetadata, RepositorySnapshot,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
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

fn request(objective: &str) -> ControllerIntakeRequest {
    let snapshot = ProjectDiscoverySnapshot {
        repository: RepositorySnapshot {
            root: "omitted".into(),
            branch: None,
            commit: None,
            changed_files: vec![],
        },
        project: ProjectMetadata {
            name: "controller-intake-evaluation".into(),
            description: Some("small Rust service".into()),
            engineering_contract: Some("Keep changes bounded and reviewable.".into()),
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
        unknowns_and_risks: vec![],
        fingerprint: "evaluation-snapshot".into(),
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
        "controller-intake-evaluation",
        "Keep changes bounded and reviewable.",
        objective,
        &BTreeMap::new(),
        &snapshot,
        None,
    )
    .expect("evaluation request is bounded")
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF; see docs/orc-next/tasks/M05-009.md"]
fn qwen3_evaluates_all_controller_intake_outcomes() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let scenarios = [
        (
            "direct_tasks",
            "Add one focused unit test for the existing parser and document the test command.",
            ControllerIntakeDecision::DirectTasks,
        ),
        (
            "plan_required",
            "This work requires a supervised multi-step plan and must not be represented as direct tasks; no operator choice is needed. Introduce a new authentication subsystem with database migrations, API changes, compatibility handling, and integration tests.",
            ControllerIntakeDecision::PlanRequired,
        ),
        (
            "user_decision_required",
            "Choose the deployment boundary for this service: the operator must decide whether production deployment targets the existing host or a new managed platform before implementation.",
            ControllerIntakeDecision::UserDecisionRequired,
        ),
    ];
    let mut runtime = JsonRuntime { inner: runtime };
    let mut strict_passes = 0;
    let mut semantic_passes = 0;
    for (name, objective, expected) in scenarios {
        let result = ControllerIntakeBuilder::new().classify(&request(objective), &mut runtime);
        match result {
            Ok(result) => {
                strict_passes += 1;
                let semantic = result.decision == expected;
                semantic_passes += usize::from(semantic);
                println!(
                    "{name}: strict=PASS semantic={} observed={:?} expected={expected:?}",
                    if semantic { "PASS" } else { "FAIL" },
                    result.decision,
                );
            }
            Err(error) => println!("{name}: strict=FAIL semantic=FAIL error={error}"),
        }
    }
    println!("Controller intake evaluation: strict={strict_passes}/3 semantic={semantic_passes}/3");
    assert_eq!(
        strict_passes, 3,
        "all intake outputs must satisfy the strict contract"
    );
    assert_eq!(
        semantic_passes, 3,
        "all intake outcomes must match their scenarios"
    );
}
