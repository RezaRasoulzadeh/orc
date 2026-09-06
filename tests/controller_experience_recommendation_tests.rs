use orc::app::OrcApp;
use orc::controller::{
    ControllerRecommendation, ControllerRecommendationInput, ControllerStateBuilder,
};
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis,
};
use orc::controller_experience_recommendation::{
    CONTROLLER_RECOMMENDATION_EXPERIENCE_CAPABILITY, ControllerExperienceRecommendationRequest,
};
use orc::controller_memory::ControllerMemoryContext;
use orc::operations::OperationalNextStep;
use orc::storage::Database;
use orc::task::{CreateTaskInput, TaskPriority, TaskScopeMode};
use serde_json::Value;
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp, String) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = database.create_project("recommendation curation").unwrap();
    let task_id = database
        .create_task(
            project_id,
            &CreateTaskInput {
                title: "Curate one recommendation".into(),
                objective: "Preserve the exact normal recommendation contract".into(),
                role: "developer".into(),
                priority: TaskPriority::Normal,
                required_capabilities: vec![],
                scope_mode: Some(TaskScopeMode::Focused),
                context_files: vec![],
                expected_changes: vec!["src/controller.rs".into()],
                dependencies: vec![],
            },
        )
        .unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app, task_id)
}

fn input(app: &OrcApp, task_id: &str) -> ControllerRecommendationInput {
    let packet = ControllerStateBuilder::new()
        .build(&app.operations(), task_id)
        .unwrap();
    ControllerRecommendationInput::from_packet(&packet, ControllerMemoryContext::empty())
}

fn structured(step: Option<&str>, rationale: &str) -> Value {
    serde_json::json!({
        "suggested_next_step": step,
        "decision_class": if step.is_some() { "action" } else { "operator_decision" },
        "rationale": rationale,
    })
}

fn recommendation(task_id: &str, output: Value, response_text: &str) -> ControllerRecommendation {
    let suggested_next_step = output["suggested_next_step"].as_str().map(|value| {
        serde_json::from_value::<OperationalNextStep>(Value::String(value.into())).unwrap()
    });
    ControllerRecommendation {
        task_id: task_id.into(),
        response_text: response_text.into(),
        suggested_next_step,
        rationale: output["rationale"].as_str().unwrap().into(),
        structured_output: Some(output),
    }
}

fn request(
    input: ControllerRecommendationInput,
    observed: ControllerRecommendation,
    accepted: ControllerRecommendation,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperienceRecommendationRequest {
    ControllerExperienceRecommendationRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit operator curation evidence".into(),
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
fn equal_observed_and_accepted_persist_one_active_accepted_example() {
    let (_directory, app, task_id) = fixture();
    let input = input(&app, &task_id);
    let output = structured(Some("dispatch"), "The task is ready.");
    let observed = recommendation(&task_id, output.clone(), "arbitrary prose is not persisted");
    let accepted = recommendation(&task_id, output.clone(), "different prose");
    let stored = app
        .create_controller_recommendation_experience_example(&request(
            input,
            observed,
            accepted,
            ControllerExperienceOutcome::Accepted,
        ))
        .unwrap();

    assert_eq!(all(&app).len(), 1);
    assert_eq!(
        stored.capability,
        CONTROLLER_RECOMMENDATION_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert_eq!(stored.accepted_output, output);
    assert!(stored.correction.is_none());
}

#[test]
fn corrected_recommendation_preserves_observed_output_and_persists_accepted_output() {
    let (_directory, app, task_id) = fixture();
    let input = input(&app, &task_id);
    let observed_output = structured(None, "The task needs an operator decision.");
    let accepted_output = structured(Some("dispatch"), "The task is ready.");
    let observed = recommendation(&task_id, observed_output.clone(), "observed prose");
    let accepted = recommendation(&task_id, accepted_output.clone(), "accepted prose");
    let mut request = request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:1".into(),
        reason: "operator supplied the accepted recommendation".into(),
    });

    let stored = app
        .create_controller_recommendation_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
}

#[test]
fn exact_input_is_preserved_and_capability_is_not_caller_overridable() {
    let (_directory, app, task_id) = fixture();
    let input = input(&app, &task_id);
    let expected_input = serde_json::to_value(&input).unwrap();
    let output = structured(Some("dispatch"), "The task is ready.");
    let stored = app
        .create_controller_recommendation_experience_example(&request(
            input,
            recommendation(&task_id, output.clone(), "prose"),
            recommendation(&task_id, output, "other prose"),
            ControllerExperienceOutcome::Accepted,
        ))
        .unwrap();

    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.capability,
        CONTROLLER_RECOMMENDATION_EXPERIENCE_CAPABILITY
    );
}

