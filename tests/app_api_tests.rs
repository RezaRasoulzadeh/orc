use orc::app::OrcApp;
use orc::protocol::{PROTOCOL_VERSION, PlanResponse, PlannedTask};
use orc::storage::Database;
use orc::task::TaskPriority;
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
