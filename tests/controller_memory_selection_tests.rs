use orc::app::OrcApp;
use orc::controller_memory_maintenance_grant::ControllerMemoryMaintenanceGrantState;
use orc::controller_memory_selection::{
    ControllerMemorySelectionResult, MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES,
    MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES, MAX_CONTROLLER_MEMORY_SELECTION_PROMPT_BYTES,
};
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::storage::Database;
use tempfile::TempDir;

struct CountingRuntime {
    response: Result<LocalInferenceResponse, LocalInferenceError>,
    calls: usize,
    requests: Vec<LocalInferenceRequest>,
}

impl CountingRuntime {
    fn response(value: serde_json::Value) -> Self {
        Self {
            response: Ok(LocalInferenceResponse::structured("selection", value)),
            calls: 0,
            requests: Vec::new(),
        }
    }

    fn failing(error: LocalInferenceError) -> Self {
        Self {
            response: Err(error),
            calls: 0,
            requests: Vec::new(),
        }
    }
}

impl LocalInferenceRuntime for CountingRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        self.calls += 1;
        self.requests.push(request.clone());
        self.response.clone()
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

fn draft(kind: MemoryKind, scope: MemoryScope, subject: &str, content: &str) -> MemoryDraft {
    MemoryDraft {
        kind,
        scope,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Imported,
            source_reference: Some(format!("selection:{subject}")),
        },
        confidence: Some(0.8),
    }
}

fn project_target(app: &OrcApp, project_id: i64, kind: MemoryKind, subject: &str) -> MemoryId {
    app.memories()
        .unwrap()
        .create(&draft(
            kind,
            MemoryScope::Project { project_id },
            subject,
            "candidate content",
        ))
        .unwrap()
        .id
}

fn input_from_prompt(runtime: &CountingRuntime) -> serde_json::Value {
    let prompt = &runtime.requests[0].prompt;
    let input = prompt
        .rsplit_once("\n\n")
        .expect("selector prompt contains serialized input")
        .1;
    serde_json::from_str(input).expect("selector input is valid JSON")
}

fn select(target: &MemoryId) -> serde_json::Value {
    serde_json::json!({"decision": "select_target", "target": target})
}

#[test]
fn empty_eligible_set_returns_no_target_without_inference_or_mutation() {
    let (_directory, app, project_id) = open_app("selection empty");
    let global_user = app
        .memories()
        .unwrap()
        .create(&draft(
            MemoryKind::User,
            MemoryScope::Global,
            "user-preference",
            "global user memory",
        ))
        .unwrap();
    let before = app.memories().unwrap().history(&global_user.id).unwrap();
    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let mut runtime = CountingRuntime::response(serde_json::json!({"decision": "no_target"}));

    assert!(matches!(
        app.select_controller_memory_target(
            &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(Vec::new()),
            &mut runtime,
        ),
        Ok(ControllerMemorySelectionResult::NoTarget)
    ));
    assert_eq!(runtime.calls, 0);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(grant.state(), ControllerMemoryMaintenanceGrantState::Active);
    assert_eq!(
        app.memories().unwrap().history(&global_user.id).unwrap(),
        before
    );
    assert_eq!(project_id, 1);
}

#[test]
fn project_and_episodic_targets_can_be_selected_exactly() {
    for kind in [MemoryKind::Project, MemoryKind::Episodic] {
        let (_directory, app, project_id) = open_app("selection eligible");
        let target = project_target(&app, project_id, kind, kind.as_str());
        let mut runtime = CountingRuntime::response(select(&target));
        let result = app
            .select_controller_memory_target(
                &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(vec![
                    format!("The selected {kind:?} record clearly warrants review."),
                ]),
                &mut runtime,
            )
            .unwrap();
        assert_eq!(
            result,
            ControllerMemorySelectionResult::SelectTarget { target }
        );
        assert_eq!(runtime.calls, 1);
    }
}

