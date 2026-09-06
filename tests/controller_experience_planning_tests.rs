use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis, MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES,
};
use orc::controller_experience_planning::{
    CONTROLLER_PLAN_GENERATION_EXPERIENCE_CAPABILITY, ControllerExperiencePlanningRequest,
};
use orc::controller_memory::ControllerMemoryContext;
use orc::controller_planning::{
    CONTROLLER_PLANNING_REQUEST_VERSION, ControllerPlanResult, ControllerPlanningInput,
    ControllerPlanningRequest, ControllerPlanningState, MAX_CONTROLLER_PLANNING_REQUEST_BYTES,
};
use orc::protocol::{PROTOCOL_VERSION, PlanResponse, PlanResponseSchema};
use orc::storage::Database;
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    database.create_project("planning curation").unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app)
}

fn input() -> ControllerPlanningInput {
    ControllerPlanningInput {
        current_request: ControllerPlanningRequest {
            packet_version: CONTROLLER_PLANNING_REQUEST_VERSION,
            kind: "project_plan".into(),
            project_name: Some("planning curation".into()),
            engineering_contract: "Keep planning read-only.".into(),
            objective: "Plan one bounded change.".into(),
            constraints: vec!["Do not mutate state.".into()],
            target_platforms: vec![],
            stack: vec!["Rust".into()],
            non_goals: vec!["Applying the plan".into()],
            deliverables: vec!["A PlanResponse".into()],
            definition_of_done: vec!["The plan is reviewable.".into()],
            response_schema: PlanResponseSchema::v1(),
            role_boundaries: vec!["Controller proposes only.".into()],
            planning_constraints: vec![],
            approval_requirements: vec!["Operator approval is required.".into()],
            current_state: None,
        },
        memory: ControllerMemoryContext::empty(),
    }
}

fn result(objective: &str, rationale: &str) -> ControllerPlanResult {
    ControllerPlanResult {
        plan: PlanResponse {
            protocol_version: PROTOCOL_VERSION,
            objective: objective.into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks: vec![],
        },
        rationale: rationale.into(),
        uncertainty: None,
    }
}

