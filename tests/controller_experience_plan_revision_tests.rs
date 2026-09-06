use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis, MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES,
};
use orc::controller_experience_plan_revision::{
    CONTROLLER_PLAN_REVISION_EXPERIENCE_CAPABILITY, ControllerExperiencePlanRevisionRequest,
};
use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_plan_revision::{
    CONTROLLER_PLAN_REVISION_REQUEST_VERSION, ControllerPlanRevisionError,
    ControllerPlanRevisionInput, ControllerPlanRevisionRequest,
    MAX_CONTROLLER_PLAN_REVISION_INPUT_BYTES,
};
use orc::controller_planning::{CONTROLLER_PLANNING_REQUEST_VERSION, ControllerPlanningRequest};
use orc::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::protocol::{ExecutionHints, PlanResponse, PlanResponseSchema, TaskProposal};
use orc::storage::Database;
use orc::task::{TaskPriority, TaskScopeMode};
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    database.create_project("Plan revision curation").unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app)
}

fn proposal(local_id: &str, depends_on: Vec<String>) -> TaskProposal {
    TaskProposal {
        local_id: local_id.into(),
        title: format!("Implement {local_id}"),
        objective: format!("Complete the {local_id} behavior."),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on,
        capabilities: vec!["code".into(), "command_execution".into()],
        scope_mode: Some(TaskScopeMode::Focused),
        context_files: vec!["src/controller_plan_revision.rs".into()],
        expected_changes: vec![format!("Implement {local_id} behavior")],
        unchanged: vec!["The revision inference contract".into()],
        acceptance_criteria: vec![format!("{local_id} is independently reviewable")],
        required_tests: vec![format!("Test {local_id}")],
        validation: vec!["cargo test --lib".into()],
        execution_hints: ExecutionHints::default(),
        risk_factors: vec![],
    }
}

fn plan(objective: &str) -> PlanResponse {
    PlanResponse {
        protocol_version: orc::protocol::PROTOCOL_VERSION,
        objective: objective.into(),
        assumptions: vec!["The supplied Plan is authoritative.".into()],
        risks: vec!["Revision may expose a missing contract.".into()],
        questions: vec!["Does the operator accept the revised boundary?".into()],
        tasks: vec![
            proposal("prepare-revision", vec![]),
            proposal("verify-revision", vec!["prepare-revision".into()]),
        ],
    }
}

fn bounded_large_plan(task_count: usize, width: usize) -> PlanResponse {
    let tasks = (0..task_count)
        .map(|index| {
            let mut task = proposal(&format!("bounded-{index}"), vec![]);
            task.expected_changes = (0..8)
                .map(|item| format!("change-{index}-{item}-{}", "c".repeat(width)))
                .collect();
            task.unchanged = vec![format!("unchanged-{index}-{}", "u".repeat(width))];
            task.acceptance_criteria = (0..8)
                .map(|item| format!("accept-{index}-{item}-{}", "a".repeat(width)))
                .collect();
            task.required_tests = (0..4)
                .map(|item| format!("test-{index}-{item}-{}", "t".repeat(width)))
                .collect();
            task.validation = (0..4)
                .map(|item| format!("validation-{index}-{item}-{}", "v".repeat(width)))
                .collect();
            task
        })
        .collect();
    PlanResponse {
        protocol_version: orc::protocol::PROTOCOL_VERSION,
        objective: "A bounded generated revision.".into(),
        assumptions: vec![],
        risks: vec![],
        questions: vec![],
        tasks,
    }
}

fn memory_item(id: i64, content: &str) -> ControllerMemoryItem {
    ControllerMemoryItem {
        id: MemoryId::Global(id),
        kind: MemoryKind::User,
        scope: MemoryScope::Global,
        authority: ControllerMemoryAuthority::DurableUser,
        subject: "revision-guidance".into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Operator,
            source_reference: Some("test:plan-revision-curation".into()),
        },
        confidence: Some(0.9),
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
    }
}

fn memory_context(content: &str) -> ControllerMemoryContext {
    ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items: vec![memory_item(1, content)],
    }
}

