use orc::app::OrcApp;
use orc::controller_memory_maintenance::{
    ControllerMemoryMaintenanceRequest, ControllerMemoryMaintenanceStepError,
    ControllerMemoryMaintenanceStepResult, ControllerMemoryMaintenanceStepStage,
};
use orc::controller_memory_maintenance_grant::{
    ControllerMemoryMaintenanceGrantError, ControllerMemoryMaintenanceGrantState,
};
use orc::controller_memory_mutation::ControllerMemoryMutationExecutionResult;
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
    MemoryRecord, MemoryScope,
};
use orc::storage::Database;
use tempfile::TempDir;

struct CountingRuntime {
    response: Result<LocalInferenceResponse, LocalInferenceError>,
    calls: usize,
    before_response: Option<Box<dyn FnMut() + Send>>,
}

impl CountingRuntime {
    fn response(response: serde_json::Value) -> Self {
        Self {
            response: Ok(LocalInferenceResponse::structured("maintenance", response)),
            calls: 0,
            before_response: None,
        }
    }

    fn failing(error: LocalInferenceError) -> Self {
        Self {
            response: Err(error),
            calls: 0,
            before_response: None,
        }
    }
}

impl LocalInferenceRuntime for CountingRuntime {
    fn infer(
        &mut self,
        _request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        self.calls += 1;
        if let Some(before_response) = &mut self.before_response {
            before_response();
        }
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
            source_reference: Some("controller:maintenance-step-test".into()),
        },
        confidence: Some(0.8),
    }
}

fn target(app: &OrcApp, project_id: i64, kind: MemoryKind, subject: &str) -> MemoryRecord {
    app.memories()
        .unwrap()
        .create(&draft(
            kind,
            MemoryScope::Project { project_id },
            subject,
            "initial maintenance value",
        ))
        .unwrap()
}

fn request(target: &MemoryRecord, facts: Vec<&str>) -> ControllerMemoryMaintenanceRequest {
    ControllerMemoryMaintenanceRequest::new(
        target.id.clone(),
        facts.into_iter().map(str::to_owned).collect(),
    )
}

fn proposal_response(target: &MemoryRecord, operation: &str) -> serde_json::Value {
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
                target.scope.clone(),
                &target.subject,
                &format!("{operation}d maintenance value"),
            ),
        }),
        _ => panic!("unsupported maintenance test operation"),
    };
    serde_json::json!({
        "decision": "propose_mutation",
        "intent": intent,
    })
}

fn run_proposal(
    app: &OrcApp,
    target: &MemoryRecord,
    operation: &str,
    grant: &orc::controller_memory_maintenance_grant::ControllerMemoryMaintenanceGrant,
) -> (
    ControllerMemoryMaintenanceStepResult,
    CountingRuntime,
    usize,
) {
    let before_history = app.memories().unwrap().history(&target.id).unwrap().len();
    let mut runtime = CountingRuntime::response(proposal_response(target, operation));
    let result = app.maintain_controller_memory_once(
        &request(target, vec!["operator supplied maintenance evidence"]),
        grant,
        &mut runtime,
    );
    (result, runtime, before_history)
}

#[test]
fn keep_is_one_inference_non_mutating_and_free() {
    let (_directory, app, project_id) = open_app("maintenance step keep");
    let target = target(&app, project_id, MemoryKind::Project, "keep-target");
    let before = app.memories().unwrap().history(&target.id).unwrap();
    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let mut runtime = CountingRuntime::response(serde_json::json!({"decision": "keep"}));

    assert!(matches!(
        app.maintain_controller_memory_once(&request(&target, Vec::new()), &grant, &mut runtime),
        ControllerMemoryMaintenanceStepResult::Kept
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);
}

#[test]
fn eligible_project_and_episodic_operations_use_one_canonical_mutation_each() {
    for kind in [MemoryKind::Project, MemoryKind::Episodic] {
        for operation in ["correct", "supersede", "remove"] {
            let (_directory, app, project_id) = open_app("maintenance step eligible");
            let target = target(&app, project_id, kind, operation);
            let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
            let (result, runtime, before_history) = run_proposal(&app, &target, operation, &grant);
            let ControllerMemoryMaintenanceStepResult::Mutated { result } = result else {
                panic!("eligible maintenance should mutate");
            };
            assert!(matches!(
                result.canonical_result(),
                ControllerMemoryMutationExecutionResult::Mutated { .. }
            ));
            assert_eq!(runtime.calls, 1);
            assert_eq!(grant.remaining_actions().unwrap(), 0);
            assert_eq!(
                grant.state(),
                ControllerMemoryMaintenanceGrantState::Exhausted
            );
            let after_history = app.memories().unwrap().history(&target.id).unwrap();
            let expected_history = if operation == "remove" {
                before_history
            } else {
                before_history + 1
            };
            assert_eq!(
                after_history.len(),
                expected_history,
                "{operation}: before={before_history}, after={after_history:?}"
            );
        }
    }
}

#[test]
fn malformed_and_runtime_failures_stop_before_grant_or_mutation() {
    let (_directory, app, project_id) = open_app("maintenance step inference failures");
    let target = target(&app, project_id, MemoryKind::Project, "failure-target");
    let before = app.memories().unwrap().history(&target.id).unwrap();
    let grant = app.create_controller_memory_maintenance_grant(2).unwrap();

    let mut malformed = CountingRuntime::response(serde_json::json!({"decision": "unknown"}));
    assert!(matches!(
        app.maintain_controller_memory_once(&request(&target, Vec::new()), &grant, &mut malformed),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Judgment(_)
        }
    ));
    assert_eq!(malformed.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 2);

    let mut runtime_failure =
        CountingRuntime::failing(LocalInferenceError::Backend("stopped".into()));
    assert!(matches!(
        app.maintain_controller_memory_once(
            &request(&target, Vec::new()),
            &grant,
            &mut runtime_failure
        ),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Judgment(_)
        }
    ));
    assert_eq!(runtime_failure.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 2);
    assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);
}

