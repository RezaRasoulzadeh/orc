use std::fs;
use std::process::Command;

use anyhow::Result;
use orc::automated::{ActionBackend, ActionExecution, ActionOverrides, ReviewResult};
use orc::registry::{AgentAction, AgentDefinition, ReasoningEffort};
use orc::storage::Database;
use orc::task::{TaskPriority, TaskStatus};
use orc::validation::test_helpers::FakeValidationRunner;
use orc::worker::TokenUsage;
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn configured_lead_run_executes_cli_and_persists_canonical_direct_tasks() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    assert!(
        orc_command(
            directory.path(),
            &[
                "agent",
                "add",
                "lead",
                "--backend",
                "codex",
                "--action",
                "lead",
                "--profile",
                "/tmp/lead-profile"
            ],
        )
        .status
        .success()
    );
    assert!(
        orc_command(directory.path(), &["lead", "set", "lead"])
            .status
            .success()
    );

    let bin = directory.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let codex = bin.join("codex");
    fs::write(&codex, r##"#!/bin/sh
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"message\":\"assessment\",\"proposals\":[],\"decision\":{\"kind\":\"DIRECT_TASKS\",\"details\":{\"tasks\":[{\"local_id\":\"canonical-cli\",\"title\":\"Canonical CLI task\",\"objective\":\"Do it\",\"role\":\"developer\",\"priority\":\"normal\",\"depends_on\":[],\"capabilities\":[],\"scope_mode\":\"project\",\"context_files\":[],\"expected_changes\":[],\"unchanged\":[],\"acceptance_criteria\":[],\"required_tests\":[],\"validation\":[],\"execution_hints\":{}}]}}}"}}'
"##).unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), old_path.to_string_lossy());
    let output = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(directory.path())
        .env(
            "ORC_GLOBAL_REGISTRY_PATH",
            directory.path().join(".orc/test.agents.db"),
        )
        .env("PATH", path)
        .args(["lead", "run", "assess"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Decision: DirectTasks"));
    let db = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    let project_id = db.get_project_id().unwrap().unwrap();
    let tasks = db.list_tasks().unwrap();
    assert!(tasks.is_empty(), "Lead run must not create tasks");
    let decision = db.pending_lead_decision(project_id).unwrap().unwrap();
    assert!(
        decision.run_id.is_some(),
        "decision must link to its Lead run"
    );
    let details: serde_json::Value = serde_json::from_str(&decision.details).unwrap();
    assert_eq!(details["tasks"][0]["local_id"], "canonical-cli");
    assert_eq!(details["tasks"][0]["title"], "Canonical CLI task");
}

#[test]
fn dispatch_queue_concurrency_parser_accepts_auto_and_positive_values() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());

    for value in ["auto", "1", "3"] {
        let output = orc_command(
            directory.path(),
            &["dispatch-queue", "--concurrency", value],
        );
        assert!(
            output.status.success(),
            "{value}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn dispatch_queue_concurrency_parser_rejects_zero_and_invalid_text() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());

    for value in ["0", "invalid"] {
        let output = orc_command(
            directory.path(),
            &["dispatch-queue", "--concurrency", value],
        );
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("positive integer or 'auto'"));
    }
}

