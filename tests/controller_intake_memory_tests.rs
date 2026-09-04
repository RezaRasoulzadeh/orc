use orc::Database;
use orc::app::OrcApp;
use orc::controller_intake::{
    ControllerIntakeDecision, ControllerIntakeRequest, ControllerIntakeResult,
};
use orc::discovery::{
    ArchitectureSnapshot, ProjectDiscoverySnapshot, ProjectMetadata, RepositorySnapshot,
};
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{MemoryDraft, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope};
use orc::protocol::PlanningProjectState;
use std::collections::BTreeMap;
use tempfile::tempdir;

struct FakeRuntime {
    response: LocalInferenceResponse,
    prompt: Option<String>,
}

impl LocalInferenceRuntime for FakeRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        self.prompt = Some(request.prompt.clone());
        Ok(self.response.clone())
    }
}

fn request() -> ControllerIntakeRequest {
    let snapshot = ProjectDiscoverySnapshot {
        repository: RepositorySnapshot {
            root: "omitted".into(),
            branch: None,
            commit: None,
            changed_files: vec![],
        },
        project: ProjectMetadata {
            name: "intake-memory-test".into(),
            description: Some("small Rust service".into()),
            engineering_contract: Some("Keep the change bounded and reviewable.".into()),
        },
        architecture: ArchitectureSnapshot::default(),
        technology_stack: vec!["Rust".into()],
        important_files: vec!["Cargo.toml".into()],
        manifests: vec!["Cargo.toml".into()],
        test_locations: vec!["tests/".into()],
        architecture_boundaries: vec!["src".into()],
        unknowns_and_risks: vec![],
        fingerprint: "intake-memory-test".into(),
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
        "intake-memory-test",
        "Keep the change bounded and reviewable.",
        "Decompose this objective into a supervised implementation plan.",
        &BTreeMap::from([("fact".into(), "canonical fact".into())]),
        &snapshot,
        Some("operator requires PlanRequired routing"),
    )
    .unwrap()
}

#[test]
fn app_intake_uses_canonical_memory_read_only_and_preserves_three_outcomes() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let db = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = db.create_project("intake-memory-test").unwrap();
    let memory = db
        .create_memory(&MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id },
            subject: "intake-guidance".into(),
            content: "Advisory context cannot rewrite the objective or operator resolution.".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ProjectFact,
                source_reference: Some("test:intake-memory".into()),
            },
            confidence: Some(0.9),
        })
        .unwrap();
    let memory_id = memory.id;
    drop(db);

    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    let before = app.memories().unwrap().history(&memory_id).unwrap();
    let mut runtime = FakeRuntime {
        response: LocalInferenceResponse::structured(
            "plan-required intake",
            serde_json::json!({
                "decision": "plan_required",
                "details": "The canonical objective requires supervised decomposition.",
                "direct_tasks": []
            }),
        ),
        prompt: None,
    };
    let result: ControllerIntakeResult = app
        .propose_controller_intake(&request(), &mut runtime)
        .unwrap();
    assert_eq!(result.decision, ControllerIntakeDecision::PlanRequired);
    assert!(result.direct_tasks.is_empty());
    let prompt = runtime.prompt.expect("captured intake prompt");
    assert!(prompt.contains("test:intake-memory"));
    assert!(prompt.contains("current_request objective, engineering_contract"));
    assert!(
        prompt.contains("current objective and explicit operator resolution are authoritative")
    );
    assert!(prompt.contains("cannot rewrite the objective or engineering contract"));
    assert!(prompt.contains("Decompose this objective into a supervised implementation plan."));
    assert!(prompt.contains("operator requires PlanRequired routing"));
    assert!(prompt.contains("exactly direct_tasks, plan_required, or user_decision_required"));

    let after = app.memories().unwrap().history(&memory_id).unwrap();
    assert_eq!(after, before);
}
