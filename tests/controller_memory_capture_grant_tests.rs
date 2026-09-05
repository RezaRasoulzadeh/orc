use orc::app::OrcApp;
use orc::controller_memory_capture_grant::{
    ControllerMemoryCaptureGrantError, ControllerMemoryCaptureGrantState,
    MAX_CONTROLLER_MEMORY_CAPTURE_ACTIONS,
};
use orc::controller_memory_mutation::{
    ControllerMemoryMutationAuthorizationRejection, ControllerMemoryMutationExecutionResult,
    ControllerMemoryMutationIntent,
};
use orc::memory::{MemoryDraft, MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope};
use orc::storage::Database;
use tempfile::TempDir;

fn open_app() -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project_id = db.create_project("capture-grant-test").unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    (directory, app, project_id)
}

fn draft(kind: MemoryKind, scope: MemoryScope, subject: &str) -> MemoryDraft {
    MemoryDraft {
        kind,
        scope,
        subject: subject.into(),
        content: format!("content for {subject}"),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::ControllerApproved,
            source_reference: Some("controller:capture-grant-test".into()),
        },
        confidence: Some(0.8),
    }
}

fn project_proposal(
    app: &OrcApp,
    project_id: i64,
    subject: &str,
) -> orc::controller_memory_mutation::ControllerMemoryMutationProposal {
    app.propose_controller_memory_mutation(ControllerMemoryMutationIntent::Create {
        draft: draft(
            MemoryKind::Project,
            MemoryScope::Project { project_id },
            subject,
        ),
    })
    .unwrap()
}

#[test]
fn eligible_project_and_episodic_creates_consume_shared_finite_budget_and_execute_canonically() {
    let (_directory, app, project_id) = open_app();
    let grant = app.create_controller_memory_capture_grant(2).unwrap();
    let clone = grant.clone();
    let project = project_proposal(&app, project_id, "project-fact");
    let project_authorization = app
        .inspect_controller_memory_capture_grant(&grant, &project)
        .unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(clone.remaining_actions().unwrap(), 1);
    assert!(matches!(
        app.execute_authorized_controller_memory_mutation(&project, Some(project_authorization)),
        ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));

    let episodic = app
        .propose_controller_memory_mutation(ControllerMemoryMutationIntent::Create {
            draft: draft(
                MemoryKind::Episodic,
                MemoryScope::Project { project_id },
                "episodic-fact",
            ),
        })
        .unwrap();
    let episodic_authorization = app
        .inspect_controller_memory_capture_grant(&clone, &episodic)
        .unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(grant.state(), ControllerMemoryCaptureGrantState::Exhausted);
    assert!(matches!(
        app.execute_authorized_controller_memory_mutation(&episodic, Some(episodic_authorization)),
        ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));
    assert_eq!(app.memories().unwrap().list(None, false).unwrap().len(), 2);
}

