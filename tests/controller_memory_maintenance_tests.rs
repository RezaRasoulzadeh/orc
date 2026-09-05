use orc::app::OrcApp;
use orc::controller_memory_maintenance::{
    ControllerMemoryMaintenanceError, ControllerMemoryMaintenanceRequest,
    ControllerMemoryMaintenanceResult,
};
use orc::controller_memory_mutation::ControllerMemoryMutationIntent;
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::storage::Database;
use tempfile::TempDir;

struct FakeRuntime {
    response: LocalInferenceResponse,
    requests: Vec<LocalInferenceRequest>,
}

impl FakeRuntime {
    fn new(response: serde_json::Value) -> Self {
        Self {
            response: LocalInferenceResponse::structured("ignored", response),
            requests: Vec::new(),
        }
    }
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

fn open_app() -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project_id = db.create_project("maintenance-test").unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    (directory, app, project_id)
}

fn draft(project_id: i64) -> MemoryDraft {
    MemoryDraft {
        kind: MemoryKind::Project,
        scope: MemoryScope::Project { project_id },
        subject: "release-gate".into(),
        content: "Releases used to require manual approval.".into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Imported,
            source_reference: Some("history:release-gate".into()),
        },
        confidence: Some(0.7),
    }
}

fn proposal(
    operation: &str,
    target: &MemoryId,
    replacement: Option<MemoryDraft>,
) -> serde_json::Value {
    let target = serde_json::to_value(target).unwrap();
    match (operation, replacement) {
        ("remove", None) => serde_json::json!({
            "decision": "propose_mutation",
            "intent": {"operation": "remove", "target": target}
        }),
        (operation, Some(replacement)) => serde_json::json!({
            "decision": "propose_mutation",
            "intent": {"operation": operation, "target": target, "replacement": replacement}
        }),
        _ => panic!("invalid maintenance proposal fixture"),
    }
}

#[test]
fn target_resolution_and_judgment_are_read_only_until_separate_handoff() {
    let (_directory, app, project_id) = open_app();
    let target = app.memories().unwrap().create(&draft(project_id)).unwrap();
    let before = app.memories().unwrap().history(&target.id).unwrap();
    let request = ControllerMemoryMaintenanceRequest::new(
        target.id.clone(),
        vec!["The operator now requires two-person approval.".into()],
    );
    let replacement = MemoryDraft {
        content: "Releases require two-person approval.".into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Operator,
            source_reference: Some("operator:release-gate".into()),
        },
        confidence: Some(0.95),
        ..draft(project_id)
    };
    let mut runtime = FakeRuntime::new(proposal("supersede", &target.id, Some(replacement)));
    let result = app
        .judge_controller_memory_maintenance(&request, &mut runtime)
        .unwrap();
    let ControllerMemoryMaintenanceResult::ProposeMutation { intent } = result else {
        panic!("expected a maintenance proposal");
    };
    assert!(matches!(
        intent,
        ControllerMemoryMutationIntent::Supersede { .. }
    ));
    assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);
    let proposal = app.propose_controller_memory_mutation(intent).unwrap();
    assert_eq!(proposal.project_id(), project_id);
    assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);
}

#[test]
fn missing_historical_and_cross_project_targets_are_rejected_before_runtime() {
    let (_directory, app, project_id) = open_app();
    let missing = ControllerMemoryMaintenanceRequest::new(
        MemoryId::Project {
            project_id,
            id: 999,
        },
        Vec::new(),
    );
    let mut missing_runtime = FakeRuntime::new(serde_json::json!({"decision": "keep"}));
    assert!(matches!(
        app.judge_controller_memory_maintenance(&missing, &mut missing_runtime),
        Err(ControllerMemoryMaintenanceError::TargetNotFound)
    ));
    assert!(missing_runtime.requests.is_empty());

    let historical = app.memories().unwrap().create(&draft(project_id)).unwrap();
    app.memories().unwrap().remove(&historical.id).unwrap();
    let historical_request = ControllerMemoryMaintenanceRequest::new(historical.id, Vec::new());
    let mut historical_runtime = FakeRuntime::new(serde_json::json!({"decision": "keep"}));
    assert!(matches!(
        app.judge_controller_memory_maintenance(&historical_request, &mut historical_runtime),
        Err(ControllerMemoryMaintenanceError::TargetNotActive)
    ));
    assert!(historical_runtime.requests.is_empty());

    let cross_project = ControllerMemoryMaintenanceRequest::new(
        MemoryId::Project {
            project_id: project_id + 1,
            id: 1,
        },
        Vec::new(),
    );
    let mut cross_runtime = FakeRuntime::new(serde_json::json!({"decision": "keep"}));
    assert!(matches!(
        app.judge_controller_memory_maintenance(&cross_project, &mut cross_runtime),
        Err(ControllerMemoryMaintenanceError::CrossProjectTarget)
    ));
    assert!(cross_runtime.requests.is_empty());
}

#[test]
fn keep_and_invalid_maintenance_proposals_do_not_mutate_memory() {
    let (_directory, app, project_id) = open_app();
    let target = app.memories().unwrap().create(&draft(project_id)).unwrap();
    let before = app.memories().unwrap().history(&target.id).unwrap();
    let request = ControllerMemoryMaintenanceRequest::new(target.id.clone(), Vec::new());

    let mut keep_runtime = FakeRuntime::new(serde_json::json!({"decision": "keep"}));
    assert!(matches!(
        app.judge_controller_memory_maintenance(&request, &mut keep_runtime)
            .unwrap(),
        ControllerMemoryMaintenanceResult::Keep
    ));

    let mut wrong_subject = draft(project_id);
    wrong_subject.subject = "different-subject".into();
    let mut mismatch_runtime =
        FakeRuntime::new(proposal("correct", &target.id, Some(wrong_subject)));
    assert!(matches!(
        app.judge_controller_memory_maintenance(&request, &mut mismatch_runtime),
        Err(ControllerMemoryMaintenanceError::InvalidStructuredOutput(_))
    ));
    assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);
}