#[test]
fn invalid_or_mismatched_recommendations_write_zero_rows() {
    let (_directory, app, task_id) = fixture();
    let valid_input = input(&app, &task_id);
    let output = structured(Some("dispatch"), "The task is ready.");

    let mut invalid_input = valid_input.clone();
    invalid_input.current_packet.task.summary.task_id.clear();
    assert!(
        app.create_controller_recommendation_experience_example(&request(
            invalid_input,
            recommendation(&task_id, output.clone(), "observed"),
            recommendation(&task_id, output.clone(), "accepted"),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let mut malformed_observed = recommendation(&task_id, output.clone(), "observed");
    malformed_observed.structured_output = Some(serde_json::json!({
        "suggested_next_step": "dispatch",
        "decision_class": "action",
        "rationale": "valid",
        "unknown": true
    }));
    assert!(
        app.create_controller_recommendation_experience_example(&request(
            valid_input.clone(),
            malformed_observed,
            recommendation(&task_id, output.clone(), "accepted"),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let mut inconsistent_observed = recommendation(&task_id, output.clone(), "observed");
    inconsistent_observed.suggested_next_step = None;
    assert!(
        app.create_controller_recommendation_experience_example(&request(
            valid_input.clone(),
            inconsistent_observed,
            recommendation(&task_id, output.clone(), "accepted"),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let malformed_accepted = ControllerRecommendation {
        task_id: task_id.clone(),
        response_text: "accepted".into(),
        suggested_next_step: Some(OperationalNextStep::Dispatch),
        rationale: "accepted".into(),
        structured_output: Some(serde_json::json!({
            "suggested_next_step": "dispatch"
        })),
    };
    assert!(
        app.create_controller_recommendation_experience_example(&request(
            valid_input.clone(),
            recommendation(&task_id, output.clone(), "observed"),
            malformed_accepted,
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    assert!(
        app.create_controller_recommendation_experience_example(&request(
            valid_input.clone(),
            recommendation(&task_id, output.clone(), "observed"),
            recommendation("other-task", output.clone(), "accepted"),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );
    assert!(all(&app).is_empty());
}

#[test]
fn correction_and_m08_metadata_mismatches_write_zero_rows() {
    let (_directory, app, task_id) = fixture();
    let input = input(&app, &task_id);
    let observed_output = structured(None, "Needs a decision.");
    let accepted_output = structured(Some("dispatch"), "Ready.");

    let mut equal_with_correction = request(
        input.clone(),
        recommendation(&task_id, observed_output.clone(), "observed"),
        recommendation(&task_id, observed_output.clone(), "accepted"),
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:1".into(),
        reason: "not allowed when equal".into(),
    });
    assert!(
        app.create_controller_recommendation_experience_example(&equal_with_correction)
            .is_err()
    );

    let differing_without_correction = request(
        input.clone(),
        recommendation(&task_id, observed_output.clone(), "observed"),
        recommendation(&task_id, accepted_output.clone(), "accepted"),
        ControllerExperienceOutcome::Corrected,
    );
    assert!(
        app.create_controller_recommendation_experience_example(&differing_without_correction)
            .is_err()
    );

    let mut wrong_original = request(
        input.clone(),
        recommendation(&task_id, observed_output.clone(), "observed"),
        recommendation(&task_id, accepted_output, "accepted"),
        ControllerExperienceOutcome::Corrected,
    );
    wrong_original.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"not": "observed"}),
        operator_reference: "operator:1".into(),
        reason: "wrong original".into(),
    });
    assert!(
        app.create_controller_recommendation_experience_example(&wrong_original)
            .is_err()
    );

    let mut invalid_quality = request(
        input,
        recommendation(&task_id, observed_output, "observed"),
        recommendation(&task_id, structured(None, "accepted"), "accepted"),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_quality.quality.score = 101;
    assert!(
        app.create_controller_recommendation_experience_example(&invalid_quality)
            .is_err()
    );
    assert!(all(&app).is_empty());
}

#[test]
fn m08_payload_bound_failure_writes_zero_rows() {
    let (_directory, app, task_id) = fixture();
    let mut input = input(&app, &task_id);
    input.current_packet.task.contract.unchanged = vec!["u".repeat(1024); 14];
    input.current_packet.task.contract.acceptance_criteria = vec!["a".repeat(1024); 14];
    input.current_packet.task.contract.required_tests = vec!["t".repeat(1024); 14];
    input.current_packet.task.contract.validation = vec!["v".repeat(1024); 14];
    let output = structured(Some("dispatch"), "The task is ready.");
    assert!(input.validate().is_ok());
    assert!(
        app.create_controller_recommendation_experience_example(&request(
            input,
            recommendation(&task_id, output.clone(), "observed"),
            recommendation(&task_id, output, "accepted"),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );
    assert!(all(&app).is_empty());
}