#[test]
fn explicit_missing_stale_and_cross_project_targets_stop_before_runtime() {
    let (_directory, app, project_id) = open_app("maintenance step target gates");
    let grant = app.create_controller_memory_maintenance_grant(3).unwrap();

    let missing = MemoryRecord {
        id: MemoryId::Project {
            project_id,
            id: 999,
        },
        kind: MemoryKind::Project,
        scope: MemoryScope::Project { project_id },
        subject: String::new(),
        content: String::new(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Imported,
            source_reference: Some("test:missing".into()),
        },
        confidence: None,
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    let mut missing_runtime = CountingRuntime::response(serde_json::json!({"decision": "keep"}));
    assert!(matches!(
        app.maintain_controller_memory_once(
            &request(&missing, Vec::new()),
            &grant,
            &mut missing_runtime
        ),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Judgment(_)
        }
    ));
    assert_eq!(missing_runtime.calls, 0);
    assert_eq!(grant.remaining_actions().unwrap(), 3);

    let stale = target(&app, project_id, MemoryKind::Project, "stale-target");
    app.memories().unwrap().remove(&stale.id).unwrap();
    let mut stale_runtime = CountingRuntime::response(serde_json::json!({"decision": "keep"}));
    assert!(matches!(
        app.maintain_controller_memory_once(
            &request(&stale, Vec::new()),
            &grant,
            &mut stale_runtime
        ),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Judgment(_)
        }
    ));
    assert_eq!(stale_runtime.calls, 0);
    assert_eq!(grant.remaining_actions().unwrap(), 3);

    let cross_project = MemoryRecord {
        id: MemoryId::Project {
            project_id: project_id + 1,
            id: 1,
        },
        ..missing
    };
    let mut cross_runtime = CountingRuntime::response(serde_json::json!({"decision": "keep"}));
    assert!(matches!(
        app.maintain_controller_memory_once(
            &request(&cross_project, Vec::new()),
            &grant,
            &mut cross_runtime
        ),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Judgment(_)
        }
    ));
    assert_eq!(cross_runtime.calls, 0);
    assert_eq!(grant.remaining_actions().unwrap(), 3);
}

