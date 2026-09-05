use orc::app::OrcApp;
use orc::automated::{ActionBackend, ActionExecution, ActionOverrides};
use orc::controller_actions::ControllerActionIntent;
use orc::controller_continuation::{
    ControllerContinuationAllowedActions, ControllerContinuationGrantError,
    ControllerContinuationGrantState, ControllerContinuationStepResult,
};
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::registry::{self, AgentAction, AgentDefinition, ReasoningEffort};
use orc::storage::Database;
use orc::task::{CreateTaskInput, TaskPriority, TaskScopeMode};
use orc::validation::test_helpers::FakeValidationRunner;
use orc::worker::TokenUsage;
use orc::worker::test_helpers::FakeWorker;
use std::process::Command;
use tempfile::tempdir;

struct FakeRuntime {
    response: LocalInferenceResponse,
    calls: usize,
}

impl LocalInferenceRuntime for FakeRuntime {
    fn infer(
        &mut self,
        _request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        self.calls += 1;
        Ok(self.response.clone())
    }
}

fn runtime_for(step: &str) -> FakeRuntime {
    FakeRuntime {
        response: LocalInferenceResponse::structured(
            "bounded continuation recommendation",
            serde_json::json!({
                "suggested_next_step": step,
                "decision_class": "action",
                "rationale": "one supervised step"
            }),
        ),
        calls: 0,
    }
}

fn agent() -> AgentDefinition {
    AgentDefinition {
        id: "agent-a".into(),
        backend: "codex".into(),
        execution_mode: registry::AUTOMATED.into(),
        display_name: "Test agent".into(),
        enabled: true,
        priority: 1,
        capabilities: vec!["code".into(), "command_execution".into(), "review".into()],
        status: registry::AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: None,
        model: Some("test-model".into()),
        reasoning_effort: Some(ReasoningEffort::Low),
        config_metadata: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![AgentAction::Code, AgentAction::Review],
    }
}

struct ReviewBackend;

impl ActionBackend for ReviewBackend {
    fn invoke(
        &self,
        _agent: &AgentDefinition,
        action: AgentAction,
        _input: &str,
        _model: Option<&str>,
        _effort: Option<ReasoningEffort>,
    ) -> anyhow::Result<ActionExecution> {
        assert_eq!(action, AgentAction::Review);
        Ok(ActionExecution {
            output: serde_json::json!({
                "verdict": "REVISE",
                "criterion_results": [{
                    "criterion_id": "acceptance-criterion-1",
                    "status": "insufficient_evidence",
                    "evidence": [{
                        "kind": "task_contract",
                        "reference": "task_contract.objective",
                        "explanation": "Implementation evidence is still required."
                    }],
                    "rationale": "Implementation evidence is still required."
                }],
                "findings": ["implementation evidence is required"],
                "blocking_findings": ["implementation evidence is required"],
                "non_blocking_findings": [],
                "severity": "medium",
                "revision_feedback": "provide implementation evidence",
                "blockers": []
            })
            .to_string(),
            token_usage: Some(TokenUsage {
                total_tokens: 1,
                input_tokens: Some(1),
                output_tokens: Some(1),
                cached_input_tokens: None,
            }),
        })
    }
}