#[test]
fn budget_bounds_exhaustion_revocation_and_clone_state_are_shared() {
    let (_directory, app, project_id) = open_app();
    assert!(matches!(
        app.create_controller_memory_capture_grant(0),
        Err(ControllerMemoryCaptureGrantError::InvalidBudget)
    ));
    assert!(matches!(
        app.create_controller_memory_capture_grant(MAX_CONTROLLER_MEMORY_CAPTURE_ACTIONS + 1),
        Err(ControllerMemoryCaptureGrantError::InvalidBudget)
    ));

    let grant = app.create_controller_memory_capture_grant(1).unwrap();
    let clone = grant.clone();
    let proposal = project_proposal(&app, project_id, "one-shot");
    let _authorization = app
        .inspect_controller_memory_capture_grant(&grant, &proposal)
        .unwrap();
    assert!(matches!(
        app.inspect_controller_memory_capture_grant(&clone, &proposal),
        Err(ControllerMemoryCaptureGrantError::Exhausted)
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 0);

    let revoked = app.create_controller_memory_capture_grant(1).unwrap();
    let revoked_clone = revoked.clone();
    revoked.revoke().unwrap();
    assert_eq!(
        revoked_clone.state(),
        ControllerMemoryCaptureGrantState::Revoked
    );
    assert!(matches!(
        app.inspect_controller_memory_capture_grant(&revoked_clone, &proposal),
        Err(ControllerMemoryCaptureGrantError::Revoked)
    ));
    assert_eq!(revoked.remaining_actions().unwrap(), 1);
}

#[test]
fn global_and_maintenance_proposals_are_rejected_without_consumption() {
    let (_directory, app, project_id) = open_app();
    let grant = app.create_controller_memory_capture_grant(5).unwrap();
    for kind in [MemoryKind::User, MemoryKind::Experience] {
        let proposal = app
            .propose_controller_memory_mutation(ControllerMemoryMutationIntent::Create {
                draft: draft(kind, MemoryScope::Global, kind.as_str()),
            })
            .unwrap();
        assert!(matches!(
            app.inspect_controller_memory_capture_grant(&grant, &proposal),
            Err(ControllerMemoryCaptureGrantError::UnsupportedKind(rejected))
                if rejected == kind
        ));
    }

    let target = app
        .memories()
        .unwrap()
        .create(&draft(
            MemoryKind::Project,
            MemoryScope::Project { project_id },
            "maintenance-target",
        ))
        .unwrap();
    let replacement = draft(
        MemoryKind::Project,
        MemoryScope::Project { project_id },
        "maintenance-target",
    );
    let maintenance = [
        ControllerMemoryMutationIntent::Correct {
            target: target.id.clone(),
            replacement: replacement.clone(),
        },
        ControllerMemoryMutationIntent::Supersede {
            target: target.id.clone(),
            replacement,
        },
        ControllerMemoryMutationIntent::Remove { target: target.id },
    ];
    for intent in maintenance {
        let proposal = app.propose_controller_memory_mutation(intent).unwrap();
        assert!(matches!(
            app.inspect_controller_memory_capture_grant(&grant, &proposal),
            Err(ControllerMemoryCaptureGrantError::UnsupportedOperation(_))
        ));
    }
    assert_eq!(grant.remaining_actions().unwrap(), 5);
}

#[test]
fn exact_authorization_is_not_reusable_for_a_different_proposal() {
    let (_directory, app, project_id) = open_app();
    let grant = app.create_controller_memory_capture_grant(2).unwrap();
    let first = project_proposal(&app, project_id, "first");
    let second = project_proposal(&app, project_id, "second");
    let authorization = app
        .inspect_controller_memory_capture_grant(&grant, &first)
        .unwrap();
    assert!(matches!(
        app.execute_authorized_controller_memory_mutation(&second, Some(authorization)),
        ControllerMemoryMutationExecutionResult::AuthorizationRejected {
            reason: ControllerMemoryMutationAuthorizationRejection::NotAuthorizedForIntent,
            ..
        }
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    let second_authorization = app
        .inspect_controller_memory_capture_grant(&grant, &second)
        .unwrap();
    assert!(matches!(
        app.execute_authorized_controller_memory_mutation(&second, Some(second_authorization)),
        ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));
}

#[test]
fn reopening_preserves_memory_but_never_reconstructs_capture_grant() {
    let (directory, app, project_id) = open_app();
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let grant = app.create_controller_memory_capture_grant(2).unwrap();
    let proposal = project_proposal(&app, project_id, "restart-safe");
    let authorization = app
        .inspect_controller_memory_capture_grant(&grant, &proposal)
        .unwrap();
    assert!(matches!(
        app.execute_authorized_controller_memory_mutation(&proposal, Some(authorization)),
        ControllerMemoryMutationExecutionResult::Mutated { .. }
    ));
    drop(app);
    drop(grant);

    let reopened = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    assert_eq!(
        reopened
            .memories()
            .unwrap()
            .list(None, false)
            .unwrap()
            .len(),
        1
    );
    let new_grant = reopened.create_controller_memory_capture_grant(2).unwrap();
    assert_eq!(new_grant.remaining_actions().unwrap(), 2);
}

#[test]
fn wrong_project_and_scope_mismatch_are_rejected_before_budget_consumption() {
    let (_directory, app, project_id) = open_app();
    let grant = app.create_controller_memory_capture_grant(2).unwrap();
    let other_project = project_id + 1;
    let other_app_proposal =
        app.propose_controller_memory_mutation(ControllerMemoryMutationIntent::Create {
            draft: draft(
                MemoryKind::Project,
                MemoryScope::Project {
                    project_id: other_project,
                },
                "other-project",
            ),
        });
    assert!(matches!(
        other_app_proposal,
        Err(orc::controller_memory_mutation::ControllerMemoryMutationError::InvalidProjectBinding)
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 2);
}

#[test]
fn grant_api_does_not_persist_or_automatically_invoke_capture_judgment() {
    let (_directory, app, _project_id) = open_app();
    let grant = app.create_controller_memory_capture_grant(1).unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert!(
        app.memories()
            .unwrap()
            .list(Some(MemoryKind::Project), false)
            .unwrap()
            .is_empty()
    );
}
