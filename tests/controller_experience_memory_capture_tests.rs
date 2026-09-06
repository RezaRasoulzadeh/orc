use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis, MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES,
};
use orc::controller_experience_memory_capture::{
    CONTROLLER_MEMORY_CAPTURE_EXPERIENCE_CAPABILITY, ControllerExperienceMemoryCaptureRequest,
};
use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_memory_capture::{
    ControllerMemoryCaptureCandidate, ControllerMemoryCaptureError, ControllerMemoryCaptureInput,
    ControllerMemoryCaptureRequest, ControllerMemoryCaptureResult,
    MAX_CONTROLLER_MEMORY_CAPTURE_INPUT_BYTES,
};
use orc::controller_memory_mutation::ControllerMemoryMutationIntent;
use orc::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
    MemoryScope,
};
use orc::storage::Database;
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = database.create_project("memory capture curation").unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app, project_id)
}

fn candidate(project_id: i64) -> ControllerMemoryCaptureCandidate {
    ControllerMemoryCaptureCandidate {
        draft: MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id },
            subject: "release-gate".into(),
            content: "Production releases require an operator approval checklist.".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ProjectFact,
                source_reference: Some("operator:release-decision".into()),
            },
            confidence: Some(0.9),
        },
        source_facts: vec![
            "Explicit current-project operator decision.".into(),
            "The candidate is intended for durable project memory.".into(),
        ],
    }
}

fn memory_item(id: i64, content: &str) -> ControllerMemoryItem {
    ControllerMemoryItem {
        id: MemoryId::Global(id),
        kind: MemoryKind::User,
        scope: MemoryScope::Global,
        authority: ControllerMemoryAuthority::DurableUser,
        subject: "capture-guidance".into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Operator,
            source_reference: Some("test:memory-capture-curation".into()),
        },
        confidence: Some(0.8),
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

fn input(project_id: i64) -> ControllerMemoryCaptureInput {
    let request = ControllerMemoryCaptureRequest::from_candidate(candidate(project_id));
    ControllerMemoryCaptureInput::from_request(
        &request,
        memory_context("Historical memory is advisory; the supplied candidate is authoritative."),
    )
}

fn propose(input: &ControllerMemoryCaptureInput) -> ControllerMemoryCaptureResult {
    ControllerMemoryCaptureResult::ProposeMutation {
        intent: ControllerMemoryMutationIntent::Create {
            draft: input.current_request.candidate.draft.clone(),
        },
    }
}

fn request(
    input: ControllerMemoryCaptureInput,
    observed: ControllerMemoryCaptureResult,
    accepted: ControllerMemoryCaptureResult,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperienceMemoryCaptureRequest {
    ControllerExperienceMemoryCaptureRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit memory-capture curation evidence".into(),
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

fn assert_zero_rows(app: &OrcApp, request: &ControllerExperienceMemoryCaptureRequest) {
    assert!(
        app.create_controller_memory_capture_experience_example(request)
            .is_err()
    );
    assert!(all(app).is_empty());
}

#[test]
fn equal_ignore_persists_one_exact_active_non_correction_example() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let expected_input = serde_json::to_value(&input).unwrap();
    let request = request(
        input,
        ControllerMemoryCaptureResult::Ignore,
        ControllerMemoryCaptureResult::Ignore,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_memory_capture_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.accepted_output,
        serde_json::json!({"decision": "ignore"})
    );
    assert_eq!(
        stored.capability,
        CONTROLLER_MEMORY_CAPTURE_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert!(stored.correction.is_none());
}

#[test]
fn equal_candidate_backed_proposal_persists_exact_complete_result() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let output = propose(&input);
    let expected_input = serde_json::to_value(&input).unwrap();
    let expected_output = serde_json::to_value(&output).unwrap();
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_memory_capture_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(stored.accepted_output, expected_output);
    assert_eq!(stored.accepted_output["decision"], "propose_mutation");
    assert_eq!(
        stored.accepted_output["intent"]["draft"],
        serde_json::to_value(&request.input.current_request.candidate.draft).unwrap()
    );
    assert!(stored.correction.is_none());
}

#[test]
fn corrected_ignore_to_proposal_preserves_exact_outputs() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let observed = ControllerMemoryCaptureResult::Ignore;
    let accepted = propose(&input);
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:capture-1".into(),
        reason: "The candidate-backed proposal was explicitly accepted.".into(),
    });

    let stored = app
        .create_controller_memory_capture_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn corrected_proposal_to_ignore_preserves_exact_outputs() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let observed = propose(&input);
    let accepted = ControllerMemoryCaptureResult::Ignore;
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:capture-2".into(),
        reason: "The operator explicitly declined the mutation proposal.".into(),
    });

    let stored = app
        .create_controller_memory_capture_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn exact_full_input_projection_preserves_candidate_source_facts_and_memory() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let expected = serde_json::to_value(&input).unwrap();
    let output = ControllerMemoryCaptureResult::Ignore;
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    let stored = app
        .create_controller_memory_capture_experience_example(&request)
        .unwrap();

    assert_eq!(stored.input, expected);
    assert_eq!(
        stored.input["current_request"]["candidate"]["draft"]["subject"],
        "release-gate"
    );
    assert_eq!(
        stored.input["current_request"]["candidate"]["source_facts"][0],
        "Explicit current-project operator decision."
    );
    assert_eq!(
        stored.input["memory"]["items"][0]["content"],
        "Historical memory is advisory; the supplied candidate is authoritative."
    );
    assert_eq!(stored.capability, "controller.memory_capture");
}

