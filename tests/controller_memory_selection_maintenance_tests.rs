use orc::app::OrcApp;
use orc::controller_memory_mutation::ControllerMemoryMutationExecutionResult;
use orc::controller_memory_selection::ControllerMemorySelectionRequest;
use orc::controller_memory_selection_maintenance::ControllerMemorySelectionMaintenanceStepResult;
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryRecord,
    MemoryScope,
};
use orc::storage::Database;
use tempfile::TempDir;

struct SequenceRuntime {
    responses: Vec<Result<LocalInferenceResponse, LocalInferenceError>>,
    calls: usize,
    requests: Vec<LocalInferenceRequest>,
    before_response: Option<Box<dyn FnMut(usize) + Send>>,
}

impl SequenceRuntime {
    fn new(responses: Vec<Result<LocalInferenceResponse, LocalInferenceError>>) -> Self {
        Self {
            responses,
            calls: 0,
            requests: Vec::new(),
            before_response: None,
        }
    }

    fn json(value: serde_json::Value) -> Result<LocalInferenceResponse, LocalInferenceError> {
        Ok(LocalInferenceResponse::structured("controller", value))
    }
}

impl LocalInferenceRuntime for SequenceRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        self.calls += 1;
        self.requests.push(request.clone());
        if let Some(before_response) = &mut self.before_response {
            before_response(self.calls);
        }
        self.responses
            .get(self.calls - 1)
            .cloned()
            .unwrap_or_else(|| Err(LocalInferenceError::Backend("unexpected retry".into())))
    }
}

fn open_app(name: &str) -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project_id = db.create_project(name).unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    (directory, app, project_id)
}

fn draft(kind: MemoryKind, project_id: i64, subject: &str, content: &str) -> MemoryDraft {
    MemoryDraft {
        kind,
        scope: MemoryScope::Project { project_id },
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Imported,
            source_reference: Some("controller:selection-maintenance-test".into()),
        },
        confidence: Some(0.8),
    }
}

fn target(app: &OrcApp, project_id: i64, kind: MemoryKind, subject: &str) -> MemoryRecord {
    app.memories()
        .unwrap()
        .create(&draft(kind, project_id, subject, "initial value"))
        .unwrap()
}

fn selection_request(facts: &[&str]) -> ControllerMemorySelectionRequest {
    ControllerMemorySelectionRequest::new(facts.iter().map(|fact| (*fact).to_owned()).collect())
}

fn select(target: &MemoryId) -> serde_json::Value {
    serde_json::json!({"decision": "select_target", "target": target})
}

fn proposal(target: &MemoryRecord, operation: &str) -> serde_json::Value {
    let target_id = serde_json::to_value(&target.id).unwrap();
    let intent = match operation {
        "remove" => serde_json::json!({
            "operation": "remove",
            "target": target_id,
        }),
        "correct" | "supersede" => serde_json::json!({
            "operation": operation,
            "target": target_id,
            "replacement": draft(
                target.kind,
                target.scope.project_id().unwrap(),
                &target.subject,
                &format!("{operation}d value"),
            ),
        }),
        _ => panic!("unsupported operation"),
    };
    serde_json::json!({"decision": "propose_mutation", "intent": intent})
}

fn prompt_json(request: &LocalInferenceRequest) -> serde_json::Value {
    serde_json::from_str(request.prompt.rsplit_once("\n\n").unwrap().1).unwrap()
}

#[test]
fn no_target_and_selector_failures_stop_before_maintenance() {
    let (_directory, app, _project_id) = open_app("selection-maintenance no target");
    let grant = app.create_controller_memory_maintenance_grant(2).unwrap();
    let mut no_target = SequenceRuntime::new(vec![SequenceRuntime::json(
        serde_json::json!({"decision": "no_target"}),
    )]);
    assert!(matches!(
        app.maintain_selected_controller_memory_once(
            &selection_request(&["nothing warrants maintenance"]),
            &grant,
            &mut no_target,
        ),
        ControllerMemorySelectionMaintenanceStepResult::NoTarget
    ));
    assert_eq!(no_target.calls, 0);
    assert_eq!(grant.remaining_actions().unwrap(), 2);

    let (_directory, app, project_id) = open_app("selection-maintenance selector failure");
    let target = target(&app, project_id, MemoryKind::Project, "selector-failure");
    let before = app.memories().unwrap().history(&target.id).unwrap();
    for response in [
        Ok(LocalInferenceResponse::structured(
            "controller",
            serde_json::json!({"decision": "unknown"}),
        )),
        Ok(LocalInferenceResponse::structured(
            "controller",
            select(&MemoryId::Project {
                project_id,
                id: 999_999,
            }),
        )),
        Err(LocalInferenceError::Backend("selector stopped".into())),
    ] {
        let grant = app.create_controller_memory_maintenance_grant(2).unwrap();
        let mut runtime = SequenceRuntime::new(vec![response]);
        assert!(matches!(
            app.maintain_selected_controller_memory_once(
                &selection_request(&["selector evidence"]),
                &grant,
                &mut runtime,
            ),
            ControllerMemorySelectionMaintenanceStepResult::Rejected { .. }
        ));
        assert_eq!(runtime.calls, 1);
        assert_eq!(grant.remaining_actions().unwrap(), 2);
        assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);
    }
}

