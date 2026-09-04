use orc::app::OrcApp;
use orc::controller_memory_capture::{
    ControllerMemoryCaptureCandidate, ControllerMemoryCaptureRequest, ControllerMemoryCaptureResult,
};
use orc::controller_memory_mutation::ControllerMemoryMutationIntent;
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{MemoryDraft, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope};
use orc::storage::Database;
use tempfile::TempDir;

struct FakeRuntime {
    response: LocalInferenceResponse,
}

impl LocalInferenceRuntime for FakeRuntime {
    fn infer(
        &mut self,
        _request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        Ok(self.response.clone())
    }
}

fn open_app() -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project_id = db.create_project("capture-test").unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    (directory, app, project_id)
}

fn candidate(project_id: i64) -> ControllerMemoryCaptureCandidate {
    ControllerMemoryCaptureCandidate {
        draft: MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id },
            subject: "release-gate".into(),
            content: "Production releases require an operator approval checklist.".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ProjectFact,
                source_reference: Some("operator:release-decision".into()),
            },
            confidence: Some(0.9),
        },
        source_facts: vec!["Explicit current-project operator decision.".into()],
    }
}

#[test]
fn capture_judgment_is_read_only_and_handoff_reuses_m06_009_legality() {
    let (_directory, app, project_id) = open_app();
    let request = ControllerMemoryCaptureRequest::from_candidate(candidate(project_id));
    let response = serde_json::json!({
        "decision": "propose_mutation",
        "intent": {
            "operation": "create",
            "draft": request.candidate.draft
        }
    });
    let mut runtime = FakeRuntime {
        response: LocalInferenceResponse::structured("ignored", response),
    };
    let result = app
        .judge_controller_memory_capture(&request, &mut runtime)
        .unwrap();
    let ControllerMemoryCaptureResult::ProposeMutation { intent } = result else {
        panic!("expected a proposed mutation");
    };
    assert!(matches!(
        intent,
        ControllerMemoryMutationIntent::Create { .. }
    ));
    assert!(
        app.memories()
            .unwrap()
            .list(Some(MemoryKind::Project), false)
            .unwrap()
            .is_empty()
    );

    let proposal = app.propose_controller_memory_mutation(intent).unwrap();
    assert_eq!(proposal.project_id(), project_id);
    assert!(
        app.memories()
            .unwrap()
            .list(Some(MemoryKind::Project), false)
            .unwrap()
            .is_empty()
    );

    let mut cross_project = candidate(project_id + 1);
    cross_project.draft.content = "outside project".into();
    let cross_request = ControllerMemoryCaptureRequest::from_candidate(cross_project.clone());
    let cross_response = serde_json::json!({
        "decision": "propose_mutation",
        "intent": {"operation": "create", "draft": cross_project.draft}
    });
    let mut cross_runtime = FakeRuntime {
        response: LocalInferenceResponse::structured("ignored", cross_response),
    };
    let ControllerMemoryCaptureResult::ProposeMutation { intent } = app
        .judge_controller_memory_capture(&cross_request, &mut cross_runtime)
        .unwrap()
    else {
        panic!("expected a cross-project proposal for handoff rejection");
    };
    assert!(app.propose_controller_memory_mutation(intent).is_err());
    assert!(
        app.memories()
            .unwrap()
            .list(Some(MemoryKind::Project), false)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn ignore_capture_preserves_existing_memory_history() {
    let (_directory, app, project_id) = open_app();
    let memory = app
        .memories()
        .unwrap()
        .create(&MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id },
            subject: "duplicate".into(),
            content: "already durable".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ProjectFact,
                source_reference: Some("operator:existing".into()),
            },
            confidence: Some(0.8),
        })
        .unwrap();
    let before = app.memories().unwrap().history(&memory.id).unwrap();
    let request = ControllerMemoryCaptureRequest::from_candidate(candidate(project_id));
    let mut runtime = FakeRuntime {
        response: LocalInferenceResponse::structured(
            "ignored",
            serde_json::json!({"decision": "ignore"}),
        ),
    };
    assert!(matches!(
        app.judge_controller_memory_capture(&request, &mut runtime)
            .unwrap(),
        ControllerMemoryCaptureResult::Ignore
    ));
    assert_eq!(app.memories().unwrap().history(&memory.id).unwrap(), before);
}
