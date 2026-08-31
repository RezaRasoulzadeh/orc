use orc::lead::LeadDecisionKind;
use orc::storage::Database;
use orc::storage::db::LeadDecisionMetadata;
use std::process::Command;
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn plan_run_cli_success_persists_and_terminates_without_repository_or_task_mutation() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let root = dir.path();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_orc"))
            .current_dir(root)
            .env("ORC_GLOBAL_REGISTRY_PATH", root.join(".orc/test.agents.db"))
            .arg("init")
            .output()
            .unwrap()
            .status
            .success()
    );
    let profile = root.join("planner-profile");
    fs::write(&profile, "profile").unwrap();
    let add = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(root)
        .env("ORC_GLOBAL_REGISTRY_PATH", root.join(".orc/test.agents.db"))
        .args([
            "agent",
            "add",
            "planner",
            "--backend",
            "codex",
            "--action",
            "plan",
            "--profile",
        ])
        .arg(&profile)
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let db_path = root.join(".orc/orc.db");
    let db = Database::open(&db_path).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    assert!(db.set_agent_quota("planner", 100, None).unwrap());
    db.record_lead_decision(
        project,
        &LeadDecisionKind::PlanRequired,
        &serde_json::json!({"kind":"PLAN_REQUIRED"}),
        LeadDecisionMetadata {
            snapshot: "snapshot",
            run_id: None,
            source_request: "cli objective",
            summary: "cli objective",
        },
    )
    .unwrap();
    let repository_file = root.join("preserved.txt");
    fs::write(&repository_file, "do not touch").unwrap();
    drop(db);

    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let codex = bin.join("codex");
    let count = root.join("planner-calls");
    fs::write(&codex, r#"#!/bin/sh
printf 'x' >> "$ORC_PLANNER_CALLS"
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"protocol_version\":1,\"objective\":\"cli objective\",\"assumptions\":[],\"risks\":[],\"questions\":[],\"tasks\":[]}"}}'
"#).unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), old_path.to_string_lossy());
    let output = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(root)
        .env("ORC_GLOBAL_REGISTRY_PATH", root.join(".orc/test.agents.db"))
        .env("PATH", path)
        .env("ORC_PLANNER_CALLS", &count)
        .args(["plan", "run", "--agent", "planner"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("persisted"));
    assert_eq!(fs::read_to_string(&count).unwrap().len(), 1);
    assert_eq!(
        fs::read_to_string(&repository_file).unwrap(),
        "do not touch"
    );
    let reopened = Database::open(&db_path).unwrap();
    assert_eq!(reopened.list_tasks().unwrap().len(), 0);
    assert_eq!(reopened.list_plan_history(project).unwrap().len(), 1);
    assert!(reopened.pending_lead_decision(project).unwrap().is_none());
}

#[test]
fn plan_run_cli_rejects_missing_actionable_plan_decision_and_terminates() {
    let dir = tempdir().unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(dir.path())
        .env(
            "ORC_GLOBAL_REGISTRY_PATH",
            dir.path().join(".orc/test.agents.db"),
        )
        .arg("init")
        .output()
        .unwrap();
    assert!(init.status.success());
    let db_path = dir.path().join(".orc/orc.db");
    let db = Database::open(&db_path).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    db.record_lead_decision(
        project,
        &LeadDecisionKind::DirectTasks,
        &serde_json::json!({"kind":"DIRECT_TASKS"}),
        LeadDecisionMetadata {
            snapshot: "snapshot",
            run_id: None,
            source_request: "request",
            summary: "summary",
        },
    )
    .unwrap();
    drop(db);
    let result = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(dir.path())
        .env(
            "ORC_GLOBAL_REGISTRY_PATH",
            dir.path().join(".orc/test.agents.db"),
        )
        .args(["plan", "run"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("actionable PLAN_REQUIRED"), "{stderr}");
    let db = Database::open(&db_path).unwrap();
    assert!(db.list_plan_history(project).unwrap().is_empty());
    assert!(db.list_tasks().unwrap().is_empty());
}