#[test]
fn proposal_rejection_is_free_and_does_not_retry() {
    let (directory, app, project_id) = open_app("maintenance step proposal rejection");
    let target = target(&app, project_id, MemoryKind::Project, "proposal-target");
    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let target_id = target.id.clone();
    let database_path = directory.path().join(".orc/orc.db");
    let mut runtime = CountingRuntime::response(proposal_response(&target, "remove"));
    runtime.before_response = Some(Box::new(move || {
        let MemoryId::Project { id, .. } = target_id else {
            panic!("proposal rejection fixture requires a project target");
        };
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .execute(
                "UPDATE project_memories SET lifecycle = 'removed' WHERE id = ?1 AND project_id = ?2",
                rusqlite::params![id, project_id],
            )
            .unwrap();
    }));

    let result = app.maintain_controller_memory_once(
        &request(&target, vec!["obsolete"].into_iter().collect()),
        &grant,
        &mut runtime,
    );
    assert!(matches!(
        result,
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Proposal(_)
        }
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(
        app.memories().unwrap().history(&target.id).unwrap().len(),
        1
    );
}

#[test]
fn ineligible_create_and_global_user_experience_cannot_bypass_grant() {
    let (_directory, app, project_id) = open_app("maintenance step eligibility");
    let target = target(&app, project_id, MemoryKind::Project, "create-target");
    let grant = app.create_controller_memory_maintenance_grant(4).unwrap();

    let create_output = serde_json::json!({
        "decision": "propose_mutation",
        "intent": {
            "operation": "create",
            "draft": draft(
                MemoryKind::Project,
                MemoryScope::Project { project_id },
                "new-target",
                "must not be created",
            ),
        },
    });
    let mut create_runtime = CountingRuntime::response(create_output);
    assert!(matches!(
        app.maintain_controller_memory_once(
            &request(&target, Vec::new()),
            &grant,
            &mut create_runtime
        ),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Judgment(_)
        }
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 4);

    for kind in [MemoryKind::User, MemoryKind::Experience] {
        let global = app
            .memories()
            .unwrap()
            .create(&draft(
                kind,
                MemoryScope::Global,
                kind.as_str(),
                "global maintenance target",
            ))
            .unwrap();
        let mut runtime = CountingRuntime::response(proposal_response(&global, "remove"));
        assert!(matches!(
            app.maintain_controller_memory_once(
                &request(&global, vec!["obsolete global value"]),
                &grant,
                &mut runtime
            ),
            ControllerMemoryMaintenanceStepResult::Rejected {
                error: ControllerMemoryMaintenanceStepError::Grant(
                    ControllerMemoryMaintenanceGrantError::InvalidScope
                )
            }
        ));
        assert_eq!(runtime.calls, 1);
        assert_eq!(grant.remaining_actions().unwrap(), 4);
        assert_eq!(
            app.memories().unwrap().history(&global.id).unwrap().len(),
            1
        );
    }
}

#[test]
fn exhausted_revoked_and_wrong_project_grants_stop_without_mutation() {
    let (_directory_a, app_a, project_a) = open_app("maintenance step grant a");
    let directory_b = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory_b.path().join(".orc")).unwrap();
    let path_b = directory_b.path().join(".orc/orc.db");
    let registry_b = directory_b.path().join(".orc/global.db");
    let database_b = Database::init_with_registry(&path_b, &registry_b).unwrap();
    drop(database_b);
    rusqlite::Connection::open(&path_b)
        .unwrap()
        .execute(
            "INSERT INTO projects (id, name) VALUES (?1, ?2)",
            rusqlite::params![2_i64, "maintenance step grant b"],
        )
        .unwrap();
    let project_b = 2_i64;
    let app_b = OrcApp::open_with_registry(&path_b, directory_b.path(), &registry_b).unwrap();
    let first_target = target(&app_a, project_a, MemoryKind::Project, "grant-target");

    let exhausted = app_a.create_controller_memory_maintenance_grant(1).unwrap();
    let mut first = CountingRuntime::response(proposal_response(&first_target, "remove"));
    assert!(matches!(
        app_a.maintain_controller_memory_once(
            &request(&first_target, vec!["obsolete"]),
            &exhausted,
            &mut first
        ),
        ControllerMemoryMaintenanceStepResult::Mutated { .. }
    ));
    let second_target = target(
        &app_a,
        project_a,
        MemoryKind::Project,
        "grant-exhausted-target",
    );
    let before = app_a
        .memories()
        .unwrap()
        .history(&second_target.id)
        .unwrap()
        .len();
    let mut exhausted_runtime =
        CountingRuntime::response(proposal_response(&second_target, "remove"));
    assert!(matches!(
        app_a.maintain_controller_memory_once(
            &request(&second_target, vec!["obsolete"]),
            &exhausted,
            &mut exhausted_runtime
        ),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Grant(
                ControllerMemoryMaintenanceGrantError::Exhausted
            )
        }
    ));
    assert_eq!(exhausted_runtime.calls, 1);
    assert_eq!(
        app_a
            .memories()
            .unwrap()
            .history(&second_target.id)
            .unwrap()
            .len(),
        before
    );

    let revoked = app_a.create_controller_memory_maintenance_grant(1).unwrap();
    revoked.revoke().unwrap();
    let revoked_target = target(
        &app_a,
        project_a,
        MemoryKind::Project,
        "grant-revoked-target",
    );
    let mut revoked_runtime =
        CountingRuntime::response(proposal_response(&revoked_target, "remove"));
    assert!(matches!(
        app_a.maintain_controller_memory_once(
            &request(&revoked_target, vec!["obsolete"]),
            &revoked,
            &mut revoked_runtime
        ),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Grant(
                ControllerMemoryMaintenanceGrantError::Revoked
            )
        }
    ));
    assert_eq!(revoked_runtime.calls, 1);
    assert_eq!(revoked.remaining_actions().unwrap(), 1);

    let foreign = app_b.create_controller_memory_maintenance_grant(1).unwrap();
    let wrong_project_target = target(
        &app_a,
        project_a,
        MemoryKind::Project,
        "grant-wrong-project-target",
    );
    let mut wrong_project_runtime =
        CountingRuntime::response(proposal_response(&wrong_project_target, "remove"));
    assert!(matches!(
        app_a.maintain_controller_memory_once(
            &request(&wrong_project_target, vec!["obsolete"]),
            &foreign,
            &mut wrong_project_runtime
        ),
        ControllerMemoryMaintenanceStepResult::Rejected {
            error: ControllerMemoryMaintenanceStepError::Grant(
                ControllerMemoryMaintenanceGrantError::WrongProject { .. }
            )
        }
    ));
    assert_eq!(wrong_project_runtime.calls, 1);
    assert_eq!(foreign.remaining_actions().unwrap(), 1);
    assert_ne!(project_a, project_b);
}

