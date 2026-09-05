use orc::app::OrcApp;
use orc::controller_actions::ControllerActionIntent;
use orc::controller_continuation::{
    ControllerContinuationAllowedActions, ControllerContinuationGrantError,
    ControllerContinuationGrantState,
};
use orc::storage::Database;
use orc::task::{CreateTaskInput, TaskPriority, TaskScopeMode};
use tempfile::tempdir;

fn app_with_task() -> (tempfile::TempDir, OrcApp, String) {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("agents.db");
    let db = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = db.create_project("continuation-grant").unwrap();
    let task_id = db
        .create_task(
            project_id,
            &CreateTaskInput {
                title: "Continuation task".into(),
                objective: "Test bounded continuation authorization".into(),
                role: "developer".into(),
                priority: TaskPriority::Normal,
                required_capabilities: vec![],
                scope_mode: Some(TaskScopeMode::Focused),
                context_files: vec![],
                expected_changes: vec![],
                dependencies: vec![],
            },
        )
        .unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app, task_id)
}

#[test]
fn grant_mints_only_for_allowed_currently_legal_actions_and_consumes_once() {
    let (_directory, app, task_id) = app_with_task();
    let grant = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 2)
        .unwrap();

    let intent = ControllerActionIntent::SemanticReview {
        task_id: task_id.clone(),
    };
    let _first_authorization = app
        .inspect_controller_continuation_grant(&grant, &intent)
        .unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(grant.state(), ControllerContinuationGrantState::Active);

    let _second_authorization = app
        .inspect_controller_continuation_grant(&grant, &intent)
        .unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(grant.state(), ControllerContinuationGrantState::Exhausted);

    assert!(matches!(
        app.inspect_controller_continuation_grant(&grant, &intent),
        Err(ControllerContinuationGrantError::Exhausted)
    ));
}

#[test]
fn rejected_inspection_and_accept_do_not_consume_grant_budget() {
    let (_directory, app, task_id) = app_with_task();
    let grant = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();

    let accept = ControllerActionIntent::Accept {
        task_id: task_id.clone(),
    };
    assert!(matches!(
        app.inspect_controller_continuation_grant(&grant, &accept),
        Err(ControllerContinuationGrantError::UnsupportedAction(
            orc::controller_actions::ControllerActionKind::Accept
        ))
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 1);

    let malformed = ControllerActionIntent::SemanticReview {
        task_id: " ".into(),
    };
    assert!(matches!(
        app.inspect_controller_continuation_grant(&grant, &malformed),
        Err(ControllerContinuationGrantError::InvalidIntent(_))
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 1);

    let stale_or_illegal = ControllerActionIntent::Revise { task_id };
    assert!(matches!(
        app.inspect_controller_continuation_grant(&grant, &stale_or_illegal),
        Err(ControllerContinuationGrantError::CanonicallyIllegal(_))
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 1);
}

#[test]
fn revoked_and_value_copied_grants_cannot_reset_budget() {
    let (_directory, app, task_id) = app_with_task();
    let grant = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    let copied = grant.clone();
    grant.revoke().unwrap();
    assert_eq!(copied.state(), ControllerContinuationGrantState::Revoked);
    assert!(matches!(
        app.inspect_controller_continuation_grant(
            &copied,
            &ControllerActionIntent::SemanticReview { task_id }
        ),
        Err(ControllerContinuationGrantError::Revoked)
    ));
}