fn orc_command(directory: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(directory)
        .env(
            "ORC_GLOBAL_REGISTRY_PATH",
            directory.join(".orc/test.agents.db"),
        )
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn agent_attach_and_detach_control_project_scheduler_eligibility() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());

    let db_path = directory.path().join(".orc/orc.db");
    let db = Database::open(&db_path).unwrap();
    let project_id = db.get_project_id().unwrap().unwrap();
    for reference in db.list_project_agent_references(project_id).unwrap() {
        db.remove_global_agent_reference(project_id, &reference.agent_id)
            .unwrap();
    }
    let agent = orc::registry::Agent::from_definition(&AgentDefinition {
        id: "existing-global".into(),
        backend: "codex".into(),
        execution_mode: "automated".into(),
        display_name: "Existing Global".into(),
        enabled: true,
        priority: 10,
        capabilities: vec![],
        status: "available".into(),
        unavailable_reason: None,
        profile_path: None,
        model: None,
        reasoning_effort: None,
        config_metadata: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![AgentAction::Code],
    })
    .unwrap();
    db.insert_global_agent(&agent).unwrap();
    drop(db);

    assert!(
        orc_command(
            directory.path(),
            &["task", "create", "Attach test", "Exercise scheduler"]
        )
        .status
        .success()
    );
    let before = orc_command(
        directory.path(),
        &["schedule", "T-0001", "--explain", "--mode", "automated"],
    );
    assert!(before.status.success());
    let before_stdout = String::from_utf8_lossy(&before.stdout);
    assert!(before_stdout.contains("Candidates:\n\nReason:"));

    let unknown = orc_command(directory.path(), &["agent", "attach", "unknown"]);
    assert!(!unknown.status.success());

    let attach = orc_command(directory.path(), &["agent", "attach", "existing-global"]);
    assert!(
        attach.status.success(),
        "{}",
        String::from_utf8_lossy(&attach.stderr)
    );
    assert!(
        orc_command(directory.path(), &["agent", "attach", "existing-global"])
            .status
            .success()
    );

    let reopened = Database::open(&db_path).unwrap();
    assert_eq!(
        reopened
            .list_project_agent_references(project_id)
            .unwrap()
            .len(),
        1
    );
    let project_only = rusqlite::Connection::open(&db_path).unwrap();
    let local_agents: i64 = project_only
        .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
        .unwrap();
    assert_eq!(local_agents, 0);
    drop(project_only);
    drop(reopened);

    let after = orc_command(
        directory.path(),
        &["schedule", "T-0001", "--explain", "--mode", "automated"],
    );
    assert!(after.status.success());
    assert!(String::from_utf8_lossy(&after.stdout).contains("existing-global"));

    assert!(
        orc_command(directory.path(), &["agent", "detach", "existing-global"])
            .status
            .success()
    );
    let detached = Database::open(&db_path).unwrap();
    assert!(detached.list_project_agents(project_id).unwrap().is_empty());
    assert!(
        detached
            .get_global_agent("existing-global")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        detached
            .list_project_agent_references(project_id)
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn lead_run_without_configuration_explains_how_to_configure_it() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());

    let output = orc_command(directory.path(), &["lead", "run", "assess"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("orc lead set <agent>"));
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
fn agent_actions_can_be_added_removed_and_reopened() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    assert!(
        orc_command(
            directory.path(),
            &[
                "agent",
                "add",
                "multi",
                "--backend",
                "codex",
                "--capability",
                "code",
                "--capability",
                "terminal",
                "--action",
                "review",
                "--action",
                "plan"
            ]
        )
        .status
        .success()
    );
    let show = orc_command(directory.path(), &["agent", "show", "multi"]);
    let show_text = String::from_utf8_lossy(&show.stdout);
    assert!(show_text.contains("Capabilities:        code, command_execution"));
    assert!(show_text.contains("Actions:             plan, review"));
    let actions = orc_command(directory.path(), &["agent", "actions", "multi"]);
    let actions_text = String::from_utf8_lossy(&actions.stdout);
    assert!(actions.status.success());
    assert!(actions_text.contains("plan\tmodel=-\teffort=-"));
    assert!(actions_text.contains("review\tmodel=-\teffort=-"));
    assert!(
        orc_command(
            directory.path(),
            &["agent", "action-remove", "multi", "review"]
        )
        .status
        .success()
    );
    assert!(
        orc_command(directory.path(), &["agent", "action-add", "multi", "lead"])
            .status
            .success()
    );
    let db = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    let agent = db.get_agent("multi").unwrap().unwrap();
    assert_eq!(
        agent.actions,
        vec![
            orc::registry::AgentAction::Lead,
            orc::registry::AgentAction::Plan
        ]
    );
    drop(db);
    let reopened = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    assert_eq!(
        reopened.get_agent("multi").unwrap().unwrap().actions,
        agent.actions
    );
}