#[test]
fn selected_keep_preserves_exact_facts_and_identity_for_project_and_episodic() {
    for kind in [MemoryKind::Project, MemoryKind::Episodic] {
        let (_directory, app, project_id) = open_app("selection-maintenance keep");
        let target = target(&app, project_id, kind, "keep-target");
        let before = app.memories().unwrap().history(&target.id).unwrap();
        let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
        let mut runtime = SequenceRuntime::new(vec![
            SequenceRuntime::json(select(&target.id)),
            SequenceRuntime::json(serde_json::json!({"decision": "keep"})),
        ]);
        let request = selection_request(&["fact one", "fact two in caller order"]);
        let result = app.maintain_selected_controller_memory_once(&request, &grant, &mut runtime);
        let ControllerMemorySelectionMaintenanceStepResult::Kept { result } = result else {
            panic!("selected target should be kept");
        };
        assert_eq!(result.target(), &target.id);
        assert_eq!(grant.remaining_actions().unwrap(), 1);
        assert_eq!(runtime.calls, 2);
        assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);

        let selection_input = prompt_json(&runtime.requests[0]);
        let maintenance_input = prompt_json(&runtime.requests[1]);
        assert_eq!(
            selection_input["current_request"]["current_facts"],
            serde_json::json!(["fact one", "fact two in caller order"])
        );
        assert_eq!(
            maintenance_input["current_request"]["current_facts"],
            selection_input["current_request"]["current_facts"]
        );
        assert_eq!(
            maintenance_input["current_request"]["target"],
            serde_json::to_value(&target.id).unwrap()
        );
    }
}

#[test]
fn selected_project_and_episodic_mutations_use_one_unit_and_one_canonical_result() {
    for kind in [MemoryKind::Project, MemoryKind::Episodic] {
        for operation in ["correct", "supersede", "remove"] {
            let (_directory, app, project_id) = open_app("selection-maintenance mutation");
            let target = target(&app, project_id, kind, operation);
            let before = app.memories().unwrap().history(&target.id).unwrap().len();
            let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
            let mut runtime = SequenceRuntime::new(vec![
                SequenceRuntime::json(select(&target.id)),
                SequenceRuntime::json(proposal(&target, operation)),
            ]);
            let result = app.maintain_selected_controller_memory_once(
                &selection_request(&["explicit maintenance evidence"]),
                &grant,
                &mut runtime,
            );
            let ControllerMemorySelectionMaintenanceStepResult::Mutated { result } = result else {
                panic!("eligible selected target should mutate");
            };
            assert_eq!(result.target(), &target.id);
            assert!(matches!(
                result.maintenance_result().canonical_result(),
                ControllerMemoryMutationExecutionResult::Mutated { .. }
            ));
            assert_eq!(runtime.calls, 2);
            assert_eq!(grant.remaining_actions().unwrap(), 0);
            assert_eq!(
                app.memories().unwrap().history(&target.id).unwrap().len(),
                if operation == "remove" {
                    before
                } else {
                    before + 1
                }
            );
        }
    }
}

