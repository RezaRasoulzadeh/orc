use orc::app::OrcApp;
use orc::controller_memory_capture::{
    ControllerMemoryCaptureCandidate, ControllerMemoryCaptureRequest,
    ControllerMemoryCaptureStepError, ControllerMemoryCaptureStepResult,
};
use orc::controller_memory_capture_grant::{
    ControllerMemoryCaptureGrantError, ControllerMemoryCaptureGrantState,
};
use orc::local_runtime::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
};
use orc::memory::{MemoryDraft, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope};
use orc::storage::Database;
use tempfile::TempDir;

struct CountingRuntime {
    response: Result<LocalInferenceResponse, LocalInferenceError>,
    calls: usize,
}

struct ExecutionFailureRuntime {
    response: LocalInferenceResponse,
    database_path: std::path::PathBuf,
    calls: usize,
}

impl LocalInferenceRuntime for ExecutionFailureRuntime {
    fn infer(
        &mut self,
        _request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        self.calls += 1;
        let connection = rusqlite::Connection::open(&self.database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_capture_step BEFORE INSERT ON project_memories
                 BEGIN SELECT RAISE(ABORT, 'capture step execution failure'); END;",
            )
            .unwrap();
        Ok(self.response.clone())
    }
}

impl LocalInferenceRuntime for CountingRuntime {
    fn infer(
        &mut self,
        _request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        self.calls += 1;
        self.response.clone()
    }
}

fn open_app() -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project_id = db.create_project("capture-step-test").unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    (directory, app, project_id)
}

fn request(project_id: i64, kind: MemoryKind, subject: &str) -> ControllerMemoryCaptureRequest {
    let scope = if kind.is_global() {
        MemoryScope::Global
    } else {
        MemoryScope::Project { project_id }
    };
    ControllerMemoryCaptureRequest::from_candidate(ControllerMemoryCaptureCandidate {
        draft: MemoryDraft {
            kind,
            scope,
            subject: subject.into(),
            content: format!("capture content for {subject}"),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ProjectFact,
                source_reference: Some("controller:capture-step-test".into()),
            },
            confidence: Some(0.9),
        },
        source_facts: vec!["explicit deterministic test candidate".into()],
    })
}

fn proposing_runtime(request: &ControllerMemoryCaptureRequest) -> CountingRuntime {
    CountingRuntime {
        response: Ok(LocalInferenceResponse::structured(
            "propose",
            serde_json::json!({
                "decision": "propose_mutation",
                "intent": {
                    "operation": "create",
                    "draft": request.candidate.draft,
                }
            }),
        )),
        calls: 0,
    }
}

fn ignore_runtime() -> CountingRuntime {
    CountingRuntime {
        response: Ok(LocalInferenceResponse::structured(
            "ignore",
            serde_json::json!({"decision": "ignore"}),
        )),
        calls: 0,
    }
}

fn memory_count(app: &OrcApp) -> usize {
    app.memories().unwrap().list(None, false).unwrap().len()
}