#[test]
fn malformed_input_and_production_result_validation_failures_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let valid_input = input(project_id);
    let valid_ignore = ControllerMemoryCaptureResult::Ignore;

    let mut malformed_input = valid_input.clone();
    malformed_input.current_request.packet_version = 0;
    assert_zero_rows(
        &app,
        &request(
            malformed_input,
            valid_ignore.clone(),
            valid_ignore.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let invalid_create = ControllerMemoryCaptureResult::ProposeMutation {
        intent: ControllerMemoryMutationIntent::Create {
            draft: MemoryDraft {
                kind: MemoryKind::Project,
                scope: MemoryScope::Project { project_id },
                subject: "invalid".into(),
                content: String::new(),
                provenance: MemoryProvenance {
                    kind: MemoryProvenanceKind::ProjectFact,
                    source_reference: None,
                },
                confidence: None,
            },
        },
    };
    assert_zero_rows(
        &app,
        &request(
            valid_input.clone(),
            invalid_create,
            valid_ignore.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut different_candidate = valid_input.current_request.candidate.draft.clone();
    different_candidate.subject = "not-the-candidate".into();
    assert_zero_rows(
        &app,
        &request(
            valid_input.clone(),
            ControllerMemoryCaptureResult::ProposeMutation {
                intent: ControllerMemoryMutationIntent::Create {
                    draft: different_candidate,
                },
            },
            valid_ignore.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let maintenance_intent = ControllerMemoryMutationIntent::Remove {
        target: MemoryId::Project { project_id, id: 1 },
    };
    assert_zero_rows(
        &app,
        &request(
            valid_input.clone(),
            ControllerMemoryCaptureResult::ProposeMutation {
                intent: maintenance_intent.clone(),
            },
            valid_ignore.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    assert_zero_rows(
        &app,
        &request(
            valid_input.clone(),
            valid_ignore.clone(),
            ControllerMemoryCaptureResult::ProposeMutation {
                intent: ControllerMemoryMutationIntent::Create {
                    draft: MemoryDraft {
                        kind: MemoryKind::Project,
                        scope: MemoryScope::Project { project_id },
                        subject: "accepted-invalid".into(),
                        content: String::new(),
                        provenance: MemoryProvenance {
                            kind: MemoryProvenanceKind::ProjectFact,
                            source_reference: None,
                        },
                        confidence: None,
                    },
                },
            },
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut accepted_different = valid_input.current_request.candidate.draft.clone();
    accepted_different.subject = "accepted-not-the-candidate".into();
    assert_zero_rows(
        &app,
        &request(
            valid_input.clone(),
            valid_ignore.clone(),
            ControllerMemoryCaptureResult::ProposeMutation {
                intent: ControllerMemoryMutationIntent::Create {
                    draft: accepted_different,
                },
            },
            ControllerExperienceOutcome::Accepted,
        ),
    );

    assert_zero_rows(
        &app,
        &request(
            valid_input,
            valid_ignore,
            ControllerMemoryCaptureResult::ProposeMutation {
                intent: maintenance_intent,
            },
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn genuine_production_input_bound_rejection_writes_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let mut large = input(project_id);
    large.current_request.candidate.source_facts = (0..16)
        .map(|index| format!("fact-{index}-{}", "f".repeat(2_040)))
        .collect();
    large.memory = ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items: (1..=8)
            .map(|id| memory_item(id, &"m".repeat(3_790)))
            .collect(),
    };
    assert!(large.current_request.candidate.validate().is_ok());
    assert!(large.memory.validate().is_ok());
    let error = large.validate().unwrap_err();
    assert!(matches!(
        error,
        ControllerMemoryCaptureError::InputTooLarge {
            max: MAX_CONTROLLER_MEMORY_CAPTURE_INPUT_BYTES,
            ..
        }
    ));
    assert_zero_rows(
        &app,
        &request(
            large,
            ControllerMemoryCaptureResult::Ignore,
            ControllerMemoryCaptureResult::Ignore,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn invalid_m08_metadata_and_correction_states_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let output = ControllerMemoryCaptureResult::Ignore;

    let mut invalid_quality = request(
        input.clone(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_quality.quality.score = 101;
    assert_zero_rows(&app, &invalid_quality);

    let mut invalid_provenance = request(
        input.clone(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_provenance.provenance.project_id = Some(0);
    assert_zero_rows(&app, &invalid_provenance);

    let mut equal_with_correction = request(
        input.clone(),
        output.clone(),
        output.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::to_value(&output).unwrap(),
        operator_reference: "operator:capture".into(),
        reason: "Equal outputs cannot be corrected.".into(),
    });
    assert_zero_rows(&app, &equal_with_correction);

    assert_zero_rows(
        &app,
        &request(
            input.clone(),
            output.clone(),
            output.clone(),
            ControllerExperienceOutcome::Corrected,
        ),
    );

    let accepted = propose(&input);
    assert_zero_rows(
        &app,
        &request(
            input.clone(),
            output.clone(),
            accepted.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );
    assert_zero_rows(
        &app,
        &request(
            input.clone(),
            output.clone(),
            accepted.clone(),
            ControllerExperienceOutcome::Corrected,
        ),
    );

    let mut wrong_original = request(
        input,
        output,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    wrong_original.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"decision": "not-observed"}),
        operator_reference: "operator:capture".into(),
        reason: "Observed output must be preserved exactly.".into(),
    });
    assert_zero_rows(&app, &wrong_original);
}

#[test]
fn distinct_m08_payload_and_complete_record_bounds_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let mut large_input = input(project_id);
    large_input.memory = ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items: (1..=8)
            .map(|id| memory_item(id, &"m".repeat(2_500)))
            .collect(),
    };
    assert!(large_input.validate().is_ok());
    assert!(
        serde_json::to_vec(&large_input).unwrap().len() > MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES
    );
    assert_zero_rows(
        &app,
        &request(
            large_input,
            ControllerMemoryCaptureResult::Ignore,
            ControllerMemoryCaptureResult::Ignore,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut selected = None;
    for content_length in [14_000, 15_000, 15_500, 16_000] {
        for memory_length in [0, 1_000, 2_000, 3_000, 4_000] {
            let mut candidate_input = input(project_id);
            candidate_input.current_request.candidate.draft.content = "c".repeat(content_length);
            candidate_input.memory = ControllerMemoryContext {
                context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
                items: if memory_length == 0 {
                    vec![]
                } else {
                    vec![memory_item(1, &"m".repeat(memory_length))]
                },
            };
            let accepted = propose(&candidate_input);
            let candidate_request = request(
                candidate_input,
                accepted.clone(),
                accepted,
                ControllerExperienceOutcome::Accepted,
            );
            if candidate_request.input.validate().is_ok()
                && serde_json::to_vec(&candidate_request.input).unwrap().len()
                    <= MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES
                && candidate_request
                    .accepted
                    .validate(&candidate_request.input.current_request.candidate)
                    .is_ok()
                && serde_json::to_vec(&candidate_request.accepted)
                    .unwrap()
                    .len()
                    <= MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES
            {
                let mut candidate_request = candidate_request;
                candidate_request.provenance.task_id = Some("t".repeat(256));
                candidate_request.provenance.source_reference = Some("s".repeat(256));
                candidate_request.quality.rationale = "q".repeat(1024);
                if candidate_request
                    .into_example_draft()
                    .unwrap()
                    .validate()
                    .is_err()
                {
                    selected = Some(candidate_request);
                    break;
                }
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let request =
        selected.expect("valid candidate/result values should exceed the M08 record bound");
    assert_zero_rows(&app, &request);
}

#[test]
fn successful_call_creates_exactly_one_row() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let output = propose(&input);
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    app.create_controller_memory_capture_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
}
