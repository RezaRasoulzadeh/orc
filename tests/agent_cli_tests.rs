use std::fs;
use std::process::Command;

use anyhow::Result;
use orc::automated::{ActionBackend, ActionExecution, ActionOverrides, ReviewResult};
use orc::registry::{AgentAction, AgentDefinition, ReasoningEffort};
use orc::storage::Database;
use orc::task::TaskPriority;
use orc::worker::TokenUsage;
use tempfile::tempdir;

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
    assert!(show_text.contains("Capabilities:        code, terminal"));
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
            })?,
            token_usage: Some(TokenUsage {
                input_tokens: Some(1),
                output_tokens: Some(1),
                total_tokens: 2,
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
    drop(db);

    let app = orc::app::OrcApp::open(&db_path, directory.path()).unwrap();
    let (_, result) = app
        .automated_review_with_backend(&task, &ActionOverrides::default(), &ReviewBackend)
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
