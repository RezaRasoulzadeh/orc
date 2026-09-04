use orc::Database;
use orc::app::OrcApp;
use orc::controller_actions::{
    ControllerActionIntent, ControllerActionLegality, ControllerActionProposal,
};
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{MemoryDraft, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope};
use orc::task::{CreateTaskInput, TaskPriority, TaskScopeMode};
use tempfile::tempdir;

struct FakeRuntime {
    response: LocalInferenceResponse,
    requests: Vec<LocalInferenceRequest>,
}

impl LocalInferenceRuntime for FakeRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        self.requests.push(request.clone());
        Ok(self.response.clone())
    }
}

#[test]
fn normal_recommendation_reads_canonical_memory_without_mutation_or_legality_bypass() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let db = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = db.create_project("recommendation-memory").unwrap();
    let task_id = db
        .create_task(
            project_id,
            &CreateTaskInput {
                title: "Canonical current task".into(),
                objective: "Keep the current task contract authoritative".into(),
                role: "developer".into(),
                priority: TaskPriority::Normal,
                required_capabilities: vec![],
                scope_mode: Some(TaskScopeMode::Focused),
                context_files: vec![],
                expected_changes: vec!["src/controller.rs".into()],
                dependencies: vec![],
            },
        )
        .unwrap();
    let memory = db
        .create_memory(&MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id },
            subject: "recommendation-guidance".into(),
            content: "This is advisory project context, not current task truth.".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ProjectFact,
                source_reference: Some("test:normal-recommendation".into()),
            },
            confidence: Some(0.9),
        })
        .unwrap();
    let memory_id = memory.id.clone();
    drop(db);

    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    let before_history = app.memories().unwrap().history(&memory_id).unwrap();
    let mut runtime = FakeRuntime {
        response: LocalInferenceResponse::structured(
            "advisory dispatch suggestion",
            serde_json::json!({
                "suggested_next_step": "dispatch",
                "decision_class": "action",
                "rationale": "The memory is advisory; current facts and kernel legality decide actionability."
            }),
        ),
        requests: Vec::new(),
    };

    let proposal = app
        .propose_controller_action(&task_id, &mut runtime)
        .unwrap();
    assert!(matches!(
        proposal,
        ControllerActionProposal::Proposed {
            intent: ControllerActionIntent::Dispatch { .. }
        }
    ));
    let prompt = &runtime.requests[0].prompt;
    assert!(prompt.contains("test:normal-recommendation"));
    assert!(prompt.contains("current_packet project/task facts and task contract"));
    assert!(prompt.contains("always outrank contradictory memory"));
    assert!(prompt.contains("downstream deterministic Controller/kernel boundaries remain final"));
    assert!(prompt.contains("current_packet"));
    assert!(prompt.contains("Canonical current task"));

    let after_history = app.memories().unwrap().history(&memory_id).unwrap();
    assert_eq!(after_history, before_history);

    let intent = match proposal {
        ControllerActionProposal::Proposed { intent } => intent,
        _ => unreachable!(),
    };
    assert!(matches!(
        intent.inspect(&app.operations()).unwrap(),
        ControllerActionLegality::Rejected { .. }
    ));
}
