use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use orc::controller_experience_intake::{
    CONTROLLER_WORKFLOW_INTAKE_EXPERIENCE_CAPABILITY, ControllerExperienceIntakeRequest,
};
use orc::controller_intake::{
    CONTROLLER_INTAKE_REQUEST_VERSION, ControllerIntakeCount, ControllerIntakeDecision,
    ControllerIntakeDiscovery, ControllerIntakeFact, ControllerIntakeInput,
    ControllerIntakeRequest, ControllerIntakeResult, ControllerIntakeState,
};
use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::protocol::{ExecutionHints, TaskProposal};
use orc::storage::Database;
use orc::task::{TaskPriority, TaskScopeMode};
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    database.create_project("intake curation").unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app)
}

fn input() -> ControllerIntakeInput {
    ControllerIntakeInput {
        current_request: ControllerIntakeRequest {
            packet_version: CONTROLLER_INTAKE_REQUEST_VERSION,
            kind: "workflow_intake".into(),
            project_name: "intake curation".into(),
            engineering_contract: "Keep the intake boundary read-only.".into(),
            objective: "Curate one already-produced intake result.".into(),
            project_facts: vec![ControllerIntakeFact {
                key: "authority".into(),
                value: "the typed intake packet is authoritative".into(),
            }],
            discovery: ControllerIntakeDiscovery {
                fingerprint: "intake-curation-fingerprint".into(),
                technology_stack: vec!["Rust".into()],
                important_files: vec!["src/controller_intake.rs".into()],
                architecture_boundaries: vec!["controller".into()],
                unknowns_and_risks: vec![],
                validation_commands: vec!["cargo test --lib".into()],
                state: ControllerIntakeState {
                    task_counts: vec![ControllerIntakeCount {
                        status: "ready".into(),
                        count: 1,
                    }],
                    ready_tasks: vec![],
                    active_tasks: vec![],
                    review_tasks: vec![],
                    blocked_tasks: vec![],
                },
            },
            operator_resolution: Some("Use explicit verification metadata.".into()),
        },
        memory: ControllerMemoryContext::empty(),
    }
}

fn task(local_id: &str) -> TaskProposal {
    TaskProposal {
        local_id: local_id.into(),
        title: "Preserve the exact intake proposal".into(),
        objective: "Keep the complete canonical proposal.".into(),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on: vec![],
        capabilities: vec!["code".into()],
        scope_mode: Some(TaskScopeMode::Module),
        context_files: vec!["src/controller_intake.rs".into()],
        expected_changes: vec!["The explicit adapter".into()],
        unchanged: vec!["The intake inference contract".into()],
        acceptance_criteria: vec!["The exact typed output is persisted".into()],
        required_tests: vec!["Focused curation tests".into()],
        validation: vec!["cargo test --lib".into()],
        execution_hints: ExecutionHints::default(),
        risk_factors: vec![],
    }
}

fn result(decision: ControllerIntakeDecision, details: &str) -> ControllerIntakeResult {
    ControllerIntakeResult {
        decision,
        details: details.into(),
        direct_tasks: vec![],
    }
}

fn direct_result(details: &str) -> ControllerIntakeResult {
    ControllerIntakeResult {
        decision: ControllerIntakeDecision::DirectTasks,
        details: details.into(),
        direct_tasks: vec![task("intake-task-1")],
    }
}

fn request(
    input: ControllerIntakeInput,
    observed: ControllerIntakeResult,
    accepted: ControllerIntakeResult,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperienceIntakeRequest {
    ControllerExperienceIntakeRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit intake curation evidence".into(),
        },
    }
}

fn all(app: &OrcApp) -> Vec<orc::controller_experience::ControllerExperienceExample> {
    app.list_controller_experience_examples(&ControllerExperienceExampleQuery {
        capability: None,
        lifecycle: ControllerExperienceLifecycleFilter::All,
        limit: 128,
        offset: 0,
    })
    .unwrap()
}

fn assert_zero_rows(app: &OrcApp, request: &ControllerExperienceIntakeRequest) {
    assert!(
        app.create_controller_intake_experience_example(request)
            .is_err()
    );
    assert!(all(app).is_empty());
}

#[test]
fn equal_valid_result_persists_one_exact_non_correction_example() {
    let (_directory, app) = fixture();
    let input = input();
    let expected_input = serde_json::to_value(&input).unwrap();
    let output = result(
        ControllerIntakeDecision::PlanRequired,
        "The objective needs a Plan.",
    );
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_intake_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.accepted_output,
        serde_json::to_value(&request.accepted).unwrap()
    );
    assert_eq!(
        stored.capability,
        CONTROLLER_WORKFLOW_INTAKE_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert!(stored.correction.is_none());
}