#[test]
fn mixed_candidates_have_deterministic_project_then_episodic_order() {
    let (_directory, app, project_id) = open_app("selection ordering");
    let project_one = project_target(&app, project_id, MemoryKind::Project, "project-one");
    let episodic_one = project_target(&app, project_id, MemoryKind::Episodic, "episodic-one");
    let project_two = project_target(&app, project_id, MemoryKind::Project, "project-two");
    let mut runtime = CountingRuntime::response(serde_json::json!({"decision": "no_target"}));

    assert!(matches!(
        app.select_controller_memory_target(
            &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(Vec::new()),
            &mut runtime,
        ),
        Ok(ControllerMemorySelectionResult::NoTarget)
    ));
    let input = input_from_prompt(&runtime);
    let candidates = input["candidates"].as_array().unwrap();
    let ids: Vec<MemoryId> = candidates
        .iter()
        .map(|candidate| serde_json::from_value(candidate["id"].clone()).unwrap())
        .collect();
    assert_eq!(ids, vec![project_one, project_two, episodic_one]);
}

#[test]
fn global_user_experience_historical_and_cross_project_records_are_excluded() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project_id = db.create_project("selection isolation").unwrap();
    let other_project = db.create_project("selection other").unwrap();
    let historical = db
        .create_memory(&draft(
            MemoryKind::Project,
            MemoryScope::Project { project_id },
            "historical",
            "historical record",
        ))
        .unwrap();
    db.remove_memory(&historical.id).unwrap();
    db.create_memory(&draft(
        MemoryKind::Project,
        MemoryScope::Project {
            project_id: other_project,
        },
        "cross-project",
        "other project record",
    ))
    .unwrap();
    db.create_memory(&draft(
        MemoryKind::User,
        MemoryScope::Global,
        "user",
        "global user record",
    ))
    .unwrap();
    db.create_memory(&draft(
        MemoryKind::Experience,
        MemoryScope::Global,
        "experience",
        "global experience record",
    ))
    .unwrap();
    db.create_memory(&draft(
        MemoryKind::Project,
        MemoryScope::Project { project_id },
        "eligible",
        "current project record",
    ))
    .unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    let mut runtime = CountingRuntime::response(serde_json::json!({"decision": "no_target"}));
    app.select_controller_memory_target(
        &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(Vec::new()),
        &mut runtime,
    )
    .unwrap();
    let input = input_from_prompt(&runtime);
    assert_eq!(input["eligible_candidate_count"], 1);
    assert_eq!(input["selected_candidate_count"], 1);
    assert_eq!(input["omitted_candidate_count"], 0);
}

#[test]
fn candidate_count_bound_is_deterministic_and_observable() {
    let (_directory, app, project_id) = open_app("selection count bound");
    for index in 0..(MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES + 2) {
        project_target(
            &app,
            project_id,
            MemoryKind::Project,
            &format!("candidate-{index}"),
        );
    }
    let mut runtime = CountingRuntime::response(serde_json::json!({"decision": "no_target"}));
    app.select_controller_memory_target(
        &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(Vec::new()),
        &mut runtime,
    )
    .unwrap();
    let input = input_from_prompt(&runtime);
    assert_eq!(input["eligible_candidate_count"], 10);
    assert_eq!(
        input["selected_candidate_count"],
        MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES
    );
    assert_eq!(input["omitted_candidate_count"], 2);
}

#[test]
fn input_byte_bound_omission_is_deterministic_and_observable() {
    let (_directory, app, project_id) = open_app("selection byte bound");
    for index in 0..MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES {
        let record = draft(
            MemoryKind::Project,
            MemoryScope::Project { project_id },
            &format!("large-{index}"),
            &"bounded candidate content ".repeat(200),
        );
        app.memories().unwrap().create(&record).unwrap();
    }
    let mut runtime = CountingRuntime::response(serde_json::json!({"decision": "no_target"}));
    app.select_controller_memory_target(
        &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(Vec::new()),
        &mut runtime,
    )
    .unwrap();
    let input = input_from_prompt(&runtime);
    assert_eq!(
        input["eligible_candidate_count"],
        MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES
    );
    assert!(
        input["selected_candidate_count"].as_u64().unwrap()
            < MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES as u64
    );
    assert!(input["omitted_candidate_count"].as_u64().unwrap() > 0);
    let serialized = serde_json::to_vec(&input).unwrap();
    assert!(serialized.len() <= MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES);
    assert!(runtime.requests[0].prompt.len() <= MAX_CONTROLLER_MEMORY_SELECTION_PROMPT_BYTES);
}