#[test]
fn ignore_is_one_inference_non_mutating_and_free() {
    let (_directory, app, project_id) = open_app();
    let request = request(project_id, MemoryKind::Project, "ignored");
    let grant = app.create_controller_memory_capture_grant(1).unwrap();
    let mut runtime = ignore_runtime();

    assert!(matches!(
        app.capture_controller_memory_once(&request, &grant, &mut runtime),
        ControllerMemoryCaptureStepResult::Ignored
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(memory_count(&app), 0);
}

#[test]
fn project_and_episodic_capture_each_use_one_canonical_mutation_and_unit() {
    for kind in [MemoryKind::Project, MemoryKind::Episodic] {
        let (_directory, app, project_id) = open_app();
        let request = request(project_id, kind, kind.as_str());
        let grant = app.create_controller_memory_capture_grant(1).unwrap();
        let mut runtime = proposing_runtime(&request);

        let result = app.capture_controller_memory_once(&request, &grant, &mut runtime);
        let ControllerMemoryCaptureStepResult::Mutated { result } = result else {
            panic!("eligible capture should mutate");
        };
        assert!(matches!(
            result.canonical_result(),
            orc::controller_memory_mutation::ControllerMemoryMutationExecutionResult::Mutated { .. }
        ));
        assert_eq!(runtime.calls, 1);
        assert_eq!(grant.remaining_actions().unwrap(), 0);
        assert_eq!(grant.state(), ControllerMemoryCaptureGrantState::Exhausted);
        assert_eq!(memory_count(&app), 1);
    }
}

#[test]
fn malformed_output_and_runtime_failure_stop_before_authorization() {
    let (_directory, app, project_id) = open_app();
    let request = request(project_id, MemoryKind::Project, "malformed");
    let grant = app.create_controller_memory_capture_grant(1).unwrap();
    let mut malformed = CountingRuntime {
        response: Ok(LocalInferenceResponse::structured(
            "malformed",
            serde_json::json!({"decision": "unknown"}),
        )),
        calls: 0,
    };
    assert!(matches!(
        app.capture_controller_memory_once(&request, &grant, &mut malformed),
        ControllerMemoryCaptureStepResult::Rejected {
            error: ControllerMemoryCaptureStepError::Capture(_),
        }
    ));
    assert_eq!(malformed.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(memory_count(&app), 0);

    let mut runtime_failure = CountingRuntime {
        response: Err(LocalInferenceError::Backend("stopped".into())),
        calls: 0,
    };
    assert!(matches!(
        app.capture_controller_memory_once(&request, &grant, &mut runtime_failure),
        ControllerMemoryCaptureStepResult::Rejected {
            error: ControllerMemoryCaptureStepError::Capture(_),
        }
    ));
    assert_eq!(runtime_failure.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(memory_count(&app), 0);
}

#[test]
fn proposal_and_global_kind_rejections_are_free_and_non_mutating() {
    let (_directory, app, project_id) = open_app();
    let grant = app.create_controller_memory_capture_grant(3).unwrap();
    let outside = request(project_id + 1, MemoryKind::Project, "outside");
    let mut outside_runtime = proposing_runtime(&outside);
    assert!(matches!(
        app.capture_controller_memory_once(&outside, &grant, &mut outside_runtime),
        ControllerMemoryCaptureStepResult::Rejected {
            error: ControllerMemoryCaptureStepError::Proposal(_),
        }
    ));
    assert_eq!(outside_runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 3);
    assert_eq!(memory_count(&app), 0);

    for kind in [MemoryKind::User, MemoryKind::Experience] {
        let global = request(project_id, kind, kind.as_str());
        let mut runtime = proposing_runtime(&global);
        assert!(matches!(
            app.capture_controller_memory_once(&global, &grant, &mut runtime),
            ControllerMemoryCaptureStepResult::Rejected {
                error: ControllerMemoryCaptureStepError::Grant(
                    ControllerMemoryCaptureGrantError::UnsupportedKind(_)
                ),
            }
        ));
        assert_eq!(runtime.calls, 1);
        assert_eq!(grant.remaining_actions().unwrap(), 3);
        assert_eq!(memory_count(&app), 0);
    }
}

#[test]
fn exhausted_and_revoked_grants_do_not_mutate_or_retry() {
    let (_directory, app, project_id) = open_app();
    let request = request(project_id, MemoryKind::Project, "exhaustion");
    let grant = app.create_controller_memory_capture_grant(1).unwrap();
    let mut first_runtime = proposing_runtime(&request);
    assert!(matches!(
        app.capture_controller_memory_once(&request, &grant, &mut first_runtime),
        ControllerMemoryCaptureStepResult::Mutated { .. }
    ));
    assert_eq!(first_runtime.calls, 1);
    assert_eq!(memory_count(&app), 1);

    let mut exhausted_runtime = proposing_runtime(&request);
    assert!(matches!(
        app.capture_controller_memory_once(&request, &grant, &mut exhausted_runtime),
        ControllerMemoryCaptureStepResult::Rejected {
            error: ControllerMemoryCaptureStepError::Grant(
                ControllerMemoryCaptureGrantError::Exhausted
            ),
        }
    ));
    assert_eq!(exhausted_runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(memory_count(&app), 1);

    let revoked = app.create_controller_memory_capture_grant(1).unwrap();
    revoked.revoke().unwrap();
    let mut revoked_runtime = proposing_runtime(&request);
    assert!(matches!(
        app.capture_controller_memory_once(&request, &revoked, &mut revoked_runtime),
        ControllerMemoryCaptureStepResult::Rejected {
            error: ControllerMemoryCaptureStepError::Grant(
                ControllerMemoryCaptureGrantError::Revoked
            ),
        }
    ));
    assert_eq!(revoked_runtime.calls, 1);
    assert_eq!(revoked.remaining_actions().unwrap(), 1);
    assert_eq!(memory_count(&app), 1);
}

#[test]
fn cloned_grant_accounts_two_single_steps_without_reset() {
    let (_directory, app, project_id) = open_app();
    let grant = app.create_controller_memory_capture_grant(2).unwrap();
    let clone = grant.clone();
    let first = request(project_id, MemoryKind::Project, "first");
    let second = request(project_id, MemoryKind::Episodic, "second");
    let mut first_runtime = proposing_runtime(&first);
    let mut second_runtime = proposing_runtime(&second);

    assert!(matches!(
        app.capture_controller_memory_once(&first, &grant, &mut first_runtime),
        ControllerMemoryCaptureStepResult::Mutated { .. }
    ));
    assert!(matches!(
        app.capture_controller_memory_once(&second, &clone, &mut second_runtime),
        ControllerMemoryCaptureStepResult::Mutated { .. }
    ));
    assert_eq!(first_runtime.calls, 1);
    assert_eq!(second_runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(clone.remaining_actions().unwrap(), 0);
    assert_eq!(memory_count(&app), 2);
}

#[test]
fn post_mint_execution_failure_consumes_once_without_refund_or_retry() {
    let (directory, app, project_id) = open_app();
    let request = request(project_id, MemoryKind::Project, "execution-failure");
    let grant = app.create_controller_memory_capture_grant(1).unwrap();
    let mut runtime = ExecutionFailureRuntime {
        response: proposing_runtime(&request).response.unwrap(),
        database_path: directory.path().join(".orc/orc.db"),
        calls: 0,
    };

    let result = app.capture_controller_memory_once(&request, &grant, &mut runtime);
    let ControllerMemoryCaptureStepResult::Rejected { error } = result else {
        panic!("storage failure should reject execution");
    };
    assert_eq!(
        error.stage(),
        orc::controller_memory_capture::ControllerMemoryCaptureStepStage::Execution
    );
    let ControllerMemoryCaptureStepError::Execution(error) = error else {
        panic!("storage failure should be an execution rejection");
    };
    assert!(matches!(
        error.canonical_result(),
        orc::controller_memory_mutation::ControllerMemoryMutationExecutionResult::MutationFailed { .. }
    ));
    assert_eq!(runtime.calls, 1);
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(memory_count(&app), 0);
}

#[test]
fn composed_result_types_expose_only_matching_success_or_rejection_states() {
    let (_directory, app, project_id) = open_app();
    let request = request(project_id, MemoryKind::Project, "state-safe");
    let grant = app.create_controller_memory_capture_grant(1).unwrap();
    let mut runtime = proposing_runtime(&request);

    let result = app.capture_controller_memory_once(&request, &grant, &mut runtime);
    let ControllerMemoryCaptureStepResult::Mutated { result } = result else {
        panic!("eligible capture should mutate");
    };
    assert!(matches!(
        result.canonical_result(),
        orc::controller_memory_mutation::ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));

    let mut failing_runtime = ExecutionFailureRuntime {
        response: proposing_runtime(&request).response.unwrap(),
        database_path: _directory.path().join(".orc/orc.db"),
        calls: 0,
    };
    let second_grant = app.create_controller_memory_capture_grant(1).unwrap();
    let result = app.capture_controller_memory_once(&request, &second_grant, &mut failing_runtime);
    let ControllerMemoryCaptureStepResult::Rejected { error } = result else {
        panic!("storage failure should reject execution");
    };
    let ControllerMemoryCaptureStepError::Execution(error) = error else {
        panic!("storage failure should be an execution rejection");
    };
    assert!(!matches!(
        error.canonical_result(),
        orc::controller_memory_mutation::ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));
}