#[test]
fn agent_add_defaults_to_code_and_final_action_cannot_be_removed() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    assert!(
        orc_command(
            directory.path(),
            &["agent", "add", "default", "--backend", "codex"]
        )
        .status
        .success()
    );

    let db = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    assert_eq!(
        db.get_agent("default").unwrap().unwrap().actions,
        vec![AgentAction::Code]
    );
    drop(db);

    let remove = orc_command(
        directory.path(),
        &["agent", "action-remove", "default", "code"],
    );
    assert!(!remove.status.success());
    assert!(String::from_utf8_lossy(&remove.stderr).contains("final supported action"));
}

struct ReviewBackend;

#[test]
fn init_creates_engineering_contract_when_missing() {
    let directory = tempdir().unwrap();
    let output = orc_command(directory.path(), &["init"]);
    assert!(output.status.success());
    let contract = directory.path().join(".orc/engineering.md");
    assert!(contract.is_file());
    assert!(!std::fs::read_to_string(contract).unwrap().is_empty());
}

#[test]
fn init_preserves_existing_engineering_contract() {
    let directory = tempdir().unwrap();
    let contract = directory.path().join(".orc/engineering.md");
    std::fs::create_dir_all(contract.parent().unwrap()).unwrap();
    let custom = "# CUSTOM_INIT_CONTRACT\nKeep this exact content.\n";
    std::fs::write(&contract, custom).unwrap();

    let output = orc_command(directory.path(), &["init"]);
    assert!(output.status.success());
    assert_eq!(std::fs::read_to_string(contract).unwrap(), custom);
}

impl ActionBackend for ReviewBackend {
    fn invoke(
        &self,
        agent: &AgentDefinition,
        action: AgentAction,
        _input: &str,
        _model: Option<&str>,
        _effort: Option<ReasoningEffort>,
    ) -> Result<ActionExecution> {
        assert_eq!(agent.id, "reviewer");
        assert_eq!(action, AgentAction::Review);
        Ok(ActionExecution {
            output: serde_json::to_string(&ReviewResult {
                verdict: "accept".into(),
                findings: Vec::new(),
                blocking_findings: Vec::new(),
                non_blocking_findings: Vec::new(),
                severity: None,
                revision_feedback: None,
                blockers: Vec::new(),
            })?,
            token_usage: Some(TokenUsage {
                input_tokens: Some(1),
                output_tokens: Some(1),
                total_tokens: 2,
                cached_input_tokens: None,
            }),
        })
    }
}

#[test]
fn cli_configured_review_action_is_selected_for_automated_review() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    assert!(
        orc_command(
            directory.path(),
            &[
                "agent",
                "add",
                "reviewer",
                "--backend",
                "codex",
                "--action",
                "review",
            ],
        )
        .status
        .success()
    );
    let db_path = directory.path().join(".orc/orc.db");
    let db = Database::open(&db_path).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    let task = db
        .insert_task(
            project,
            "Review selection",
            "Select the CLI-configured reviewer",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&task, TaskStatus::Review).unwrap();
    drop(db);

    let app = orc::app::OrcApp::open(&db_path, directory.path()).unwrap();
    let (_, result) = app
        .automated_review_with_backend(
            &task,
            &ActionOverrides::default(),
            &ReviewBackend,
            &FakeValidationRunner::success(),
        )
        .unwrap();
    assert_eq!(result.verdict, "accept");
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
            "capabilities": [],
            "scope_mode": null,
            "context_files": ["existing.rs", "missing.rs"],
            "expected_changes": ["new.rs"],
            "unchanged": ["existing behavior"],
            "acceptance_criteria": ["context warning is reported"],
            "required_tests": ["apply plan CLI test"],
            "validation": ["cargo test"],
            "execution_hints": {"effort":"low","effort_reason":"isolated context check"},
            "risk_factors": [],
            "depends_on": []
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