#[test]
fn valid_no_target_uses_one_inference_and_is_read_only() {
    let (_directory, app, project_id) = open_app("selection no target");
    let target = project_target(&app, project_id, MemoryKind::Project, "ambiguous");
    let before = app.memories().unwrap().history(&target).unwrap();
    let mut runtime = CountingRuntime::response(serde_json::json!({"decision": "no_target"}));
    let result = app
        .select_controller_memory_target(
            &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(vec![
                "The evidence is ambiguous and unrelated to a clear maintenance need.".into(),
            ]),
            &mut runtime,
        )
        .unwrap();
    assert_eq!(result, ControllerMemorySelectionResult::NoTarget);
    assert_eq!(runtime.calls, 1);
    assert_eq!(app.memories().unwrap().history(&target).unwrap(), before);
}

#[test]
fn invalid_outputs_are_rejected_and_never_mutate_memory() {
    let (_directory, app, project_id) = open_app("selection invalid output");
    let target = project_target(&app, project_id, MemoryKind::Project, "valid-target");
    let historical = project_target(&app, project_id, MemoryKind::Project, "historical-target");
    app.memories().unwrap().remove(&historical).unwrap();
    let cases = vec![
        serde_json::json!({"decision": "select_target", "target": {"Project": {"project_id": project_id, "id": 9999}}}),
        serde_json::json!({"decision": "select_target", "target": {"Global": 1}}),
        serde_json::json!({"decision": "select_target", "target": {"Project": {"project_id": project_id + 1, "id": 1}}}),
        serde_json::json!({"decision": "select_target", "target": historical}),
        serde_json::json!({"decision": "no_target", "extra": true}),
        serde_json::json!({"decision": "select_target", "target": target, "targets": []}),
    ];
    for value in cases {
        let mut runtime = CountingRuntime::response(value);
        let result = app.select_controller_memory_target(
            &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(Vec::new()),
            &mut runtime,
        );
        assert!(result.is_err());
        assert_eq!(runtime.calls, 1);
    }
}

#[test]
fn omitted_target_is_rejected_without_a_second_inference() {
    let (_directory, app, project_id) = open_app("selection omitted target");
    for index in 0..(MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES + 1) {
        project_target(
            &app,
            project_id,
            MemoryKind::Project,
            &format!("candidate-{index}"),
        );
    }
    let mut runtime = CountingRuntime::response(serde_json::json!({
        "decision": "select_target",
        "target": {"Project": {"project_id": project_id, "id": 9}}
    }));
    let result = app.select_controller_memory_target(
        &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(Vec::new()),
        &mut runtime,
    );
    assert!(result.is_err());
    assert_eq!(runtime.calls, 1);
}

#[test]
fn runtime_failure_and_invalid_request_stop_without_a_second_call() {
    let (_directory, app, project_id) = open_app("selection runtime failure");
    project_target(&app, project_id, MemoryKind::Project, "runtime-target");
    let mut runtime = CountingRuntime::failing(LocalInferenceError::Backend("stopped".into()));
    let result = app.select_controller_memory_target(
        &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(Vec::new()),
        &mut runtime,
    );
    assert!(result.is_err());
    assert_eq!(runtime.calls, 1);

    let mut invalid_runtime =
        CountingRuntime::response(serde_json::json!({"decision": "no_target"}));
    let invalid = orc::controller_memory_selection::ControllerMemorySelectionRequest {
        packet_version: 99,
        current_facts: Vec::new(),
    };
    assert!(
        app.select_controller_memory_target(&invalid, &mut invalid_runtime)
            .is_err()
    );
    assert_eq!(invalid_runtime.calls, 0);
}

#[test]
fn selection_never_consumes_maintenance_grant_or_mutates_history() {
    let (_directory, app, project_id) = open_app("selection boundary");
    let target = project_target(&app, project_id, MemoryKind::Episodic, "boundary-target");
    let before = app.memories().unwrap().history(&target).unwrap();
    let grant = app.create_controller_memory_maintenance_grant(2).unwrap();
    let mut runtime = CountingRuntime::response(select(&target));
    assert!(matches!(
        app.select_controller_memory_target(
            &orc::controller_memory_selection::ControllerMemorySelectionRequest::new(vec![
                "The explicit fact identifies this target for maintenance review.".into(),
            ]),
            &mut runtime,
        ),
        Ok(ControllerMemorySelectionResult::SelectTarget { .. })
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 2);
    assert_eq!(app.memories().unwrap().history(&target).unwrap(), before);
}