fn initialize_repository(directory: &tempfile::TempDir) {
    std::fs::write(directory.path().join("README.md"), "base\n").unwrap();
    for args in [
        vec!["init", "."],
        vec!["config", "user.email", "continuation@example.com"],
        vec!["config", "user.name", "Continuation Test"],
        vec!["add", "README.md"],
        vec!["commit", "-m", "base"],
    ] {
        let output = Command::new("git")
            .current_dir(directory.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn app_with_task() -> (tempfile::TempDir, OrcApp, String) {
    let directory = tempdir().unwrap();
    initialize_repository(&directory);
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("agents.db");
    let db = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = db.create_project("continuation-grant").unwrap();
    db.insert_agent(&agent()).unwrap();
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

fn open_app_with_task_status(
    status: orc::task::TaskStatus,
    directory: &tempfile::TempDir,
) -> (OrcApp, String) {
    initialize_repository(directory);
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("agents.db");
    let db = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = db.create_project("continuation-step").unwrap();
    db.insert_agent(&agent()).unwrap();
    let task_id = db
        .create_task(
            project_id,
            &CreateTaskInput {
                title: "Continuation task".into(),
                objective: "Test one supervised continuation action".into(),
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
    db.update_task_status(&task_id, status).unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (app, task_id)
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

#[test]
fn one_step_composes_dispatch_once_and_preserves_execution_evidence() {
    let directory = tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    std::fs::write(directory.path().join(".orc/engineering.md"), "# contract\n").unwrap();
    let (app, task_id) = open_app_with_task_status(orc::task::TaskStatus::Ready, &directory);
    let grant = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    let mut runtime = runtime_for("dispatch");
    let result = app
        .continue_controller_action_once(
            &task_id,
            &grant,
            &mut runtime,
            orc::controller_actions::ControllerActionExecutionContext::dispatch_with_worker(
                "agent-a",
                &FakeWorker::new_success(None),
                &FakeValidationRunner::success(),
            ),
        )
        .unwrap();
    assert!(matches!(
        result,
        ControllerContinuationStepResult::Executed {
            result: orc::controller_actions::ControllerActionExecutionResult::Executed {
                evidence: orc::controller_actions::ControllerActionExecutionEvidence {
                    lifecycle: Some(orc::task::TaskStatus::Review),
                    run_id: Some(_),
                    ..
                },
                ..
            },
            ..
        }
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
}

#[test]
fn one_step_composes_semantic_review_once() {
    let directory = tempdir().unwrap();
    let (app, task_id) = open_app_with_task_status(orc::task::TaskStatus::Review, &directory);
    let grant = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    let mut runtime = runtime_for("run_semantic_review");
    let result = app
        .continue_controller_action_once(
            &task_id,
            &grant,
            &mut runtime,
            orc::controller_actions::ControllerActionExecutionContext::semantic_review(
                ActionOverrides::default(),
                &ReviewBackend,
                &FakeValidationRunner::success(),
            ),
        )
        .unwrap();
    assert!(matches!(
        result,
        ControllerContinuationStepResult::Executed {
            result: orc::controller_actions::ControllerActionExecutionResult::Executed {
                evidence: orc::controller_actions::ControllerActionExecutionEvidence {
                    lifecycle: Some(orc::task::TaskStatus::RevisionRequired),
                    review_run_id: Some(_),
                    ..
                },
                ..
            },
            ..
        }
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
}

#[test]
fn one_step_composes_revise_once() {
    let directory = tempdir().unwrap();
    initialize_repository(&directory);
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    std::fs::write(directory.path().join(".orc/engineering.md"), "# contract\n").unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("agents.db");
    let db = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = db.create_project("continuation-revise").unwrap();
    db.insert_agent(&agent()).unwrap();
    let task_id = db
        .create_task(
            project_id,
            &CreateTaskInput {
                title: "Revision continuation".into(),
                objective: "Test one supervised revision action".into(),
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
    let (branch, worktree) = orc::git::ensure_worktree(&task_id, directory.path()).unwrap();
    let implementation = db
        .create_agent_run_with_execution(
            project_id,
            &task_id,
            "agent-a",
            registry::AUTOMATED,
            orc::storage::AgentRunExecution {
                class: "coder",
                model: Some("test-model"),
                effort: Some(ReasoningEffort::Low),
                source: "continuation-test",
            },
        )
        .unwrap();
    db.store_worktree_metadata(
        implementation,
        &task_id,
        &branch,
        &worktree.to_string_lossy(),
    )
    .unwrap();
    db.update_agent_run_status(implementation, "completed", Some("implementation"))
        .unwrap();
    let review = db
        .create_agent_run_with_execution(
            project_id,
            &task_id,
            "agent-a",
            registry::AUTOMATED,
            orc::storage::AgentRunExecution {
                class: "review",
                model: Some("test-model"),
                effort: Some(ReasoningEffort::Low),
                source: "continuation-test",
            },
        )
        .unwrap();
    db.update_agent_run_status(
        review,
        "completed",
        Some(
            &serde_json::json!({
                "verdict": "REVISE",
                "criterion_results": [{
                    "criterion_id": "acceptance-criterion-1",
                    "status": "insufficient_evidence",
                    "evidence": [{
                        "kind": "task_contract",
                        "reference": "task_contract.objective",
                        "explanation": "Implementation evidence is still required."
                    }],
                    "rationale": "Implementation evidence is still required."
                }],
                "findings": ["implementation evidence is required"],
                "blocking_findings": ["implementation evidence is required"],
                "non_blocking_findings": [],
                "severity": "medium",
                "revision_feedback": "provide implementation evidence",
                "blockers": []
            })
            .to_string(),
        ),
    )
    .unwrap();
    db.update_task_status(&task_id, orc::task::TaskStatus::RevisionRequired)
        .unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    let grant = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    let mut runtime = runtime_for("revise");
    let result = app
        .continue_controller_action_once(
            &task_id,
            &grant,
            &mut runtime,
            orc::controller_actions::ControllerActionExecutionContext::revise_with_worker(
                "agent-a",
                &FakeWorker::new_success(None),
                &FakeValidationRunner::success(),
            ),
        )
        .unwrap();
    assert!(matches!(
        result,
        ControllerContinuationStepResult::Executed {
            result: orc::controller_actions::ControllerActionExecutionResult::Executed {
                evidence: orc::controller_actions::ControllerActionExecutionEvidence {
                    lifecycle: Some(orc::task::TaskStatus::Review),
                    run_id: Some(_),
                    ..
                },
                ..
            },
            ..
        }
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
}

#[test]
fn accept_and_unsupported_recommendations_do_not_consume_or_execute() {
    let directory = tempdir().unwrap();
    let (app, task_id) = open_app_with_task_status(orc::task::TaskStatus::Review, &directory);

    let grant = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    let mut accept_runtime = runtime_for("accept");
    let accept = app
        .continue_controller_action_once(
            &task_id,
            &grant,
            &mut accept_runtime,
            orc::controller_actions::ControllerActionExecutionContext::accept(),
        )
        .unwrap();
    assert!(matches!(
        accept,
        ControllerContinuationStepResult::GrantRejected {
            reason:
                orc::controller_continuation::ControllerContinuationStepGrantRejection::UnsupportedAction {
                    action: orc::controller_actions::ControllerActionKind::Accept
                },
            ..
        }
    ));
    assert_eq!(accept_runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);

    let mut unsupported_runtime = runtime_for("wait_for_execution");
    let unsupported = app
        .continue_controller_action_once(
            &task_id,
            &grant,
            &mut unsupported_runtime,
            orc::controller_actions::ControllerActionExecutionContext::accept(),
        )
        .unwrap();
    assert!(matches!(
        unsupported,
        ControllerContinuationStepResult::NoActionableProposal { .. }
    ));
    assert_eq!(unsupported_runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
}

#[test]
fn grant_rejection_stages_never_execute_and_preserve_or_consume_only_as_defined() {
    let directory = tempdir().unwrap();
    let (app, task_id) = open_app_with_task_status(orc::task::TaskStatus::Review, &directory);

    let revoked = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    revoked.revoke().unwrap();
    let mut revoked_runtime = runtime_for("run_semantic_review");
    let revoked_result = app
        .continue_controller_action_once(
            &task_id,
            &revoked,
            &mut revoked_runtime,
            orc::controller_actions::ControllerActionExecutionContext::accept(),
        )
        .unwrap();
    assert!(matches!(
        revoked_result,
        ControllerContinuationStepResult::GrantRejected {
            reason: orc::controller_continuation::ControllerContinuationStepGrantRejection::Revoked,
            ..
        }
    ));
    assert_eq!(revoked.remaining_actions().unwrap(), 1);

    let unsupported = app
        .create_controller_continuation_grant(
            ControllerContinuationAllowedActions::from_actions([
                orc::controller_actions::ControllerActionKind::SemanticReview,
            ])
            .unwrap(),
            1,
        )
        .unwrap();
    let mut unsupported_runtime = runtime_for("dispatch");
    let unsupported_result = app
        .continue_controller_action_once(
            &task_id,
            &unsupported,
            &mut unsupported_runtime,
            orc::controller_actions::ControllerActionExecutionContext::accept(),
        )
        .unwrap();
    assert!(matches!(
        unsupported_result,
        ControllerContinuationStepResult::GrantRejected {
            reason:
                orc::controller_continuation::ControllerContinuationStepGrantRejection::UnsupportedAction {
                    action: orc::controller_actions::ControllerActionKind::Dispatch
                },
            ..
        }
    ));
    assert_eq!(unsupported.remaining_actions().unwrap(), 1);

    let illegal = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    let mut illegal_runtime = runtime_for("revise");
    let illegal_result = app
        .continue_controller_action_once(
            &task_id,
            &illegal,
            &mut illegal_runtime,
            orc::controller_actions::ControllerActionExecutionContext::accept(),
        )
        .unwrap();
    assert!(matches!(
        illegal_result,
        ControllerContinuationStepResult::GrantRejected {
            reason:
                orc::controller_continuation::ControllerContinuationStepGrantRejection::CanonicallyIllegal,
            ..
        }
    ));
    assert_eq!(illegal.remaining_actions().unwrap(), 1);
}

#[test]
fn execution_context_mismatch_and_execution_failure_do_not_refund_budget() {
    let directory = tempdir().unwrap();
    let (app, task_id) = open_app_with_task_status(orc::task::TaskStatus::Review, &directory);

    let mismatch_grant = app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    let mut mismatch_runtime = runtime_for("run_semantic_review");
    let mismatch = app
        .continue_controller_action_once(
            &task_id,
            &mismatch_grant,
            &mut mismatch_runtime,
            orc::controller_actions::ControllerActionExecutionContext::accept(),
        )
        .unwrap();
    assert!(matches!(
        mismatch,
        ControllerContinuationStepResult::ExecutionRejected {
            result: orc::controller_actions::ControllerActionExecutionResult::ExecutionFailed {
                stage: orc::controller_actions::ControllerActionExecutionStage::ExecutionContext,
                ..
            },
            ..
        }
    ));
    assert_eq!(mismatch_runtime.calls, 1);
    assert_eq!(mismatch_grant.remaining_actions().unwrap(), 0);

    let failure_directory = tempdir().unwrap();
    std::fs::create_dir_all(failure_directory.path().join(".orc")).unwrap();
    std::fs::write(
        failure_directory.path().join(".orc/engineering.md"),
        "# contract\n",
    )
    .unwrap();
    let (failure_app, failure_task) =
        open_app_with_task_status(orc::task::TaskStatus::Ready, &failure_directory);
    let failure_grant = failure_app
        .create_controller_continuation_grant(ControllerContinuationAllowedActions::routine(), 1)
        .unwrap();
    let mut failure_runtime = runtime_for("dispatch");
    let failure = failure_app
        .continue_controller_action_once(
            &failure_task,
            &failure_grant,
            &mut failure_runtime,
            orc::controller_actions::ControllerActionExecutionContext::dispatch_with_worker(
                "agent-a",
                &FakeWorker::new_failure("execution failed".into()),
                &FakeValidationRunner::success(),
            ),
        )
        .unwrap();
    assert!(matches!(
        failure,
        ControllerContinuationStepResult::ExecutionRejected {
            result: orc::controller_actions::ControllerActionExecutionResult::ExecutionFailed {
                stage: orc::controller_actions::ControllerActionExecutionStage::CanonicalMutation,
                ..
            },
            ..
        }
    ));
    assert_eq!(failure_runtime.calls, 1);
    assert_eq!(failure_grant.remaining_actions().unwrap(), 0);
}
