use orc::lead::LeadDecisionKind;
use orc::storage::Database;
use orc::storage::db::LeadDecisionMetadata;
use std::process::Command;
use tempfile::tempdir;

fn command(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn lead_inspection_cli_shows_pending_history_and_is_read_only() {
    let dir = tempdir().unwrap();
    assert!(command(dir.path(), &["init"]).status.success());
    let db_path = dir.path().join(".orc/orc.db");
    let db = Database::open(&db_path).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    for (kind, details, request, summary) in [
        (
            LeadDecisionKind::DirectTasks,
            serde_json::json!({"tasks":[{"local_id":"canonical","title":"Ship it","objective":"Ship the task","role":"developer","priority":"normal","depends_on":[],"capabilities":[],"scope_mode":"project","context_files":[],"expected_changes":["implementation"],"unchanged":["unrelated behavior"],"acceptance_criteria":["works"],"required_tests":["focused test"],"validation":["cargo test"],"execution_hints":{}}]}),
            "direct request",
            "direct summary",
        ),
        (
            LeadDecisionKind::PlanRequired,
            serde_json::json!({"plan":"needs planning","constraints":["safe"]}),
            "plan request",
            "plan summary",
        ),
        (
            LeadDecisionKind::UserDecisionRequired,
            serde_json::json!({"question":"Choose a path","options":["a","b"]}),
            "choice request",
            "choice summary",
        ),
    ] {
        db.record_lead_decision(
            project,
            &kind,
            &details,
            LeadDecisionMetadata {
                snapshot: "persisted snapshot",
                run_id: Some(42),
                source_request: request,
                summary,
            },
        )
        .unwrap();
    }
    let before = db.list_lead_decisions(project).unwrap();
    let before_tasks = db.list_tasks().unwrap();
    let before_events = db.list_lifecycle_events(usize::MAX).unwrap();
    let before_runs = db.list_agent_runs(project, usize::MAX).unwrap();
    let before_turns = db.list_lead_turns(project, usize::MAX).unwrap();
    drop(db);

    let pending = command(dir.path(), &["lead", "pending"]);
    assert!(pending.status.success());
    let pending_json: serde_json::Value = serde_json::from_slice(&pending.stdout).unwrap();
    assert_eq!(pending_json["kind"], "USER_DECISION_REQUIRED");
    assert_eq!(pending_json["status"], "pending");
    assert_eq!(pending_json["actionable"], true);
    assert_eq!(pending_json["source_request"], "choice request");
    assert_eq!(pending_json["run_id"], 42);
    assert!(pending_json["id"].as_i64().unwrap() > 0);
    assert!(!pending_json["created_at"].as_str().unwrap().is_empty());
    assert_eq!(pending_json["summary"], "choice summary");
    assert_eq!(pending_json["snapshot"], "persisted snapshot");
    assert_eq!(pending_json["details"]["question"], "Choose a path");

    let history = command(dir.path(), &["lead", "history"]);
    assert!(history.status.success());
    let history_json: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(history_json.as_array().unwrap().len(), 3);
    assert_eq!(history_json[0]["kind"], "DIRECT_TASKS");
    assert!(history_json[0]["id"].as_i64().unwrap() > 0);
    assert!(!history_json[0]["created_at"].as_str().unwrap().is_empty());
    assert_eq!(history_json[0]["run_id"], 42);
    assert_eq!(history_json[0]["source_request"], "direct request");
    assert_eq!(history_json[0]["summary"], "direct summary");
    assert_eq!(history_json[0]["snapshot"], "persisted snapshot");
    assert_eq!(
        history_json[0]["details"]["tasks"][0]["local_id"],
        "canonical"
    );
    assert_eq!(
        history_json[0]["details"]["tasks"][0]["expected_changes"][0],
        "implementation"
    );
    assert_eq!(history_json[1]["kind"], "PLAN_REQUIRED");
    assert_eq!(history_json[1]["details"]["constraints"][0], "safe");
    assert_eq!(history_json[1]["snapshot"], "persisted snapshot");
    assert_eq!(history_json[2]["kind"], "USER_DECISION_REQUIRED");
    assert_eq!(history_json[2]["source_request"], "choice request");
    assert_eq!(history_json[2]["details"]["options"][1], "b");
    assert_eq!(history_json[0]["status"], "superseded");

    let reopened = Database::open(&db_path).unwrap();
    assert_eq!(reopened.list_lead_decisions(project).unwrap(), before);
    assert_eq!(reopened.list_tasks().unwrap(), before_tasks);
    assert_eq!(
        reopened.list_lifecycle_events(usize::MAX).unwrap(),
        before_events
    );
    assert_eq!(
        serde_json::to_value(reopened.list_agent_runs(project, usize::MAX).unwrap()).unwrap(),
        serde_json::to_value(before_runs).unwrap()
    );
    assert_eq!(
        reopened.list_lead_turns(project, usize::MAX).unwrap(),
        before_turns
    );
    drop(reopened);

    let db = Database::open(&db_path).unwrap();
    db.consume_pending_lead_decision(project).unwrap();
    drop(db);
    let no_pending = command(dir.path(), &["lead", "pending"]);
    assert!(no_pending.status.success());
    assert_eq!(String::from_utf8_lossy(&no_pending.stdout).trim(), "null");
}

#[test]
fn lead_user_decision_resolution_is_persistent_and_single_use() {
    let dir = tempdir().unwrap();
    assert!(command(dir.path(), &["init"]).status.success());
    let db_path = dir.path().join(".orc/orc.db");
    let db = Database::open(&db_path).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    let id = db
        .record_lead_decision(
            project,
            &LeadDecisionKind::UserDecisionRequired,
            &serde_json::json!({"question":"Choose"}),
            LeadDecisionMetadata {
                snapshot: "state",
                run_id: Some(7),
                source_request: "request",
                summary: "summary",
            },
        )
        .unwrap();
    let before_tasks = db.list_tasks().unwrap();
    let before_runs = db.list_agent_runs(project, usize::MAX).unwrap();
    drop(db);

    let invalid = command(dir.path(), &["lead", "resolve", &id.to_string(), "   "]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("resolution must not be empty"));

    let db = Database::open(&db_path).unwrap();
    let pending = db.pending_lead_decision(project).unwrap().unwrap();
    assert_eq!(pending.status, "pending");
    drop(db);

    let valid = command(
        dir.path(),
        &["lead", "resolve", &id.to_string(), "take option A"],
    );
    assert!(valid.status.success());
    let resolved: serde_json::Value = serde_json::from_slice(&valid.stdout).unwrap();
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(resolved["resolution"], "take option A");
    assert_eq!(resolved["source_request"], "request");

    let db = Database::open(&db_path).unwrap();
    assert_eq!(db.list_tasks().unwrap(), before_tasks);
    assert_eq!(
        serde_json::to_value(db.list_agent_runs(project, usize::MAX).unwrap()).unwrap(),
        serde_json::to_value(before_runs).unwrap()
    );
    let history = db.list_lead_decisions(project).unwrap();
    assert_eq!(history[0].details, r#"{"question":"Choose"}"#);
    assert_eq!(history[0].resolution.as_deref(), Some("take option A"));
    drop(db);

    let second = command(dir.path(), &["lead", "resolve", &id.to_string(), "again"]);
    assert!(!second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("decision is missing or already resolved")
    );

    let db = Database::open(&db_path).unwrap();
    assert_eq!(db.list_lead_decisions(project).unwrap(), history);
}
