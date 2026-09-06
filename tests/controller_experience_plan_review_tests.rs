use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use orc::controller_experience_plan_review::{
    CONTROLLER_PLAN_REVIEW_EXPERIENCE_CAPABILITY, ControllerExperiencePlanReviewRequest,
};
use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_plan_review::{
    CONTROLLER_PLAN_REVIEW_REQUEST_VERSION, ControllerPlanReviewCount,
    ControllerPlanReviewDecision, ControllerPlanReviewError, ControllerPlanReviewInput,
    ControllerPlanReviewRequest, ControllerPlanReviewResult, ControllerPlanReviewState,
    ControllerPlanReviewTask,
};
use orc::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::protocol::{ExecutionHints, PROTOCOL_VERSION, PlanResponse, TaskProposal};
use orc::storage::Database;
use orc::storage::db::{PlanOrigin, PlanStatus};
use orc::task::{TaskPriority, TaskScopeMode};
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    database.create_project("Plan review curation").unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app)
}

fn task() -> TaskProposal {
    TaskProposal {
        local_id: "review-task".into(),
        title: "Preserve the reviewed Plan contract".into(),
        objective: "Keep the current Plan reviewable and bounded.".into(),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on: vec![],
        capabilities: vec!["code".into()],
        scope_mode: Some(TaskScopeMode::Module),
        context_files: vec!["src/controller_plan_review.rs".into()],
        expected_changes: vec!["The explicit Plan-review curation adapter".into()],
        unchanged: vec!["The Plan-review inference contract".into()],
        acceptance_criteria: vec!["The exact review result is persisted".into()],
        required_tests: vec!["Focused Plan-review curation tests".into()],
        validation: vec!["cargo test --lib".into()],
        execution_hints: ExecutionHints::default(),
        risk_factors: vec![],
    }
}

fn input() -> ControllerPlanReviewInput {
    ControllerPlanReviewInput {
        current_request: ControllerPlanReviewRequest {
            packet_version: CONTROLLER_PLAN_REVIEW_REQUEST_VERSION,
            plan_id: 42,
            plan_version: 3,
            plan_status: PlanStatus::UnderReview,
            plan_origin: PlanOrigin::Controller,
            plan: PlanResponse {
                protocol_version: PROTOCOL_VERSION,
                objective: "Review one bounded Plan proposal.".into(),
                assumptions: vec!["The current Plan is the review authority.".into()],
                risks: vec!["The Plan may need one focused correction.".into()],
                questions: vec![],
                tasks: vec![task()],
            },
            project_name: Some("Plan review curation".into()),
            current_state: ControllerPlanReviewState {
                task_counts: vec![ControllerPlanReviewCount {
                    status: "ready".into(),
                    count: 1,
                }],
                ready_tasks: vec![ControllerPlanReviewTask {
                    id: "task-1".into(),
                    title: "Current task".into(),
                    status: "ready".into(),
                }],
                active_tasks: vec![],
                review_tasks: vec![],
                blocked_tasks: vec![],
                usable_agent_count: 1,
                busy_agent_count: 0,
                quota_reserve_percent: 10,
            },
            operator_resolution: Some("Operator requires explicit Plan-review evidence.".into()),
        },
        memory: memory_context(vec![memory_item(
            1,
            "review-guidance",
            "Review only the supplied Plan.",
        )]),
    }
}

fn memory_item(id: i64, subject: &str, content: &str) -> ControllerMemoryItem {
    ControllerMemoryItem {
        id: MemoryId::Global(id),
        kind: MemoryKind::User,
        scope: MemoryScope::Global,
        authority: ControllerMemoryAuthority::DurableUser,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Operator,
            source_reference: Some(format!("test:{subject}")),
        },
        confidence: Some(0.9),
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
    }
}

fn memory_context(items: Vec<ControllerMemoryItem>) -> ControllerMemoryContext {
    ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items,
    }
}

fn result(
    decision: ControllerPlanReviewDecision,
    details: &str,
    revision_feedback: Option<&str>,
) -> ControllerPlanReviewResult {
    ControllerPlanReviewResult {
        decision,
        details: details.into(),
        revision_feedback: revision_feedback.map(str::to_owned),
    }
}

