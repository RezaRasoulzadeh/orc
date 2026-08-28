use orc::agent_onboarding::{AgentAuthentication, AgentOnboardingRequest, ProviderOnboarding};
use orc::backend::ProviderInspection;
use orc::registry::{
    self, AgentAction, AgentCapability, AgentExecutionMode, AgentProviderConfiguration,
    OperatorPermission,
};
use orc::storage::Database;
use std::process::Command;
use tempfile::tempdir;

struct FakeProvider {
    inspection: ProviderInspection,
}

impl ProviderOnboarding for FakeProvider {
    fn inspect(
        &self,
        _provider: &AgentProviderConfiguration,
        _mode: AgentExecutionMode,
        _declared_capabilities: &[String],
    ) -> Result<ProviderInspection, String> {
        Ok(self.inspection.clone())
    }
}

fn request() -> AgentOnboardingRequest {
    AgentOnboardingRequest {
        id: "shared-agent".into(),
        backend: "copilot".into(),
        execution_mode: AgentExecutionMode::Automated,
        display_name: "Shared Agent".into(),
        profile_path: None,
        model: None,
        reasoning_effort: None,
        priority: 10,
        roles: vec![AgentAction::Review],
        permissions: vec![
            OperatorPermission::RepositoryRead,
            OperatorPermission::CommandExecution,
        ],
        declared_capabilities: vec![],
    }
}

fn provider() -> FakeProvider {
    FakeProvider {
        inspection: ProviderInspection {
            authenticated: true,
            authentication_method: "test-login".into(),
            authentication_detail: Some("verified".into()),
            capabilities: vec![
                AgentCapability::RepositoryRead,
                AgentCapability::StructuredOutput,
            ],
        },
    }
}

#[test]
fn onboarding_inspects_before_explicit_operator_approval_and_persists_separate_authority() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    Database::init(&path).unwrap();
    let app = orc::app::OrcApp::open(&path, directory.path()).unwrap();

    let preview = app
        .onboard_agent_with(&request(), false, &provider())
        .unwrap();
    assert!(!preview.persisted);
    assert!(app.global_agents().unwrap().is_empty());
    assert_eq!(
        preview.preview.provider_capabilities,
        vec![
            AgentCapability::RepositoryRead,
            AgentCapability::StructuredOutput
        ]
    );
    assert_eq!(
        preview.preview.permissions,
        vec![
            OperatorPermission::RepositoryRead,
            OperatorPermission::CommandExecution
        ]
    );
    assert_eq!(preview.preview.agent.actions, vec![AgentAction::Review]);

    let approved = app
        .onboard_agent_with(&request(), true, &provider())
        .unwrap();
    assert!(approved.persisted);
    let saved = app.global_agents().unwrap().pop().unwrap();
    assert_eq!(
        saved.capabilities,
        vec![
            AgentCapability::RepositoryRead,
            AgentCapability::StructuredOutput
        ]
    );
    assert_eq!(saved.roles, vec![AgentAction::Review]);
    assert_eq!(
        app.agent_configuration("shared-agent").unwrap().permissions,
        vec![
            OperatorPermission::RepositoryRead,
            OperatorPermission::CommandExecution
        ]
    );
    assert!(
        app.agent_configuration("shared-agent")
            .unwrap()
            .authentication
            .verified
    );
}

#[test]
fn configuration_export_is_versioned_and_update_is_atomic() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let app = orc::app::OrcApp::open(&path, directory.path()).unwrap();
    app.onboard_agent_with(&request(), true, &provider())
        .unwrap();

    let mut document = app.agent_configuration("shared-agent").unwrap();
    assert_eq!(
        document.configuration_version,
        registry::AGENT_CONFIGURATION_VERSION
    );
    document.agent.display_name = "Updated Agent".into();
    document.permissions = vec![OperatorPermission::RepositoryRead];
    app.import_agent_configuration(&document).unwrap();
    assert_eq!(
        db.get_global_agent("shared-agent")
            .unwrap()
            .unwrap()
            .display_name,
        "Updated Agent"
    );
    assert_eq!(
        db.agent_permissions("shared-agent").unwrap(),
        vec![OperatorPermission::RepositoryRead]
    );

    let mut unsupported = document.clone();
    unsupported.configuration_version += 1;
    unsupported.agent.display_name = "Must not persist".into();
    assert!(app.import_agent_configuration(&unsupported).is_err());
    assert_eq!(
        db.get_global_agent("shared-agent")
            .unwrap()
            .unwrap()
            .display_name,
        "Updated Agent"
    );

    let mut unauthenticated = document;
    unauthenticated.authentication = AgentAuthentication {
        verified: false,
        method: "not-verified".into(),
        detail: None,
    };
    assert!(app.import_agent_configuration(&unauthenticated).is_err());
}

#[test]
fn manual_cli_onboarding_lists_inspection_and_can_be_exported() {
    let directory = tempdir().unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(directory.path())
        .arg("init")
        .output()
        .unwrap();
    assert!(init.status.success());

    let onboard = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(directory.path())
        .args([
            "agent",
            "onboard",
            "manual-agent",
            "--backend",
            "generic_manual",
            "--mode",
            "manual",
            "--capability",
            "structured-output",
            "--permission",
            "repository-read",
            "--role",
            "review",
            "--approve",
        ])
        .output()
        .unwrap();
    assert!(
        onboard.status.success(),
        "{}",
        String::from_utf8_lossy(&onboard.stderr)
    );
    let stdout = String::from_utf8_lossy(&onboard.stdout);
    assert!(stdout.contains("Onboarded agent manual-agent"));
    assert!(stdout.contains("Provider capabilities: structured_output"));
    assert!(stdout.contains("Operator permissions: repository_read"));
    assert!(stdout.contains("Orc roles: review"));

    let list = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(directory.path())
        .args(["agents"])
        .output()
        .unwrap();
    assert!(list.status.success());
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("manual-agent"));
    assert!(list_text.contains("review"));
    assert!(list_text.contains("repository_read"));

    let export = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(directory.path())
        .args(["agent", "export", "manual-agent"])
        .output()
        .unwrap();
    assert!(export.status.success());
    let document: serde_json::Value = serde_json::from_slice(&export.stdout).unwrap();
    assert_eq!(
        document["configuration_version"],
        registry::AGENT_CONFIGURATION_VERSION
    );
    assert_eq!(document["permissions"][0], "repository_read");
    assert!(document.get("credentials").is_none());
}