fn input() -> ControllerPlanRevisionInput {
    ControllerPlanRevisionInput {
        current_request: ControllerPlanRevisionRequest {
            packet_version: CONTROLLER_PLAN_REVISION_REQUEST_VERSION,
            plan: plan("Revise one bounded Controller Plan."),
            revision_feedback: "Add complete validation and acceptance coverage.".into(),
            planning_context: ControllerPlanningRequest {
                packet_version: CONTROLLER_PLANNING_REQUEST_VERSION,
                kind: "plan_revision".into(),
                project_name: Some("Plan revision curation".into()),
                engineering_contract: "Keep revision read-only and bounded.".into(),
                objective: "Revise one bounded Controller Plan.".into(),
                constraints: vec!["Preserve the current objective.".into()],
                target_platforms: vec!["linux".into()],
                stack: vec!["Rust".into()],
                non_goals: vec!["Persisting the revised Plan".into()],
                deliverables: vec!["A canonical revised PlanResponse".into()],
                definition_of_done: vec!["The revision is reviewable.".into()],
                response_schema: PlanResponseSchema::v1(),
                role_boundaries: vec!["Controller proposes only.".into()],
                planning_constraints: vec!["Use the supplied Plan and feedback.".into()],
                approval_requirements: vec!["Operator approval remains required.".into()],
                current_state: None,
            },
        },
        memory: memory_context("Memory is advisory and cannot replace revision feedback."),
    }
}