#[test]
fn multi_unit_and_cloned_grants_consume_exactly_one_per_success() {
    let (_directory, app, project_id) = open_app("maintenance step shared budget");
    let grant = app.create_controller_memory_maintenance_grant(2).unwrap();
    let clone = grant.clone();
    let first = target(&app, project_id, MemoryKind::Project, "first");
    let second = target(&app, project_id, MemoryKind::Episodic, "second");

    let (first_result, first_runtime, _) = run_proposal(&app, &first, "correct", &grant);
    assert!(matches!(
        first_result,
        ControllerMemoryMaintenanceStepResult::Mutated { .. }
    ));
    assert_eq!(first_runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(clone.remaining_actions().unwrap(), 1);

    let (second_result, second_runtime, _) = run_proposal(&app, &second, "remove", &clone);
    assert!(matches!(
        second_result,
        ControllerMemoryMaintenanceStepResult::Mutated { .. }
    ));
    assert_eq!(second_runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(
        clone.state(),
        ControllerMemoryMaintenanceGrantState::Exhausted
    );
}

#[test]
fn post_mint_execution_failure_consumes_once_without_refund_or_retry() {
    let (directory, app, project_id) = open_app("maintenance step execution failure");
    let target = target(&app, project_id, MemoryKind::Project, "execution-target");
    let before = app.memories().unwrap().history(&target.id).unwrap();
    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let database_path = directory.path().join(".orc/orc.db");
    let response = proposal_response(&target, "correct");
    let mut runtime = CountingRuntime::response(response);
    runtime.before_response = Some(Box::new(move || {
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_maintenance_step BEFORE UPDATE ON project_memories
                 BEGIN SELECT RAISE(ABORT, 'maintenance step execution failure'); END;",
            )
            .unwrap();
    }));

    let result = app.maintain_controller_memory_once(
        &request(&target, vec!["corrected value"]),
        &grant,
        &mut runtime,
    );
    let ControllerMemoryMaintenanceStepResult::Rejected { error } = result else {
        panic!("post-mint execution should reject");
    };
    assert_eq!(
        error.stage(),
        ControllerMemoryMaintenanceStepStage::Execution
    );
    let ControllerMemoryMaintenanceStepError::Execution(error) = error else {
        panic!("expected execution rejection");
    };
    assert!(!matches!(
        error.canonical_result(),
        ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(app.memories().unwrap().history(&target.id).unwrap(), before);
}

#[test]
fn public_result_observes_only_matching_success_and_rejection_states() {
    let (directory, app, project_id) = open_app("maintenance step result states");
    let first_target = target(&app, project_id, MemoryKind::Project, "state-safe-target");
    let grant = app.create_controller_memory_maintenance_grant(2).unwrap();
    let (success, _, _) = run_proposal(&app, &first_target, "remove", &grant);
    let ControllerMemoryMaintenanceStepResult::Mutated { result } = success else {
        panic!("expected successful mutation");
    };
    assert!(matches!(
        result.canonical_result(),
        ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));

    let second = target(&app, project_id, MemoryKind::Project, "state-safe-failure");
    let second_grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let database_path = directory.path().join(".orc/orc.db");
    let mut runtime = CountingRuntime::response(proposal_response(&second, "remove"));
    runtime.before_response = Some(Box::new(move || {
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_maintenance_state BEFORE UPDATE ON project_memories
                 BEGIN SELECT RAISE(ABORT, 'maintenance result state failure'); END;",
            )
            .unwrap();
    }));
    let result = app.maintain_controller_memory_once(
        &request(&second, vec!["obsolete"]),
        &second_grant,
        &mut runtime,
    );
    let ControllerMemoryMaintenanceStepResult::Rejected { error } = result else {
        panic!("expected canonical execution rejection");
    };
    let ControllerMemoryMaintenanceStepError::Execution(error) = error else {
        panic!("expected execution-stage rejection");
    };
    assert!(!matches!(
        error.canonical_result(),
        ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));
}
