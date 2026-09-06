use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis, MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES,
};
use orc::controller_experience_recovery::{
    CONTROLLER_RECOVERY_RECOMMENDATION_EXPERIENCE_CAPABILITY,
    ControllerExperienceRecoveryRecommendationRequest,
};
use orc::recovery_controller::{
    RecoveryInferenceInput, RecoveryRecommendation, RecoveryRecommendationDecision,
};
use orc::storage::Database;
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    database.create_project("recovery curation").unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app)
}

fn input(task_id: &str) -> RecoveryInferenceInput {
    serde_json::from_value(serde_json::json!({
        "current_request": {
            "observation": {
                "task_id": task_id,
                "state": "abnormal",
                "lifecycle": "active",
                "queue_phase": "ready",
                "conditions": ["execution_failure"],
                "execution_condition": null,
                "validation": {
                    "state": "none",
                    "failure_classification": null,
                    "is_current": null
                },
                "review": {
                    "run_id": null,
                    "verdict": null,
                    "applies_to_current_change": null
                },
                "revision": {
                    "actionable_review_run_id": null,
                    "actionable_contract_id": null,
                    "contract_source_review_run_id": null
                },
                "latest_execution": null,
                "dependencies": [],
                "blockers": [],
                "agent_economy": {
                    "candidate_count": 0,
                    "eligible_count": 0,
                    "constraints": []
                }
            },
            "legal_operations": [
                {"status": "allowed", "operation": "requeue"}
            ]
        },
        "memory": {"context_version": 1, "items": []}
    }))
    .unwrap()
}

fn recommendation(
    decision: RecoveryRecommendationDecision,
    rationale: &str,
) -> RecoveryRecommendation {
    RecoveryRecommendation {
        decision,
        rationale: rationale.into(),
        confidence: Some(0.8),
    }
}

fn request(
    input: RecoveryInferenceInput,
    observed: RecoveryRecommendation,
    accepted: RecoveryRecommendation,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperienceRecoveryRecommendationRequest {
    ControllerExperienceRecoveryRecommendationRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit recovery curation evidence".into(),
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
fn equal_recovery_recommendations_persist_one_exact_non_correction_row() {
    let (_directory, app) = fixture();
    let input = input("recovery-task");
    let expected_input = serde_json::to_value(&input).unwrap();
    let recommendation = recommendation(
        RecoveryRecommendationDecision::Requeue,
        "The failed task may be requeued.",
    );
    let mut request = request(
        input,
        recommendation.clone(),
        recommendation,
        ControllerExperienceOutcome::Accepted,
    );
    request.provenance.task_id = Some("recovery-task".into());

    let stored = app
        .create_controller_recovery_experience_example(&request)
        .unwrap();

    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.accepted_output,
        serde_json::to_value(&request.accepted).unwrap()
    );
    assert_eq!(
        stored.capability,
        CONTROLLER_RECOVERY_RECOMMENDATION_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert!(stored.correction.is_none());
}

#[test]
fn corrected_recommendation_preserves_exact_observed_and_accepted_outputs() {
    let (_directory, app) = fixture();
    let observed = recommendation(
        RecoveryRecommendationDecision::OperatorDecision,
        "An operator must decide.",
    );
    let accepted = recommendation(
        RecoveryRecommendationDecision::Requeue,
        "The failed task may be requeued.",
    );
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = request(
        input("recovery-task"),
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:recovery-1".into(),
        reason: "operator supplied the accepted recovery recommendation".into(),
    });

    let stored = app
        .create_controller_recovery_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn fixed_identity_and_correction_invariants_reject_without_writing() {
    let (_directory, app) = fixture();
    let observed = recommendation(RecoveryRecommendationDecision::Requeue, "observed");
    let accepted = recommendation(RecoveryRecommendationDecision::OperatorDecision, "accepted");

    let mut mismatch = request(
        input("recovery-task"),
        observed.clone(),
        accepted.clone(),
        ControllerExperienceOutcome::Corrected,
    );
    mismatch.provenance.task_id = Some("other-task".into());
    assert!(
        app.create_controller_recovery_experience_example(&mismatch)
            .is_err()
    );

    let equal = recommendation(RecoveryRecommendationDecision::Requeue, "same");
    let mut equal_with_correction = request(
        input("recovery-task"),
        equal.clone(),
        equal,
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"wrong": true}),
        operator_reference: "operator:1".into(),
        reason: "not valid".into(),
    });
    assert!(
        app.create_controller_recovery_experience_example(&equal_with_correction)
            .is_err()
    );

    let differing_without_metadata = request(
        input("recovery-task"),
        observed.clone(),
        accepted.clone(),
        ControllerExperienceOutcome::Corrected,
    );
    assert!(
        app.create_controller_recovery_experience_example(&differing_without_metadata)
            .is_err()
    );

    let mut wrong_original = request(
        input("recovery-task"),
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    wrong_original.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"not": "the observed recommendation"}),
        operator_reference: "operator:2".into(),
        reason: "the observed output must be preserved exactly".into(),
    });
    assert!(
        app.create_controller_recovery_experience_example(&wrong_original)
            .is_err()
    );
    assert!(all(&app).is_empty());
}

