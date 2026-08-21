use std::fs;
use std::process::Command;

use orc::storage::Database;
use tempfile::tempdir;

fn orc_command(directory: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn codex_add_profile_and_profile_update_persist() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    assert!(
        orc_command(
            directory.path(),
            &[
                "agent",
                "add",
                "codex-third",
                "--backend",
                "codex",
                "--profile",
                "/profiles/third",
            ],
        )
        .status
        .success()
    );
    assert!(
        orc_command(
            directory.path(),
            &["agent", "profile", "codex-third", "/profiles/updated-third"],
        )
        .status
        .success()
    );

    let db = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    assert_eq!(
        db.get_agent("codex-third")
            .unwrap()
            .unwrap()
            .profile_path
            .as_deref(),
        Some("/profiles/updated-third")
    );
}

#[test]
fn agent_profile_rejects_missing_agent() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    let output = orc_command(
        directory.path(),
        &["agent", "profile", "unknown", "/profiles/unknown"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown"));
}

#[test]
fn codex_model_and_effort_commands_persist_and_validate() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    assert!(
        orc_command(
            directory.path(),
            &[
                "agent",
                "add",
                "codex-main",
                "--backend",
                "codex",
                "--profile",
                "/profiles/main",
                "--model",
                "gpt-5.6-luna",
                "--effort",
                "low",
            ],
        )
        .status
        .success()
    );
    assert!(
        orc_command(
            directory.path(),
            &["agent", "model", "codex-main", "gpt-5.6-terra"],
        )
        .status
        .success()
    );
    assert!(
        orc_command(directory.path(), &["agent", "effort", "codex-main", "high"])
            .status
            .success()
    );
    let show = orc_command(directory.path(), &["agent", "show", "codex-main"]);
    let show_text = String::from_utf8_lossy(&show.stdout);
    assert!(show.status.success());
    assert!(show_text.contains("Model:              gpt-5.6-terra"));
    assert!(show_text.contains("Reasoning effort:   high"));
    let invalid = orc_command(
        directory.path(),
        &["agent", "effort", "codex-main", "maximum"],
    );
    assert!(!invalid.status.success());

    let db = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    let agent = db.get_agent("codex-main").unwrap().unwrap();
    assert_eq!(agent.model.as_deref(), Some("gpt-5.6-terra"));
    assert_eq!(agent.reasoning_effort.unwrap().as_str(), "high");
    drop(db);

    let reopened = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    assert_eq!(
        reopened
            .get_agent("codex-main")
            .unwrap()
            .unwrap()
            .model
            .as_deref(),
        Some("gpt-5.6-terra")
    );
}

#[test]
fn apply_plan_warns_for_missing_context_files_without_aborting() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    fs::write(directory.path().join("existing.rs"), "fn existing() {}\n").unwrap();
    let plan_path = directory.path().join("plan.json");
    let plan = serde_json::json!({
        "protocol_version": 1,
        "objective": "test plan",
        "assumptions": [],
        "risks": [],
        "questions": [],
        "tasks": [{
            "local_id": "T-0001",
            "title": "Test task",
            "objective": "Verify context warnings",
            "role": "developer",
            "priority": "normal",
            "context_files": ["existing.rs", "missing.rs"],
            "expected_changes": ["new.rs"]
        }]
    });
    fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();

    let output = orc_command(directory.path(), &["apply-plan", "plan.json"]);

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("T-0001"));
    assert!(stderr.contains("missing.rs"));
    assert!(!stderr.contains("existing.rs"));
    assert!(!stderr.contains("new.rs"));
}
