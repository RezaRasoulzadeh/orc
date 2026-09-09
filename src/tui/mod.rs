mod event;
mod state;
mod terminal;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use anyhow::{Context, Result};

use crate::app::OrcApp;
use crate::automated::ActionOverrides;

use self::state::{Intent, LifecycleAction, Screen, TuiState};

pub fn run(database_path: impl AsRef<Path>, repository_path: impl AsRef<Path>) -> Result<()> {
    let database_path = database_path.as_ref().to_path_buf();
    let repository_path = repository_path.as_ref().to_path_buf();
    let registry_path = crate::storage::Database::default_global_registry_path();
    let app = OrcApp::open_global(&database_path, &repository_path)?;
    let project_name = app.operations().project_name()?;
    let snapshot = app.operations().snapshot()?;
    let mut state = TuiState::new(project_name, snapshot);
    let mut action_receiver = None;

    terminal::install_panic_restore_hook();
    let mut session = terminal::TerminalSession::enter()?;
    loop {
        poll_action_completion(&app, &mut state, &mut action_receiver);
        session
            .terminal()
            .draw(|frame| ui::draw(frame, &mut state))?;
        let Some(key) = event::next_key()? else {
            continue;
        };
        match state.handle_key(key) {
            Intent::None => {}
            Intent::Quit => break,
            Intent::Refresh => refresh(&app, &mut state),
            Intent::OpenDetail => open_detail(&app, &mut state),
            Intent::OpenTaskCreation => state.open_task_creation(),
            Intent::CancelTaskCreation => {}
            Intent::SubmitTaskCreation => {
                if let Some(input) = state.take_task_creation() {
                    create_task(&app, &mut state, input, session.terminal());
                }
            }
            Intent::Run(LifecycleAction::Revise) => {
                state.revision_input = Some(String::new());
                state.message = Some("enter concise revision feedback".into());
            }
            Intent::Run(action) => start_action(
                &mut state,
                &mut action_receiver,
                &database_path,
                &repository_path,
                &registry_path,
                action,
                None,
            ),
            Intent::SubmitRevision(feedback) => start_action(
                &mut state,
                &mut action_receiver,
                &database_path,
                &repository_path,
                &registry_path,
                LifecycleAction::Revise,
                Some(feedback),
            ),
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ActionCompletion {
    action: LifecycleAction,
    task_id: String,
    result: Result<String, String>,
}

fn start_action(
    state: &mut TuiState,
    receiver: &mut Option<Receiver<ActionCompletion>>,
    database_path: &Path,
    repository_path: &Path,
    registry_path: &Path,
    action: LifecycleAction,
    feedback: Option<String>,
) {
    if state.running.is_some() || state.available_action() != Some(action) {
        state.message = Some("action is no longer valid; refresh the task".into());
        return;
    }
    let Some(task_id) = state.selected_task_id().map(str::to_owned) else {
        return;
    };
    state.running = Some(action);
    state.running_task_id = Some(task_id.clone());
    state.message = None;
    match spawn_action(
        database_path.to_owned(),
        repository_path.to_owned(),
        registry_path.to_owned(),
        task_id,
        action,
        feedback,
    ) {
        Ok(action_receiver) => *receiver = Some(action_receiver),
        Err(error) => {
            state.running = None;
            state.running_task_id = None;
            state.message = Some(short_error("start action", &error));
        }
    }
}

fn spawn_action(
    database_path: PathBuf,
    repository_path: PathBuf,
    registry_path: PathBuf,
    task_id: String,
    action: LifecycleAction,
    feedback: Option<String>,
) -> Result<Receiver<ActionCompletion>> {
    let (sender, receiver) = mpsc::channel();
    let thread_name = format!("orc-tui-{}-{}", action.label(), task_id);
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let result =
                OrcApp::open_with_registry(&database_path, &repository_path, &registry_path)
                    .and_then(|app| execute_action(&app, &task_id, action, feedback.as_deref()))
                    .map_err(|error| format!("{error:#}"));
            let _ = sender.send(ActionCompletion {
                action,
                task_id,
                result,
            });
        })
        .context("could not start TUI action thread")?;
    Ok(receiver)
}

fn execute_action(
    app: &OrcApp,
    task_id: &str,
    action: LifecycleAction,
    feedback: Option<&str>,
) -> Result<String> {
    match action {
        LifecycleAction::Dispatch => app
            .dispatch(task_id, None)
            .map(|summary| format!("dispatch finished: run {}", summary.run_status)),
        LifecycleAction::Review => app
            .automated_review(task_id, &ActionOverrides::default())
            .map(|(_, review)| format!("review finished: {}", review.verdict)),
        LifecycleAction::Revise => revise(app, task_id, feedback.unwrap_or_default()),
        LifecycleAction::Accept => app.accept(task_id).map(|()| "task accepted".to_string()),
    }
}

fn poll_action_completion(
    app: &OrcApp,
    state: &mut TuiState,
    receiver: &mut Option<Receiver<ActionCompletion>>,
) {
    let Some(result) = receiver.as_ref().map(Receiver::try_recv) else {
        return;
    };
    match result {
        Ok(completion) => {
            receiver.take();
            finish_action(app, state, completion);
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            receiver.take();
            state.running = None;
            state.running_task_id = None;
            state.revision_input = None;
            state.message = Some(short_error_text(
                "action",
                "background action stopped without returning a result",
            ));
        }
    }
}

fn finish_action(app: &OrcApp, state: &mut TuiState, completion: ActionCompletion) {
    state.running = None;
    state.running_task_id = None;
    state.revision_input = None;
    let action_message = Some(match completion.result {
        Ok(message) => format!(
            "{} {}: {}",
            completion.action.label(),
            completion.task_id,
            message
        ),
        Err(error) => short_error_text(completion.action.label(), &error),
    });
    refresh(app, state);
    state.message = action_message;
}

fn create_task(
    app: &OrcApp,
    state: &mut TuiState,
    input: crate::task::CreateTaskInput,
    terminal: &mut terminal::OrcTerminal,
) {
    state.creating = true;
    state.message = None;
    if let Err(error) = terminal.draw(|frame| ui::draw(frame, state)) {
        state.creating = false;
        state.message = Some(short_error("render task creation", &error.into()));
        return;
    }

    match app.create_task(input) {
        Ok(task_id) => {
            state.creating = false;
            state.task_creation = None;
            state.detail = None;
            state.screen = Screen::Queue;
            state.message = Some(format!("created task {task_id}"));
            refresh(app, state);
            state.select_task_id(&task_id);
        }
        Err(error) => {
            state.creating = false;
            state.message = Some(short_error("create task", &error));
        }
    }
}

fn refresh(app: &OrcApp, state: &mut TuiState) {
    let selected_id = state.selected_task_id().map(str::to_owned);
    let was_detail = state.screen == Screen::Detail;
    match app.operations().snapshot() {
        Ok(snapshot) => {
            state.refresh(snapshot);
            if was_detail
                && selected_id.as_deref() == state.selected_task_id()
                && let Some(task_id) = selected_id
            {
                match app.task_operations(&task_id) {
                    Ok(Some(detail)) => state.set_detail(detail),
                    Ok(None) => state.message = Some(format!("task {task_id} no longer exists")),
                    Err(error) => state.message = Some(short_error("refresh detail", &error)),
                }
            }
            if state.message.is_none() {
                state.message = Some("refreshed".into());
            }
        }
        Err(error) => state.message = Some(short_error("refresh", &error)),
    }
}

fn open_detail(app: &OrcApp, state: &mut TuiState) {
    let Some(task_id) = state.selected_task_id().map(str::to_owned) else {
        return;
    };
    match app.task_operations(&task_id) {
        Ok(Some(detail)) => state.set_detail(detail),
        Ok(None) => state.message = Some(format!("task {task_id} no longer exists")),
        Err(error) => state.message = Some(short_error("load detail", &error)),
    }
}

fn revise(app: &OrcApp, task_id: &str, feedback: &str) -> Result<String> {
    app.revise_with_previous_agent(task_id, feedback)?;
    Ok("revision finished".into())
}

fn short_error(context: &str, error: &anyhow::Error) -> String {
    short_error_text(context, &format!("{error:#}"))
}

fn short_error_text(context: &str, error: &str) -> String {
    let message = error.replace(['\n', '\r'], " ");
    let message = message.chars().take(240).collect::<String>();
    format!("{context} failed: {message}")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::thread::sleep;
    use std::time::Duration;

    use super::*;
    use crate::automated::{ActionBackend, ActionExecution};
    use crate::registry::{self, AgentAction, AgentDefinition};
    use crate::storage::Database;
    use crate::task::TaskStatus;
    use crate::validation::test_helpers::FakeValidationRunner;
    use tempfile::TempDir;

    struct ReviewBackend {
        verdict: &'static str,
    }

    impl ActionBackend for ReviewBackend {
        fn invoke(
            &self,
            _: &AgentDefinition,
            action: AgentAction,
            _: &str,
            _: Option<&str>,
            _: Option<registry::ReasoningEffort>,
        ) -> Result<ActionExecution> {
            assert_eq!(action, AgentAction::Review);
            let passing = self.verdict == "PASS";
            let blocker_id = crate::automated::blocker_id("review");
            let output = serde_json::json!({
                "verdict": self.verdict,
                "criterion_results": [{
                    "criterion_id": "acceptance-criterion-1",
                    "status": if passing { "satisfied" } else { "violated" },
                    "evidence": [{
                        "kind": "diff",
                        "reference": "current_changes.diff",
                        "explanation": "The canonical review packet contains the lifecycle evidence."
                    }],
                    "rationale": if passing { "The task is ready." } else { "The task requires revision." }
                }],
                "findings": if passing { Vec::<String>::new() } else { vec!["fix the reported issue".to_string()] },
                "blocking_findings": if passing { Vec::<String>::new() } else { vec!["fix the reported issue".to_string()] },
                "revision_feedback": if passing { serde_json::Value::Null } else { serde_json::json!("fix the reported issue") },
                "blockers": if passing { serde_json::json!([{
                    "id": blocker_id,
                    "prior_blocker_id": blocker_id,
                    "blocker_key": "review",
                    "requirement_ref": "acceptance criterion",
                    "evidence": "The reported issue is fixed.",
                    "severity": "high",
                    "acceptance_condition": "The reported issue is fixed.",
                    "status": "resolved",
                    "finding": "The reported issue is fixed."
                }]) } else { serde_json::json!([{
                    "id": blocker_id,
                    "prior_blocker_id": null,
                    "blocker_key": "review",
                    "requirement_ref": "acceptance criterion",
                    "evidence": "The reported issue remains.",
                    "severity": "high",
                    "acceptance_condition": "The reported issue is fixed.",
                    "status": "unresolved",
                    "finding": "The reported issue remains."
                }]) }
            })
            .to_string();
            Ok(ActionExecution {
                output,
                token_usage: None,
            })
        }
    }

    struct Fixture {
        _directory: TempDir,
        database_path: PathBuf,
        repository_path: PathBuf,
        registry_path: PathBuf,
        app: OrcApp,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let repository_path = directory.path().join("repo");
        fs::create_dir_all(repository_path.join(".orc")).unwrap();
        fs::write(
            repository_path.join(".orc/engineering.md"),
            "# Test engineering contract\n",
        )
        .unwrap();
        fs::write(
            repository_path.join(".orc/validation.toml"),
            "commands = []\n",
        )
        .unwrap();
        for args in [
            vec!["init", "."],
            vec!["config", "user.email", "orc-tui@example.com"],
            vec!["config", "user.name", "Orc TUI Test"],
        ] {
            assert!(
                Command::new("git")
                    .current_dir(&repository_path)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(repository_path.join("README.md"), "fixture\n").unwrap();
        assert!(
            Command::new("git")
                .current_dir(&repository_path)
                .args(["add", "."])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(&repository_path)
                .args(["commit", "-m", "fixture"])
                .status()
                .unwrap()
                .success()
        );

        let database_path = repository_path.join(".orc/orc.db");
        let registry_path = directory.path().join("agents.db");
        let database = Database::init_with_registry(&database_path, &registry_path).unwrap();
        database.create_project("tui-fixture").unwrap();
        database
            .insert_agent(&AgentDefinition {
                id: "manual-coder".into(),
                backend: "generic_manual".into(),
                execution_mode: registry::MANUAL.into(),
                display_name: "Manual coder".into(),
                enabled: true,
                priority: 100,
                capabilities: vec!["code".into(), "command_execution".into()],
                status: registry::AVAILABLE.into(),
                unavailable_reason: None,
                profile_path: None,
                model: None,
                reasoning_effort: None,
                config_metadata: None,
                quota_remaining_percent: Some(100),
                quota_reset_at: None,
                quota_checked_at: None,
                quota_source: None,
                quota_limits: None,
                actions: vec![AgentAction::Code],
            })
            .unwrap();
        database
            .insert_agent(&AgentDefinition {
                id: "reviewer".into(),
                backend: "codex".into(),
                execution_mode: registry::AUTOMATED.into(),
                display_name: "Reviewer".into(),
                enabled: true,
                priority: 100,
                capabilities: vec!["review".into()],
                status: registry::AVAILABLE.into(),
                unavailable_reason: None,
                profile_path: None,
                model: None,
                reasoning_effort: None,
                config_metadata: None,
                quota_remaining_percent: Some(100),
                quota_reset_at: None,
                quota_checked_at: None,
                quota_source: None,
                quota_limits: None,
                actions: vec![AgentAction::Review],
            })
            .unwrap();
        drop(database);
        let app =
            OrcApp::open_with_registry(&database_path, &repository_path, &registry_path).unwrap();
        Fixture {
            _directory: directory,
            database_path,
            repository_path,
            registry_path,
            app,
        }
    }

    fn manual_completion(summary: &str, file: &str) -> String {
        serde_json::json!({
            "step_results": [{
                "step_id": "implement",
                "operations_performed": ["modify"],
                "affected_files": [file],
                "observed": [summary],
                "verification_passed": ["the implementation handoff is complete"]
            }],
            "summary": summary
        })
        .to_string()
    }

    fn revision_completion(summary: &str, file: &str) -> String {
        let blocker_id = crate::automated::blocker_id("review");
        serde_json::json!({
            "completion": {
                "step_results": [{
                    "step_id": "revise",
                    "operations_performed": ["modify"],
                    "affected_files": [file],
                    "observed": [summary],
                    "verification_passed": ["the revision handoff is complete"]
                }],
                "summary": summary
            },
            "claims": [{
                "blocker_id": blocker_id,
                "status": "addressed",
                "implementation_summary": summary,
                "changed_files": [file],
                "unresolved_risk": null
            }]
        })
        .to_string()
    }

    fn wait_for_action(
        fixture: &Fixture,
        state: &mut TuiState,
        receiver: &mut Option<Receiver<ActionCompletion>>,
    ) {
        for _ in 0..100 {
            poll_action_completion(&fixture.app, state, receiver);
            if receiver.is_none() {
                return;
            }
            sleep(Duration::from_millis(10));
        }
        panic!("TUI action did not complete in the focused test window");
    }

    #[test]
    fn canonical_tui_vertical_slice_uses_nonblocking_application_actions() {
        let fixture = fixture();
        let snapshot = fixture.app.operations().snapshot().unwrap();
        let mut state = TuiState::new(Some("tui-fixture".into()), snapshot);
        state.open_task_creation();
        let form = state.task_creation.as_mut().unwrap();
        form.title = "TUI lifecycle task".into();
        form.objective = "Complete one normal task lifecycle".into();
        form.capabilities = "code, command_execution".into();
        form.context_files = "src/tui".into();
        form.expected_changes = "src/tui".into();
        let task_id = fixture.app.create_task(form.to_input()).unwrap();
        let snapshot = fixture.app.operations().snapshot().unwrap();
        state = TuiState::new(Some("tui-fixture".into()), snapshot);
        assert_eq!(state.selected_task_id(), Some(task_id.as_str()));

        let mut receiver = None;
        start_action(
            &mut state,
            &mut receiver,
            &fixture.database_path,
            &fixture.repository_path,
            &fixture.registry_path,
            LifecycleAction::Dispatch,
            None,
        );
        assert_eq!(state.running, Some(LifecycleAction::Dispatch));
        assert_eq!(state.running_task_id.as_deref(), Some(task_id.as_str()));
        assert!(receiver.is_some());
        state.move_down();
        refresh(&fixture.app, &mut state);
        assert_eq!(state.running, Some(LifecycleAction::Dispatch));
        wait_for_action(&fixture, &mut state, &mut receiver);
        assert_eq!(state.running, None);
        assert_eq!(
            fixture.app.task(&task_id).unwrap().unwrap().status,
            TaskStatus::Active,
            "action message: {:?}",
            state.message
        );
        assert_eq!(state.selected_task().unwrap().lifecycle, TaskStatus::Active);

        let detail = fixture.app.task_operations(&task_id).unwrap().unwrap();
        let run_id = detail.executions[0].id;
        fs::write(
            fixture
                .repository_path
                .join(crate::git::worktree_path_for_task(&task_id))
                .join("implementation.txt"),
            "implementation\n",
        )
        .unwrap();
        fixture
            .app
            .submit_manual_run(
                run_id,
                &manual_completion("implementation evidence", "implementation.txt"),
            )
            .unwrap();
        refresh(&fixture.app, &mut state);
        let detail = fixture.app.task_operations(&task_id).unwrap().unwrap();
        assert_eq!(detail.summary.lifecycle, TaskStatus::Review);
        assert!(detail.executions[0].output.is_some());

        let reviewer = ActionOverrides {
            agent_id: Some("reviewer".into()),
            ..ActionOverrides::default()
        };
        let (_, review) = fixture
            .app
            .automated_review_with_backend(
                &task_id,
                &reviewer,
                &ReviewBackend { verdict: "REVISE" },
                &FakeValidationRunner::success(),
            )
            .unwrap();
        assert_eq!(review.verdict, "REVISE");
        refresh(&fixture.app, &mut state);
        let detail = fixture.app.task_operations(&task_id).unwrap().unwrap();
        assert_eq!(detail.summary.lifecycle, TaskStatus::RevisionRequired);
        assert!(!detail.review_criteria.is_empty());

        start_action(
            &mut state,
            &mut receiver,
            &fixture.database_path,
            &fixture.repository_path,
            &fixture.registry_path,
            LifecycleAction::Revise,
            Some("fix the reported issue".into()),
        );
        wait_for_action(&fixture, &mut state, &mut receiver);
        assert_eq!(
            fixture.app.task(&task_id).unwrap().unwrap().status,
            TaskStatus::Active
        );
        let detail = fixture.app.task_operations(&task_id).unwrap().unwrap();
        let revision_run_id = detail.executions[0].id;
        fs::write(
            fixture
                .repository_path
                .join(crate::git::worktree_path_for_task(&task_id))
                .join("revision.txt"),
            "revision\n",
        )
        .unwrap();
        fixture
            .app
            .submit_manual_run(
                revision_run_id,
                &revision_completion("revised implementation evidence", "revision.txt"),
            )
            .unwrap();
        let (_, review) = fixture
            .app
            .automated_review_with_backend(
                &task_id,
                &reviewer,
                &ReviewBackend { verdict: "PASS" },
                &FakeValidationRunner::success(),
            )
            .unwrap();
        assert_eq!(review.verdict, "PASS");
        refresh(&fixture.app, &mut state);
        assert_eq!(
            state.selected_task().unwrap().lifecycle,
            TaskStatus::AcceptanceReady
        );

        start_action(
            &mut state,
            &mut receiver,
            &fixture.database_path,
            &fixture.repository_path,
            &fixture.registry_path,
            LifecycleAction::Accept,
            None,
        );
        wait_for_action(&fixture, &mut state, &mut receiver);
        assert_eq!(
            fixture.app.task(&task_id).unwrap().unwrap().status,
            TaskStatus::Done
        );

        let before = fixture.app.task(&task_id).unwrap().unwrap().status;
        state.message = None;
        start_action(
            &mut state,
            &mut receiver,
            &fixture.database_path,
            &fixture.repository_path,
            &fixture.registry_path,
            LifecycleAction::Dispatch,
            None,
        );
        assert!(receiver.is_none());
        assert_eq!(state.running, None);
        assert_eq!(fixture.app.task(&task_id).unwrap().unwrap().status, before);
        assert!(state.message.is_some());
    }
}
