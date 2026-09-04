use orc::app::OrcApp;
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{MemoryDraft, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope};
use orc::storage::Database;
use orc::task::TaskPriority;
use tempfile::tempdir;

struct CaptureRuntime {
    request: Option<LocalInferenceRequest>,
}

impl LocalInferenceRuntime for CaptureRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        request.validate()?;
        self.request = Some(request.clone());
        Ok(LocalInferenceResponse::structured(
            "ignored provider output",
            serde_json::json!({
                "decision": "operator_decision",
                "rationale": "The current recovery state needs operator review.",
                "confidence": 0.5
            }),
        ))
    }
}

#[test]
fn app_recovery_uses_canonical_memory_context_without_memory_mutation() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&database_path, &registry_path).unwrap();
    let project = database.create_project("recovery memory").unwrap();
    let task = database
        .insert_task(
            project,
            "recovery-task",
            "inspect a recovery state",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    drop(database);

    let app = OrcApp::open_with_registry(&database_path, directory.path(), &registry_path).unwrap();
    let memory = app
        .memories()
        .unwrap()
        .create(&MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project {
                project_id: project,
            },
            subject: "recovery-context".into(),
            content: "The project previously diagnosed this failure as a transient issue.".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ProjectFact,
                source_reference: Some("recovery-memory-test".into()),
            },
            confidence: Some(0.9),
        })
        .unwrap();
    let before_history = app.memories().unwrap().history(&memory.id).unwrap();

    let mut runtime = CaptureRuntime { request: None };
    let result = app.recommend_recovery(&task, &mut runtime).unwrap();
    assert!(!result.validation.is_actionable());
    let prompt = runtime.request.unwrap().prompt;
    assert!(prompt.contains("\"current_request\""));
    assert!(prompt.contains("\"memory\""));
    assert!(prompt.contains("recovery-memory-test"));
    assert!(prompt.contains("Authority precedence is strict"));
    assert!(prompt.contains("exact current_request.legal_operations set"));

    let after_history = app.memories().unwrap().history(&memory.id).unwrap();
    assert_eq!(before_history, after_history);
}
