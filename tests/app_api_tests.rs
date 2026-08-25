use orc::app::OrcApp;
use orc::protocol::{PROTOCOL_VERSION, PlanResponse, PlannedTask};
use orc::storage::Database;
use orc::task::TaskPriority;
use rusqlite::Connection;
use std::sync::mpsc::TryRecvError;
use tempfile::tempdir;

fn app_with_task(name: &str) -> (tempfile::TempDir, OrcApp, String) {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project(name).unwrap();
    if name == "two" {
        db.insert_task(
            project,
            "other",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    }
    let task = db
        .insert_task(
            project,
            name,
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&task, orc::task::TaskStatus::Active)
        .unwrap();
    db.create_agent_run(project, &task, "agent").unwrap();
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    (directory, app, task)
}

#[test]
fn app_instances_are_isolated_for_queries_and_mutations() {
    let (_one_dir, one, one_task) = app_with_task("one");
    let (_two_dir, two, two_task) = app_with_task("two");

    one.cancel(&one_task, None).unwrap();
    two.cancel(&two_task, None).unwrap();
    assert_eq!(
        one.task(&one_task).unwrap().unwrap().status.to_string(),
        "cancelled"
    );
    assert_eq!(
        two.task(&two_task).unwrap().unwrap().status.to_string(),
        "cancelled"
    );
}

#[test]
fn app_plan_uses_database_validation() {
    let (_directory, app, _task) = app_with_task("plan");
    let invalid = PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: "objective".into(),
        tasks: vec![PlannedTask {
            local_id: "duplicate".into(),
            title: "one".into(),
            objective: "objective".into(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            depends_on: vec!["missing".into()],
            capabilities: vec![],
            scope_mode: None,
            context_files: vec![],
            expected_changes: vec![],
        }],
        assumptions: vec![],
        risks: vec![],
        questions: vec![],
    };
    assert!(app.apply_plan(&invalid).is_err());
}

#[test]
fn blocked_failed_task_can_be_requeued_without_losing_run_history() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    let db = Database::init(&database).unwrap();
    let project = db.create_project("recovery").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let run = db.create_agent_run(project, &task, "agent").unwrap();
    db.update_agent_run_status(run, "failed", Some("validation failed"))
        .unwrap();
    db.update_task_status(&task, orc::task::TaskStatus::Blocked)
        .unwrap();

    let app = OrcApp::open(&database, directory.path()).unwrap();
    app.requeue(&task).unwrap();

    assert_eq!(
        app.task(&task).unwrap().unwrap().status,
        orc::task::TaskStatus::Backlog
    );
    assert_eq!(app.runs_workspace(10, 10).unwrap().runs[0].status, "failed");
    assert!(
        app.lifecycle_events(10)
            .unwrap()
            .iter()
            .any(|event| event.kind == "task_requeue" && event.task_id.as_deref() == Some(&task))
    );
}

#[test]
fn app_subscription_receives_domain_events_in_order_without_replay() {
    let (_directory, app, task) = app_with_task("events");
    let subscription = app.subscribe();

    app.requeue(&task).unwrap();
    let first = subscription.recv().unwrap();
    assert!(
        matches!(first, orc::events::AppEvent::TaskLifecycle(ref event) if event.kind == "task_requeue")
    );
    assert_eq!(subscription.try_recv(), Err(TryRecvError::Empty));

    let second_subscription = app.subscribe();
    assert_eq!(second_subscription.try_recv(), Err(TryRecvError::Empty));
    assert!(matches!(subscription.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn disconnected_subscriber_does_not_affect_other_subscribers_or_operation() {
    let (_directory, app, task) = app_with_task("disconnect");
    let dropped = app.subscribe();
    let remaining = app.subscribe();
    drop(dropped);

    app.requeue(&task).unwrap();
    assert!(remaining.recv().is_ok());
}

#[test]
fn persisted_history_reconstructs_without_subscriber() {
    let (directory, app, task) = app_with_task("persisted");
    app.requeue(&task).unwrap();
    let history = app.lifecycle_events(10).unwrap();
    assert!(
        history
            .iter()
            .any(|event| event.task_id.as_deref() == Some(&task))
    );
    drop(app);
    let reopened = OrcApp::open(directory.path().join("state.sqlite"), directory.path()).unwrap();
    assert_eq!(
        reopened.task(&task).unwrap().unwrap().status.to_string(),
        "backlog"
    );
    assert!(!reopened.lifecycle_events(10).unwrap().is_empty());
}

#[test]
fn failed_database_purge_does_not_remove_task_worktree() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    let db = Database::init(&database).unwrap();
    let project = db.create_project("purge-lock").unwrap();
    let task = db
        .insert_task(project, "purge", "purge", "developer", TaskPriority::Normal)
        .unwrap();
    db.update_task_status(&task, orc::task::TaskStatus::Cancelled)
        .unwrap();
    let run = db.create_agent_run(project, &task, "agent").unwrap();
    db.update_agent_run_status(run, "completed", Some("done"))
        .unwrap();
    db.store_worktree_metadata(run, &task, "branch", ".orc/worktrees/purge")
        .unwrap();
    let worktree = directory
        .path()
        .join(orc::git::worktree_path_for_task(&task));
    std::fs::create_dir_all(&worktree).unwrap();
    let app = OrcApp::open(&database, directory.path()).unwrap();
    let lock = Connection::open(&database).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(app.purge_task(&task, true).is_err());
    assert!(worktree.exists());
    assert!(app.task(&task).unwrap().is_some());
    lock.execute_batch("ROLLBACK").unwrap();
}