#[test]
fn freshness_rejection_and_grant_rejection_do_not_fallback_or_retry() {
    let (directory, app, project_id) = open_app("selection-maintenance freshness");
    let selected = target(&app, project_id, MemoryKind::Project, "freshness-target");
    let selected_id = selected.id.clone();
    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let mut runtime = SequenceRuntime::new(vec![SequenceRuntime::json(select(&selected_id))]);
    let database_path = directory.path().join(".orc/orc.db");
    runtime.before_response = Some(Box::new(move |call| {
        if call == 1 {
            let MemoryId::Project { project_id, id } = selected_id else {
                panic!("freshness fixture requires a project target");
            };
            rusqlite::Connection::open(&database_path)
                .unwrap()
                .execute(
                    "UPDATE project_memories SET lifecycle = 'removed' WHERE project_id = ?1 AND id = ?2",
                    rusqlite::params![project_id, id],
                )
                .unwrap();
        }
    }));
    let result = app.maintain_selected_controller_memory_once(
        &selection_request(&["freshness evidence"]),
        &grant,
        &mut runtime,
    );
    assert!(matches!(
        result,
        ControllerMemorySelectionMaintenanceStepResult::Rejected { .. }
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
}

#[test]
fn exhausted_revoked_and_wrong_project_grants_stop_after_selection_without_mutation() {
    let (_directory, app, project_id) = open_app("selection-maintenance grant");
    let first_target = target(&app, project_id, MemoryKind::Project, "grant-target");
    let exhausted = app.create_controller_memory_maintenance_grant(1).unwrap();
    let mut first = SequenceRuntime::new(vec![
        SequenceRuntime::json(select(&first_target.id)),
        SequenceRuntime::json(proposal(&first_target, "remove")),
    ]);
    assert!(matches!(
        app.maintain_selected_controller_memory_once(
            &selection_request(&["obsolete"]),
            &exhausted,
            &mut first,
        ),
        ControllerMemorySelectionMaintenanceStepResult::Mutated { .. }
    ));
    let next = target(&app, project_id, MemoryKind::Project, "exhausted-target");
    let before = app.memories().unwrap().history(&next.id).unwrap();
    let mut exhausted_runtime = SequenceRuntime::new(vec![
        SequenceRuntime::json(select(&next.id)),
        SequenceRuntime::json(proposal(&next, "remove")),
    ]);
    assert!(matches!(
        app.maintain_selected_controller_memory_once(
            &selection_request(&["obsolete"]),
            &exhausted,
            &mut exhausted_runtime,
        ),
        ControllerMemorySelectionMaintenanceStepResult::Rejected { .. }
    ));
    assert_eq!(exhausted_runtime.calls, 2);
    assert_eq!(exhausted.remaining_actions().unwrap(), 0);
    assert_eq!(app.memories().unwrap().history(&next.id).unwrap(), before);

    let revoked = app.create_controller_memory_maintenance_grant(1).unwrap();
    revoked.revoke().unwrap();
    let mut revoked_runtime = SequenceRuntime::new(vec![
        SequenceRuntime::json(select(&next.id)),
        SequenceRuntime::json(proposal(&next, "remove")),
    ]);
    let revoked_result = app.maintain_selected_controller_memory_once(
        &selection_request(&["obsolete"]),
        &revoked,
        &mut revoked_runtime,
    );
    assert!(matches!(
        revoked_result,
        ControllerMemorySelectionMaintenanceStepResult::Rejected { .. }
    ));
    assert_eq!(revoked_runtime.calls, 2);
    assert!(matches!(
        revoked_result,
        ControllerMemorySelectionMaintenanceStepResult::Rejected {
            error: orc::controller_memory_selection_maintenance::ControllerMemorySelectionMaintenanceStepError::Maintenance(_)
        }
    ));

    let foreign_directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(foreign_directory.path().join(".orc")).unwrap();
    let foreign_path = foreign_directory.path().join(".orc/orc.db");
    let foreign_registry = foreign_directory.path().join(".orc/global.db");
    let foreign_db = Database::init_with_registry(&foreign_path, &foreign_registry).unwrap();
    drop(foreign_db);
    rusqlite::Connection::open(&foreign_path)
        .unwrap()
        .execute(
            "INSERT INTO projects (id, name) VALUES (?1, ?2)",
            rusqlite::params![2_i64, "selection-maintenance foreign-project"],
        )
        .unwrap();
    let foreign_app =
        OrcApp::open_with_registry(&foreign_path, foreign_directory.path(), &foreign_registry)
            .unwrap();
    let foreign = foreign_app
        .create_controller_memory_maintenance_grant(1)
        .unwrap();
    let mut wrong_project = SequenceRuntime::new(vec![
        SequenceRuntime::json(select(&next.id)),
        SequenceRuntime::json(proposal(&next, "remove")),
    ]);
    let result = app.maintain_selected_controller_memory_once(
        &selection_request(&["obsolete"]),
        &foreign,
        &mut wrong_project,
    );
    assert!(matches!(
        result,
        ControllerMemorySelectionMaintenanceStepResult::Rejected { .. }
    ));
    assert_eq!(wrong_project.calls, 2);
    assert_eq!(foreign.remaining_actions().unwrap(), 1);
    assert!(!matches!(
        result,
        ControllerMemorySelectionMaintenanceStepResult::Mutated { .. }
    ));
}

#[test]
fn post_mint_failure_consumes_once_without_refund_or_retry() {
    let (directory, app, project_id) = open_app("selection-maintenance post mint");
    let target = target(&app, project_id, MemoryKind::Project, "post-mint-target");
    let before = app.memories().unwrap().history(&target.id).unwrap();
    let database_path = directory.path().join(".orc/orc.db");
    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let mut runtime = SequenceRuntime::new(vec![
        SequenceRuntime::json(select(&target.id)),
        SequenceRuntime::json(proposal(&target, "correct")),
    ]);
    runtime.before_response = Some(Box::new(move |call| {
        if call == 2 {
            rusqlite::Connection::open(&database_path)
                .unwrap()
                .execute_batch(
                    "CREATE TRIGGER fail_selection_maintenance BEFORE UPDATE ON project_memories
                     BEGIN SELECT RAISE(ABORT, 'selection maintenance execution failure'); END;",
                )
                .unwrap();
        }
    }));
    let result = app.maintain_selected_controller_memory_once(
        &selection_request(&["corrected value"]),
        &grant,
        &mut runtime,
    );
    assert!(matches!(
        result,
        ControllerMemorySelectionMaintenanceStepResult::Rejected { .. }
    ));
    assert_eq!(runtime.calls, 2);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);
}