#[test]
fn malformed_recovery_input_or_recommendation_writes_zero_rows() {
    let (_directory, app) = fixture();
    let valid = recommendation(RecoveryRecommendationDecision::Requeue, "valid");

    let mut oversized_input = input("recovery-task");
    oversized_input.current_request.observation.task_id = "x".repeat(70_000);
    assert!(
        app.create_controller_recovery_experience_example(&request(
            oversized_input,
            valid.clone(),
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let invalid_rationale = recommendation(RecoveryRecommendationDecision::Requeue, "");
    assert!(
        app.create_controller_recovery_experience_example(&request(
            input("recovery-task"),
            invalid_rationale,
            valid.clone(),
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );

    let mut invalid_confidence = valid.clone();
    invalid_confidence.confidence = Some(2.0);
    assert!(
        app.create_controller_recovery_experience_example(&request(
            input("recovery-task"),
            invalid_confidence,
            valid,
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );
    assert!(all(&app).is_empty());
}

#[test]
fn invalid_accepted_recommendation_writes_zero_rows() {
    let (_directory, app) = fixture();
    let observed = recommendation(RecoveryRecommendationDecision::Requeue, "observed");
    let invalid_accepted = recommendation(RecoveryRecommendationDecision::Requeue, "");

    assert!(
        app.create_controller_recovery_experience_example(&request(
            input("recovery-task"),
            observed,
            invalid_accepted,
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );
    assert!(all(&app).is_empty());
}

#[test]
fn m08_final_validation_rejects_quality_provenance_and_payload_before_persistence() {
    let (_directory, app) = fixture();
    let valid = recommendation(RecoveryRecommendationDecision::Requeue, "valid");

    let mut invalid_quality = request(
        input("recovery-task"),
        valid.clone(),
        valid.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_quality.quality.score = 101;
    assert!(
        app.create_controller_recovery_experience_example(&invalid_quality)
            .is_err()
    );

    let mut invalid_provenance = request(
        input("recovery-task"),
        valid.clone(),
        valid.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_provenance.provenance.project_id = Some(0);
    assert!(
        app.create_controller_recovery_experience_example(&invalid_provenance)
            .is_err()
    );

    let mut oversized_input = input("recovery-task");
    oversized_input.current_request.observation.dependencies = (0..16)
        .map(|index| orc::recovery::RecoveryDependency {
            task_id: format!("dependency-{index}-{}", "x".repeat(1_200)),
            status: None,
            is_done: false,
        })
        .collect();
    assert!(oversized_input.validate().is_ok());
    let serialized_size = serde_json::to_vec(&oversized_input).unwrap().len();
    assert!(serialized_size > MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES);
    assert!(serialized_size < 64 * 1024);

    assert!(
        app.create_controller_recovery_experience_example(&request(
            oversized_input,
            valid.clone(),
            valid,
            ControllerExperienceOutcome::Accepted,
        ))
        .is_err()
    );
    assert!(all(&app).is_empty());
}