fn request(
    input: ControllerPlanningInput,
    observed: ControllerPlanResult,
    accepted: ControllerPlanResult,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperiencePlanningRequest {
    ControllerExperiencePlanningRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit planning curation evidence".into(),
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

#[test]
fn equal_planning_results_persist_one_exact_non_correction_row() {
    let (_directory, app) = fixture();
    let input = input();
    let expected_input = serde_json::to_value(&input).unwrap();
    let planning_result = result("Plan one bounded change.", "The proposal is bounded.");
    let request = request(
        input,
        planning_result.clone(),
        planning_result,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_planning_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.accepted_output,
        serde_json::to_value(&request.accepted).unwrap()
    );
    assert_eq!(
        stored.capability,
        CONTROLLER_PLAN_GENERATION_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert!(stored.correction.is_none());
}

#[test]
fn corrected_planning_result_preserves_exact_complete_outputs() {
    let (_directory, app) = fixture();
    let observed = result("Observed plan.", "Observed rationale.");
    let accepted = result("Accepted plan.", "Accepted rationale.");
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
        operator_reference: "operator:plan-1".into(),
        reason: "operator supplied the accepted planning result".into(),
    });

    let stored = app
        .create_controller_planning_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn malformed_input_and_observed_or_accepted_results_write_zero_rows() {
    let (_directory, app) = fixture();
    let valid = result("Plan one bounded change.", "valid");

    let mut malformed_input = input();
    malformed_input.current_request.packet_version = 0;
    assert!(
        app.create_controller_planning_experience_example(&request(
            malformed_input,
            valid.clone(),
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let mut invalid_observed_plan = valid.clone();
    invalid_observed_plan.plan.protocol_version = 99;
    assert!(
        app.create_controller_planning_experience_example(&request(
            input(),
            invalid_observed_plan,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let invalid_observed_rationale = result("Plan one bounded change.", "");
    assert!(
        app.create_controller_planning_experience_example(&request(
            input(),
            invalid_observed_rationale,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let mut invalid_observed_uncertainty = valid.clone();
    invalid_observed_uncertainty.uncertainty = Some("u".repeat(1025));
    assert!(
        app.create_controller_planning_experience_example(&request(
            input(),
            invalid_observed_uncertainty,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let mut invalid_accepted_plan = valid.clone();
    invalid_accepted_plan.plan.protocol_version = 99;
    assert!(
        app.create_controller_planning_experience_example(&request(
            input(),
            valid.clone(),
            invalid_accepted_plan,
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let invalid_accepted_rationale = result("Plan one bounded change.", "");
    assert!(
        app.create_controller_planning_experience_example(&request(
            input(),
            valid,
            invalid_accepted_rationale,
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );
    assert!(all(&app).is_empty());
}

#[test]
fn correction_invariants_and_m08_metadata_reject_without_persistence() {
    let (_directory, app) = fixture();
    let observed = result("Observed plan.", "Observed rationale.");
    let accepted = result("Accepted plan.", "Accepted rationale.");

    let mut equal_with_correction = request(
        input(),
        observed.clone(),
        observed.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"wrong": true}),
        operator_reference: "operator:1".into(),
        reason: "not allowed for equal outputs".into(),
    });
    assert!(
        app.create_controller_planning_experience_example(&equal_with_correction)
            .is_err()
    );

    let differing_without_outcome = request(
        input(),
        observed.clone(),
        accepted.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    assert!(
        app.create_controller_planning_experience_example(&differing_without_outcome)
            .is_err()
    );

    let differing_without_metadata = request(
        input(),
        observed.clone(),
        accepted.clone(),
        ControllerExperienceOutcome::Corrected,
    );
    assert!(
        app.create_controller_planning_experience_example(&differing_without_metadata)
            .is_err()
    );

    let mut wrong_original = request(
        input(),
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    wrong_original.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"not": "the observed result"}),
        operator_reference: "operator:2".into(),
        reason: "observed output must be preserved exactly".into(),
    });
    assert!(
        app.create_controller_planning_experience_example(&wrong_original)
            .is_err()
    );

    let valid = result("Plan one bounded change.", "valid");
    let mut invalid_quality = request(
        input(),
        valid.clone(),
        valid.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_quality.quality.score = 101;
    assert!(
        app.create_controller_planning_experience_example(&invalid_quality)
            .is_err()
    );

    let mut invalid_provenance = request(
        input(),
        valid.clone(),
        valid,
        ControllerExperienceOutcome::Accepted,
    );
    invalid_provenance.provenance.project_id = Some(0);
    assert!(
        app.create_controller_planning_experience_example(&invalid_provenance)
            .is_err()
    );
    assert!(all(&app).is_empty());
}

#[test]
fn valid_planning_values_rejected_by_m08_payload_bound_write_zero_rows() {
    let (_directory, app) = fixture();
    let mut input = input();
    input.current_request.current_state = Some(ControllerPlanningState {
        task_counts: vec![],
        ready_tasks: (0..16)
            .map(|index| orc::controller_planning::ControllerPlanningTask {
                id: format!("task-{index}"),
                title: "t".repeat(1_200),
                status: "ready".into(),
            })
            .collect(),
        active_tasks: vec![],
        review_tasks: vec![],
        blocked_tasks: vec![],
        usable_agents: vec![],
        busy_agents: vec![],
        quota_reserve_percent: 10,
    });
    assert!(input.validate().is_ok());
    let input_size = serde_json::to_vec(&input).unwrap().len();
    assert!(input_size > MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES);
    assert!(input_size < 64 * 1024);

    let valid = result("Plan one bounded change.", "valid");
    assert!(
        app.create_controller_planning_experience_example(&request(
            input,
            valid.clone(),
            valid,
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );
    assert!(all(&app).is_empty());
}

#[test]
fn oversized_planning_input_rejected_by_planning_bound_writes_zero_rows() {
    let (_directory, app) = fixture();
    let mut input = input();
    input.current_request.current_state = Some(ControllerPlanningState {
        task_counts: vec![],
        ready_tasks: (0..32)
            .map(|index| orc::controller_planning::ControllerPlanningTask {
                id: format!("task-{index}"),
                title: "t".repeat(2_048),
                status: "ready".into(),
            })
            .collect(),
        active_tasks: vec![],
        review_tasks: vec![],
        blocked_tasks: vec![],
        usable_agents: vec![],
        busy_agents: vec![],
        quota_reserve_percent: 10,
    });
    let serialized_size = serde_json::to_vec(&input).unwrap().len();
    assert!(serialized_size > MAX_CONTROLLER_PLANNING_REQUEST_BYTES);
    assert!(input.validate().is_err());

    let valid = result("Plan one bounded change.", "valid");
    assert!(
        app.create_controller_planning_experience_example(&request(
            input,
            valid.clone(),
            valid,
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );
    assert!(all(&app).is_empty());
}