#[test]
fn direct_tasks_persist_complete_canonical_proposals() {
    let (_directory, app) = fixture();
    let output = direct_result("The direct task proposal is ready.");
    let request = request(
        input(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_intake_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(
        stored.accepted_output,
        serde_json::to_value(output).unwrap()
    );
    assert_eq!(
        stored.accepted_output["direct_tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        stored.accepted_output["direct_tasks"][0]["local_id"],
        "intake-task-1"
    );
}

#[test]
fn corrected_result_preserves_exact_complete_observed_and_accepted_outputs() {
    let (_directory, app) = fixture();
    let observed = result(ControllerIntakeDecision::PlanRequired, "Observed decision.");
    let accepted = direct_result("Accepted direct task decision.");
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = request(
        input(),
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:intake-1".into(),
        reason: "The operator accepted the direct task proposal.".into(),
    });

    let stored = app
        .create_controller_intake_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn input_projection_is_exact_and_no_caller_capability_is_accepted() {
    let (_directory, app) = fixture();
    let input = input();
    let expected = serde_json::to_value(&input).unwrap();
    let request = request(
        input,
        result(
            ControllerIntakeDecision::UserDecisionRequired,
            "Ask the operator.",
        ),
        result(
            ControllerIntakeDecision::UserDecisionRequired,
            "Ask the operator.",
        ),
        ControllerExperienceOutcome::Accepted,
    );
    let stored = app
        .create_controller_intake_experience_example(&request)
        .unwrap();
    assert_eq!(stored.input, expected);
    assert_eq!(stored.capability, "controller.workflow_intake");
}

#[test]
fn malformed_input_and_observed_or_accepted_results_write_zero_rows() {
    let (_directory, app) = fixture();
    let valid = result(ControllerIntakeDecision::PlanRequired, "valid");

    let mut malformed_input = input();
    malformed_input.current_request.packet_version = 0;
    assert_zero_rows(
        &app,
        &request(
            malformed_input,
            valid.clone(),
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut empty_observed = valid.clone();
    empty_observed.details.clear();
    assert_zero_rows(
        &app,
        &request(
            input(),
            empty_observed,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut oversized_observed = valid.clone();
    oversized_observed.details = "x".repeat(2049);
    assert_zero_rows(
        &app,
        &request(
            input(),
            oversized_observed,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut empty_accepted = valid.clone();
    empty_accepted.details.clear();
    assert_zero_rows(
        &app,
        &request(
            input(),
            valid.clone(),
            empty_accepted,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut oversized_accepted = valid.clone();
    oversized_accepted.details = "x".repeat(2049);
    assert_zero_rows(
        &app,
        &request(
            input(),
            valid,
            oversized_accepted,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn intake_result_contract_rejects_task_shape_and_nested_proposal_errors() {
    let (_directory, app) = fixture();
    let valid = result(ControllerIntakeDecision::PlanRequired, "valid");

    let empty_direct = ControllerIntakeResult {
        decision: ControllerIntakeDecision::DirectTasks,
        details: "missing tasks".into(),
        direct_tasks: vec![],
    };
    assert_zero_rows(
        &app,
        &request(
            input(),
            empty_direct,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let non_direct_with_tasks = ControllerIntakeResult {
        decision: ControllerIntakeDecision::PlanRequired,
        details: "tasks are not allowed here".into(),
        direct_tasks: vec![task("unexpected")],
    };
    assert_zero_rows(
        &app,
        &request(
            input(),
            non_direct_with_tasks,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut invalid_task = task("invalid");
    invalid_task.acceptance_criteria.clear();
    let invalid_nested = ControllerIntakeResult {
        decision: ControllerIntakeDecision::DirectTasks,
        details: "invalid nested proposal".into(),
        direct_tasks: vec![invalid_task],
    };
    assert_zero_rows(
        &app,
        &request(
            input(),
            invalid_nested,
            valid,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let valid_direct = direct_result("valid direct result");
    let invalid_accepted_direct = ControllerIntakeResult {
        decision: ControllerIntakeDecision::DirectTasks,
        details: "accepted result is missing tasks".into(),
        direct_tasks: vec![],
    };
    assert_zero_rows(
        &app,
        &request(
            input(),
            valid_direct.clone(),
            invalid_accepted_direct,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let invalid_accepted_non_direct = ControllerIntakeResult {
        decision: ControllerIntakeDecision::UserDecisionRequired,
        details: "accepted result has forbidden tasks".into(),
        direct_tasks: vec![task("forbidden")],
    };
    assert_zero_rows(
        &app,
        &request(
            input(),
            valid_direct.clone(),
            invalid_accepted_non_direct,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut invalid_accepted_task = task("invalid-accepted");
    invalid_accepted_task.validation.clear();
    let invalid_accepted_nested = ControllerIntakeResult {
        decision: ControllerIntakeDecision::DirectTasks,
        details: "accepted result has an invalid proposal".into(),
        direct_tasks: vec![invalid_accepted_task],
    };
    assert_zero_rows(
        &app,
        &request(
            input(),
            valid_direct,
            invalid_accepted_nested,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn correction_semantics_reject_mismatches_and_equal_corrections() {
    let (_directory, app) = fixture();
    let output = result(ControllerIntakeDecision::PlanRequired, "same");

    let mut equal_with_correction = request(
        input(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::to_value(&output).unwrap(),
        operator_reference: "operator:intake".into(),
        reason: "not a correction".into(),
    });
    assert_zero_rows(&app, &equal_with_correction);

    assert_zero_rows(
        &app,
        &request(
            input(),
            output.clone(),
            output.clone(),
            ControllerExperienceOutcome::Corrected,
        ),
    );

    let accepted = result(ControllerIntakeDecision::UserDecisionRequired, "different");
    assert_zero_rows(
        &app,
        &request(
            input(),
            output.clone(),
            accepted.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );
    assert_zero_rows(
        &app,
        &request(
            input(),
            output.clone(),
            accepted.clone(),
            ControllerExperienceOutcome::Corrected,
        ),
    );

    let mut wrong_original = request(
        input(),
        output,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    wrong_original.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"not": "the observed result"}),
        operator_reference: "operator:intake".into(),
        reason: "mismatched evidence".into(),
    });
    assert_zero_rows(&app, &wrong_original);
}

#[test]
fn invalid_m08_metadata_and_bounds_write_zero_rows() {
    let (_directory, app) = fixture();
    let output = result(ControllerIntakeDecision::PlanRequired, "valid");

    let mut invalid_quality = request(
        input(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_quality.quality.score = 101;
    assert_zero_rows(&app, &invalid_quality);

    let mut invalid_provenance = request(
        input(),
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    invalid_provenance.provenance.project_id = Some(0);
    assert_zero_rows(&app, &invalid_provenance);

    let mut oversized_input = input();
    oversized_input.current_request.project_facts = (0..10)
        .map(|index| ControllerIntakeFact {
            key: format!("key-{index}"),
            value: "x".repeat(1800),
        })
        .collect();
    assert!(oversized_input.validate().is_ok());
    assert_zero_rows(
        &app,
        &request(
            oversized_input,
            result(ControllerIntakeDecision::PlanRequired, "valid"),
            result(ControllerIntakeDecision::PlanRequired, "valid"),
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn genuine_production_input_bound_rejection_writes_zero_rows() {
    let (_directory, app) = fixture();
    let mut large = input();
    large.current_request.project_facts = (0..11)
        .map(|index| ControllerIntakeFact {
            key: format!("key-{index}-{}", "k".repeat(1700)),
            value: "v".repeat(1700),
        })
        .collect();
    large.memory = ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items: (1..=8)
            .map(|id| ControllerMemoryItem {
                id: MemoryId::Global(id),
                kind: MemoryKind::User,
                scope: MemoryScope::Global,
                authority: ControllerMemoryAuthority::DurableUser,
                subject: format!("subject-{id}"),
                content: "memory-content-".to_owned() + &"m".repeat(3500),
                provenance: MemoryProvenance {
                    kind: MemoryProvenanceKind::Operator,
                    source_reference: None,
                },
                confidence: None,
                lifecycle: MemoryLifecycle::Active,
                supersedes: None,
            })
            .collect(),
    };
    assert!(large.current_request.validate().is_ok());
    assert!(large.memory.validate().is_ok());
    assert!(large.validate().is_err());
    assert_zero_rows(
        &app,
        &request(
            large,
            result(ControllerIntakeDecision::PlanRequired, "valid"),
            result(ControllerIntakeDecision::PlanRequired, "valid"),
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn distinct_m08_payload_bound_rejection_keeps_valid_intake_contracts_and_writes_zero_rows() {
    let (_directory, app) = fixture();
    let mut large = input();
    large.current_request.project_facts = (0..10)
        .map(|index| ControllerIntakeFact {
            key: format!("key-{index}"),
            value: "x".repeat(1800),
        })
        .collect();
    let observed = result(ControllerIntakeDecision::PlanRequired, "valid");
    let accepted = result(ControllerIntakeDecision::PlanRequired, "valid");
    assert!(large.validate().is_ok());
    assert!(observed.validate().is_ok());
    assert!(accepted.validate().is_ok());
    assert_zero_rows(
        &app,
        &request(
            large,
            observed,
            accepted,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn successful_call_creates_exactly_one_row() {
    let (_directory, app) = fixture();
    let output = result(ControllerIntakeDecision::UserDecisionRequired, "one row");
    let request = request(
        input(),
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    app.create_controller_intake_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(
        app.create_controller_intake_experience_example(&request)
            .unwrap()
            .id,
        2
    );
    assert_eq!(all(&app).len(), 2);
}