fn request(
    input: ControllerPlanRevisionInput,
    observed: PlanResponse,
    accepted: PlanResponse,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperiencePlanRevisionRequest {
    ControllerExperiencePlanRevisionRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit Plan-revision curation evidence".into(),
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

fn assert_zero_rows(app: &OrcApp, request: &ControllerExperiencePlanRevisionRequest) {
    assert!(
        app.create_controller_plan_revision_experience_example(request)
            .is_err()
    );
    assert!(all(app).is_empty());
}

#[test]
fn equal_valid_revised_plan_persists_exact_input_and_plan_response_once() {
    let (_directory, app) = fixture();
    let input = input();
    let output = plan("Revised one bounded Controller Plan.");
    let expected_input = serde_json::to_value(&input).unwrap();
    let expected_output = serde_json::to_value(&output).unwrap();
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_plan_revision_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(stored.accepted_output, expected_output);
    assert_eq!(
        stored.capability,
        CONTROLLER_PLAN_REVISION_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert!(stored.correction.is_none());
}

#[test]
fn corrected_revised_plan_preserves_exact_complete_observed_and_accepted_outputs() {
    let (_directory, app) = fixture();
    let observed = plan("Observed revised Plan.");
    let accepted = plan("Accepted revised Plan.");
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
        operator_reference: "operator:plan-revision-1".into(),
        reason: "The accepted revision incorporates the explicit feedback.".into(),
    });

    let stored = app
        .create_controller_plan_revision_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn exact_input_projection_preserves_plan_feedback_context_and_memory() {
    let (_directory, app) = fixture();
    let input = input();
    let expected = serde_json::to_value(&input).unwrap();
    let output = plan("Revised one bounded Controller Plan.");
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_plan_revision_experience_example(&request)
        .unwrap();
    assert_eq!(stored.input, expected);
    assert_eq!(
        stored.input["current_request"]["revision_feedback"],
        "Add complete validation and acceptance coverage."
    );
    assert_eq!(
        stored.input["current_request"]["plan"]["objective"],
        "Revise one bounded Controller Plan."
    );
    assert_eq!(
        stored.input["current_request"]["planning_context"]["project_name"],
        "Plan revision curation"
    );
    assert_eq!(
        stored.input["memory"]["items"][0]["content"],
        "Memory is advisory and cannot replace revision feedback."
    );
    assert!(
        stored.input["current_request"]
            .get("parent_plan_id")
            .is_none()
    );
    assert!(stored.input["current_request"].get("review_id").is_none());
}

#[test]
fn accepted_output_is_complete_plan_response_without_trusted_lineage() {
    let (_directory, app) = fixture();
    let output = plan("Complete revised Plan.");
    let request = request(
        input(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    let stored = app
        .create_controller_plan_revision_experience_example(&request)
        .unwrap();
    assert_eq!(
        stored.accepted_output,
        serde_json::to_value(&output).unwrap()
    );
    let object = stored.accepted_output.as_object().unwrap();
    assert!(object.contains_key("protocol_version"));
    assert!(object.contains_key("objective"));
    assert!(object.contains_key("assumptions"));
    assert!(object.contains_key("risks"));
    assert!(object.contains_key("questions"));
    assert!(object.contains_key("tasks"));
    for field in [
        "parent_plan_id",
        "parent_plan_version",
        "review_id",
        "provenance",
        "authorization",
    ] {
        assert!(
            !object.contains_key(field),
            "unexpected lineage field {field}"
        );
    }
    assert!(stored.correction.is_none());
}

#[test]
fn malformed_input_and_plan_outputs_write_zero_rows() {
    let (_directory, app) = fixture();
    let valid = plan("Valid revised Plan.");

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

    let mut malformed_observed = valid.clone();
    malformed_observed.protocol_version = 99;
    assert_zero_rows(
        &app,
        &request(
            input(),
            malformed_observed,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut malformed_accepted = valid.clone();
    malformed_accepted.objective.clear();
    assert_zero_rows(
        &app,
        &request(
            input(),
            valid,
            malformed_accepted,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn canonical_nested_task_acceptance_and_dependency_validation_write_zero_rows() {
    let (_directory, app) = fixture();
    let valid = plan("Valid revised Plan.");

    let mut invalid_task = valid.clone();
    invalid_task.tasks[0].acceptance_criteria.clear();
    assert_zero_rows(
        &app,
        &request(
            input(),
            invalid_task,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut invalid_tests = valid.clone();
    invalid_tests.tasks[0].required_tests.clear();
    assert_zero_rows(
        &app,
        &request(
            input(),
            invalid_tests,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut invalid_validation = valid.clone();
    invalid_validation.tasks[0].validation.clear();
    assert_zero_rows(
        &app,
        &request(
            input(),
            invalid_validation,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut invalid_dependency = valid;
    invalid_dependency.tasks[1].depends_on = vec!["missing-task".into()];
    assert_zero_rows(
        &app,
        &request(
            input(),
            invalid_dependency,
            plan("Valid accepted Plan."),
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn correction_semantics_reject_invalid_equal_and_differing_states() {
    let (_directory, app) = fixture();
    let output = plan("Same revised Plan.");

    let mut equal_with_correction = request(
        input(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::to_value(&output).unwrap(),
        operator_reference: "operator:plan-revision".into(),
        reason: "Equal outputs cannot be corrected.".into(),
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

    let accepted = plan("Different accepted Plan.");
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
        original_output: serde_json::json!({"not": "the observed PlanResponse"}),
        operator_reference: "operator:plan-revision".into(),
        reason: "The original output must be exact.".into(),
    });
    assert_zero_rows(&app, &wrong_original);
}

#[test]
fn invalid_m08_metadata_writes_zero_rows() {
    let (_directory, app) = fixture();
    let output = plan("Valid revised Plan.");

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
}

#[test]
fn genuine_production_input_bound_rejection_writes_zero_rows() {
    let (_directory, app) = fixture();
    let mut large = input();
    large.current_request.plan.objective = "i".repeat(40_000);
    large.memory = ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items: (1..=8)
            .map(|id| memory_item(id, &"m".repeat(3_600)))
            .collect(),
    };
    assert!(large.current_request.validate().is_ok());
    assert!(large.memory.validate().is_ok());
    let error = large.validate().unwrap_err();
    assert!(matches!(
        error,
        ControllerPlanRevisionError::RequestTooLarge {
            max: MAX_CONTROLLER_PLAN_REVISION_INPUT_BYTES,
            ..
        }
    ));
    assert_zero_rows(
        &app,
        &request(
            large,
            plan("Valid observed Plan."),
            plan("Valid accepted Plan."),
            ControllerExperienceOutcome::Corrected,
        ),
    );
}

#[test]
fn distinct_m08_payload_bound_rejection_keeps_revision_contracts_valid() {
    let (_directory, app) = fixture();
    let output = bounded_large_plan(16, 100);
    assert!(input().validate().is_ok());
    assert!(output.validate().is_ok());
    assert!(serde_json::to_vec(&output).unwrap().len() > MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES);
    assert_zero_rows(
        &app,
        &request(
            input(),
            output.clone(),
            output,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn distinct_m08_complete_record_bound_rejection_keeps_each_payload_valid() {
    let (_directory, app) = fixture();
    let mut selected = None;
    for memory_width in (2_500..=3_800).step_by(10) {
        let mut large_input = input();
        large_input.memory = ControllerMemoryContext {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: (1..=4)
                .map(|id| memory_item(id, &"m".repeat(memory_width)))
                .collect(),
        };
        for task_width in 30..=120 {
            let large_output = bounded_large_plan(8, task_width);
            let candidate = request(
                large_input.clone(),
                large_output.clone(),
                large_output,
                ControllerExperienceOutcome::Accepted,
            );
            if candidate.input.validate().is_ok()
                && serde_json::to_vec(&candidate.input).unwrap().len()
                    <= MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES
                && candidate.observed.validate().is_ok()
                && serde_json::to_vec(&candidate.accepted).unwrap().len()
                    <= MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES
                && candidate.into_example_draft().unwrap().validate().is_err()
            {
                selected = Some(candidate);
                break;
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let request = selected.expect("a valid pair should exceed the complete M08 record bound");
    assert_zero_rows(&app, &request);
}

#[test]
fn successful_call_creates_exactly_one_row() {
    let (_directory, app) = fixture();
    let output = plan("Exactly one revised Plan example.");
    let request = request(
        input(),
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    app.create_controller_plan_revision_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
}