fn request(
    input: ControllerPlanReviewInput,
    observed: ControllerPlanReviewResult,
    accepted: ControllerPlanReviewResult,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperiencePlanReviewRequest {
    ControllerExperiencePlanReviewRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit Plan-review curation evidence".into(),
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

fn assert_zero_rows(app: &OrcApp, request: &ControllerExperiencePlanReviewRequest) {
    assert!(
        app.create_controller_plan_review_experience_example(request)
            .is_err()
    );
    assert!(all(app).is_empty());
}

#[test]
fn equal_approve_result_persists_one_exact_active_non_correction_example() {
    let (_directory, app) = fixture();
    let input = input();
    let expected_input = serde_json::to_value(&input).unwrap();
    let output = result(
        ControllerPlanReviewDecision::Approve,
        "The Plan is coherent.",
        None,
    );
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_plan_review_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.accepted_output,
        serde_json::to_value(&request.accepted).unwrap()
    );
    assert_eq!(
        stored.capability,
        CONTROLLER_PLAN_REVIEW_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert!(stored.correction.is_none());
}

#[test]
fn equal_operator_decision_result_persists_exactly() {
    let (_directory, app) = fixture();
    let output = result(
        ControllerPlanReviewDecision::OperatorDecisionRequired,
        "The operator must resolve an ambiguity.",
        None,
    );
    let request = request(
        input(),
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    let stored = app
        .create_controller_plan_review_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(
        stored.accepted_output,
        serde_json::to_value(request.accepted).unwrap()
    );
    assert!(stored.correction.is_none());
}

#[test]
fn revise_plan_result_persists_complete_feedback() {
    let (_directory, app) = fixture();
    let output = result(
        ControllerPlanReviewDecision::RevisePlan,
        "The Plan omits a required implementation task.",
        Some("Add the missing implementation task and its focused validation."),
    );
    let request = request(
        input(),
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    let stored = app
        .create_controller_plan_review_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(
        stored.accepted_output,
        serde_json::to_value(request.accepted).unwrap()
    );
    assert_eq!(
        stored.accepted_output["revision_feedback"],
        "Add the missing implementation task and its focused validation."
    );
}

#[test]
fn corrected_review_preserves_exact_complete_observed_and_accepted_results() {
    let (_directory, app) = fixture();
    let observed = result(
        ControllerPlanReviewDecision::Approve,
        "The Plan is ready.",
        None,
    );
    let accepted = result(
        ControllerPlanReviewDecision::RevisePlan,
        "The Plan needs one correction.",
        Some("Add coverage for the missing boundary."),
    );
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
        operator_reference: "operator:plan-review-1".into(),
        reason: "The accepted review identified a missing boundary.".into(),
    });

    let stored = app
        .create_controller_plan_review_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn exact_input_projection_preserves_plan_identity_state_resolution_and_memory() {
    let (_directory, app) = fixture();
    let input = input();
    let expected = serde_json::to_value(&input).unwrap();
    let output = result(ControllerPlanReviewDecision::Approve, "valid", None);
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    let stored = app
        .create_controller_plan_review_experience_example(&request)
        .unwrap();
    assert_eq!(stored.input, expected);
    assert_eq!(stored.input["current_request"]["plan_id"], 42);
    assert_eq!(stored.input["current_request"]["plan_version"], 3);
    assert_eq!(
        stored.input["current_request"]["plan_status"],
        "UnderReview"
    );
    assert_eq!(stored.input["current_request"]["plan_origin"], "controller");
    assert_eq!(
        stored.input["current_request"]["operator_resolution"],
        "Operator requires explicit Plan-review evidence."
    );
    assert_eq!(
        stored.input["memory"]["items"][0]["content"],
        "Review only the supplied Plan."
    );
    assert_eq!(stored.capability, "controller.plan_review");
}

#[test]
fn malformed_input_and_observed_or_accepted_results_write_zero_rows() {
    let (_directory, app) = fixture();
    let valid = result(ControllerPlanReviewDecision::Approve, "valid", None);

    let mut malformed_input = input();
    malformed_input.current_request.plan_id = 0;
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
fn review_result_validation_rejects_feedback_contract_errors() {
    let (_directory, app) = fixture();
    let valid = result(ControllerPlanReviewDecision::Approve, "valid", None);

    let revise_without_feedback = result(ControllerPlanReviewDecision::RevisePlan, "missing", None);
    assert_zero_rows(
        &app,
        &request(
            input(),
            revise_without_feedback,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let empty_feedback = result(ControllerPlanReviewDecision::RevisePlan, "empty", Some(""));
    assert_zero_rows(
        &app,
        &request(
            input(),
            empty_feedback,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let oversized_feedback = result(
        ControllerPlanReviewDecision::RevisePlan,
        "oversized",
        Some(&"x".repeat(2049)),
    );
    assert_zero_rows(
        &app,
        &request(
            input(),
            oversized_feedback,
            valid,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let malformed_accepted = result(ControllerPlanReviewDecision::RevisePlan, "missing", None);
    assert_zero_rows(
        &app,
        &request(
            input(),
            ControllerPlanReviewResult {
                decision: ControllerPlanReviewDecision::Approve,
                details: "observed".into(),
                revision_feedback: None,
            },
            malformed_accepted,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn correction_semantics_reject_mismatches_and_equal_corrections() {
    let (_directory, app) = fixture();
    let output = result(ControllerPlanReviewDecision::Approve, "same", None);

    let mut equal_with_correction = request(
        input(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::to_value(&output).unwrap(),
        operator_reference: "operator:plan-review".into(),
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

    let accepted = result(
        ControllerPlanReviewDecision::OperatorDecisionRequired,
        "different",
        None,
    );
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
        operator_reference: "operator:plan-review".into(),
        reason: "mismatched evidence".into(),
    });
    assert_zero_rows(&app, &wrong_original);
}

#[test]
fn invalid_m08_metadata_and_bounds_write_zero_rows() {
    let (_directory, app) = fixture();
    let output = result(ControllerPlanReviewDecision::Approve, "valid", None);

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

    let mut large = input();
    large.memory = memory_context(
        (1..=8)
            .map(|id| memory_item(id, "large-review-memory", &"m".repeat(2500)))
            .collect(),
    );
    assert!(large.validate().is_ok());
    assert_zero_rows(
        &app,
        &request(
            large,
            result(ControllerPlanReviewDecision::Approve, "valid", None),
            result(ControllerPlanReviewDecision::Approve, "valid", None),
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn genuine_production_input_bound_rejection_writes_zero_rows() {
    let (_directory, app) = fixture();
    let mut large = input();
    large.current_request.current_state.ready_tasks = (0..6)
        .map(|index| ControllerPlanReviewTask {
            id: format!("{}-{index}", "i".repeat(2040)),
            title: "t".repeat(2040),
            status: "s".repeat(2040),
        })
        .collect();
    large.memory = memory_context(
        (1..=8)
            .map(|id| memory_item(id, "large-review-memory", &"m".repeat(3500)))
            .collect(),
    );
    assert!(large.current_request.validate().is_ok());
    assert!(large.memory.validate().is_ok());
    assert!(large.validate().is_err());
    let error = large.validate().unwrap_err();
    assert!(matches!(
        error,
        ControllerPlanReviewError::RequestTooLarge { .. }
    ));
    assert_zero_rows(
        &app,
        &request(
            large,
            result(ControllerPlanReviewDecision::Approve, "valid", None),
            result(ControllerPlanReviewDecision::Approve, "valid", None),
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn distinct_m08_payload_bound_rejection_keeps_plan_review_contracts_valid() {
    let (_directory, app) = fixture();
    let mut large = input();
    large.memory = memory_context(
        (1..=8)
            .map(|id| memory_item(id, "large-review-memory", &"m".repeat(2500)))
            .collect(),
    );
    let observed = result(ControllerPlanReviewDecision::Approve, "valid", None);
    let accepted = result(ControllerPlanReviewDecision::Approve, "valid", None);
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
    let output = result(ControllerPlanReviewDecision::Approve, "one row", None);
    let request = request(
        input(),
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    app.create_controller_plan_review_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(
        app.create_controller_plan_review_experience_example(&request)
            .unwrap()
            .id,
        2
    );
    assert_eq!(all(&app).len(), 2);
}
