//! Opaque, bounded permission for supervised Controller memory capture.
//!
//! This capability is deliberately separate from routine Controller task
//! continuation. It can only mint the existing M06-009 one-shot authorization
//! for an already validated project-bound Project or Episodic Create proposal.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use thiserror::Error;

use crate::controller_memory_mutation::{
    ControllerMemoryMutationAuthorization, ControllerMemoryMutationOperation,
    ControllerMemoryMutationProposal,
};
use crate::memory::{MemoryKind, MemoryScope};

/// The maximum number of memory-capture mutations one grant can authorize.
/// This is independent of task continuation, workflow transitions, and task
/// revision limits.
pub const MAX_CONTROLLER_MEMORY_CAPTURE_ACTIONS: usize = 128;

static NEXT_GRANT_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque identity for one in-process memory-capture grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ControllerMemoryCaptureGrantId(u64);

/// Observable lifecycle of a memory-capture grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerMemoryCaptureGrantState {
    Active,
    Exhausted,
    Revoked,
}

#[derive(Debug)]
struct GrantState {
    id: ControllerMemoryCaptureGrantId,
    project_id: i64,
    remaining_actions: usize,
    state: ControllerMemoryCaptureGrantState,
}

/// Trusted, finite, project-bound memory-capture permission.
///
/// The grant contains no model, storage, workflow, or mutation handle and has
/// no serialization or persistence implementation. Clones share all mutable
/// state so copying cannot reset the budget or revocation.
#[derive(Clone, Debug)]
pub struct ControllerMemoryCaptureGrant {
    state: Arc<Mutex<GrantState>>,
}

#[derive(Debug, Error)]
pub enum ControllerMemoryCaptureGrantError {
    #[error("memory capture grant project binding must be positive")]
    InvalidProject,
    #[error(
        "memory capture grant budget must be between 1 and {MAX_CONTROLLER_MEMORY_CAPTURE_ACTIONS}"
    )]
    InvalidBudget,
    #[error("memory capture grant is bound to project {expected}, not current project {actual}")]
    WrongProject { expected: i64, actual: i64 },
    #[error("memory capture grant is exhausted")]
    Exhausted,
    #[error("memory capture grant is revoked")]
    Revoked,
    #[error("memory capture grant state is unavailable")]
    StateUnavailable,
    #[error("memory capture proposal is bound to project {actual}, expected {expected}")]
    ProposalProjectMismatch { expected: i64, actual: i64 },
    #[error("memory capture proposal operation {0:?} is not Create")]
    UnsupportedOperation(ControllerMemoryMutationOperation),
    #[error("memory kind {0:?} is not eligible for automatic capture")]
    UnsupportedKind(MemoryKind),
    #[error("memory capture proposal must use the exact project scope")]
    InvalidScope,
}

impl ControllerMemoryCaptureGrant {
    pub(crate) fn new(
        project_id: i64,
        action_budget: usize,
    ) -> Result<Self, ControllerMemoryCaptureGrantError> {
        if project_id <= 0 {
            return Err(ControllerMemoryCaptureGrantError::InvalidProject);
        }
        if action_budget == 0 || action_budget > MAX_CONTROLLER_MEMORY_CAPTURE_ACTIONS {
            return Err(ControllerMemoryCaptureGrantError::InvalidBudget);
        }
        let id = ControllerMemoryCaptureGrantId(NEXT_GRANT_ID.fetch_add(1, Ordering::Relaxed));
        Ok(Self {
            state: Arc::new(Mutex::new(GrantState {
                id,
                project_id,
                remaining_actions: action_budget,
                state: ControllerMemoryCaptureGrantState::Active,
            })),
        })
    }

    pub fn id(&self) -> Result<ControllerMemoryCaptureGrantId, ControllerMemoryCaptureGrantError> {
        Ok(self.lock()?.id)
    }

    pub fn project_id(&self) -> Result<i64, ControllerMemoryCaptureGrantError> {
        Ok(self.lock()?.project_id)
    }

    pub fn remaining_actions(&self) -> Result<usize, ControllerMemoryCaptureGrantError> {
        Ok(self.lock()?.remaining_actions)
    }

    pub fn state(&self) -> ControllerMemoryCaptureGrantState {
        self.state
            .lock()
            .map(|state| state.state)
            .unwrap_or(ControllerMemoryCaptureGrantState::Revoked)
    }

    /// Revoke this grant. All clones observe the same terminal state.
    pub fn revoke(&self) -> Result<(), ControllerMemoryCaptureGrantError> {
        let mut state = self.lock()?;
        if state.state != ControllerMemoryCaptureGrantState::Exhausted {
            state.state = ControllerMemoryCaptureGrantState::Revoked;
        }
        Ok(())
    }

    pub(crate) fn inspect_and_authorize<F>(
        &self,
        current_project_id: i64,
        proposal: &ControllerMemoryMutationProposal,
        authorize: F,
    ) -> Result<ControllerMemoryMutationAuthorization, ControllerMemoryCaptureGrantError>
    where
        F: FnOnce(
            &ControllerMemoryMutationProposal,
        ) -> Result<
            ControllerMemoryMutationAuthorization,
            ControllerMemoryCaptureGrantError,
        >,
    {
        let mut state = self.lock()?;
        if state.project_id != current_project_id {
            return Err(ControllerMemoryCaptureGrantError::WrongProject {
                expected: state.project_id,
                actual: current_project_id,
            });
        }
        match state.state {
            ControllerMemoryCaptureGrantState::Active => {}
            ControllerMemoryCaptureGrantState::Exhausted => {
                return Err(ControllerMemoryCaptureGrantError::Exhausted);
            }
            ControllerMemoryCaptureGrantState::Revoked => {
                return Err(ControllerMemoryCaptureGrantError::Revoked);
            }
        }
        if proposal.project_id() != state.project_id {
            return Err(ControllerMemoryCaptureGrantError::ProposalProjectMismatch {
                expected: state.project_id,
                actual: proposal.project_id(),
            });
        }
        let crate::controller_memory_mutation::ControllerMemoryMutationIntent::Create { draft } =
            proposal.intent()
        else {
            return Err(ControllerMemoryCaptureGrantError::UnsupportedOperation(
                proposal.operation(),
            ));
        };
        match (draft.kind, &draft.scope) {
            (MemoryKind::Project | MemoryKind::Episodic, MemoryScope::Project { project_id })
                if *project_id == state.project_id => {}
            (MemoryKind::Project | MemoryKind::Episodic, _) => {
                return Err(ControllerMemoryCaptureGrantError::InvalidScope);
            }
            (kind, _) => return Err(ControllerMemoryCaptureGrantError::UnsupportedKind(kind)),
        }

        // M06-009 authorization is the exact mint point. Budget changes only
        // after it succeeds; later execution is intentionally not refundable.
        let authorization = authorize(proposal)?;
        state.remaining_actions -= 1;
        if state.remaining_actions == 0 {
            state.state = ControllerMemoryCaptureGrantState::Exhausted;
        }
        Ok(authorization)
    }

    fn lock(&self) -> Result<MutexGuard<'_, GrantState>, ControllerMemoryCaptureGrantError> {
        self.state
            .lock()
            .map_err(|_| ControllerMemoryCaptureGrantError::StateUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_memory_mutation::{
        ControllerMemoryMutationExecutionResult, ControllerMemoryMutationIntent, authorize, execute,
    };
    use crate::memory::{MemoryDraft, MemoryProvenance, MemoryProvenanceKind, MemoryService};
    use crate::storage::Database;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Database, i64) {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        let project_id = db.create_project("capture grant").unwrap();
        (directory, db, project_id)
    }

    fn draft(kind: MemoryKind, scope: MemoryScope, subject: &str) -> MemoryDraft {
        MemoryDraft {
            kind,
            scope,
            subject: subject.into(),
            content: "capture-grant test content".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ControllerApproved,
                source_reference: Some("controller:capture-grant-unit-test".into()),
            },
            confidence: Some(0.8),
        }
    }

    #[test]
    fn wrong_project_and_proposal_binding_are_rejected_before_authorization() {
        let (_directory, db, project_id) = setup();
        let other_project = project_id + 1;
        let service = MemoryService::new(&db, project_id);
        let proposal = ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Create {
                draft: draft(
                    MemoryKind::Project,
                    MemoryScope::Project { project_id },
                    "valid",
                ),
            },
            &service,
        )
        .unwrap();
        let grant = ControllerMemoryCaptureGrant::new(project_id, 3).unwrap();
        assert!(matches!(
            grant.inspect_and_authorize(other_project, &proposal, |proposal| {
                Ok(authorize(proposal))
            }),
            Err(ControllerMemoryCaptureGrantError::WrongProject { .. })
        ));

        let other_proposal = ControllerMemoryMutationProposal::from_intent(
            other_project,
            ControllerMemoryMutationIntent::Create {
                draft: draft(
                    MemoryKind::Project,
                    MemoryScope::Project {
                        project_id: other_project,
                    },
                    "other",
                ),
            },
            &service,
        )
        .unwrap();
        assert!(matches!(
            grant.inspect_and_authorize(project_id, &other_proposal, |proposal| {
                Ok(authorize(proposal))
            }),
            Err(ControllerMemoryCaptureGrantError::ProposalProjectMismatch { .. })
        ));

        assert_eq!(grant.remaining_actions().unwrap(), 3);
    }

    #[test]
    fn post_mint_fresh_validation_failure_consumes_without_refund() {
        let (_directory, db, project_id) = setup();
        let service = MemoryService::new(&db, project_id);
        let proposal = ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Create {
                draft: draft(
                    MemoryKind::Episodic,
                    MemoryScope::Project { project_id },
                    "fresh-validation",
                ),
            },
            &service,
        )
        .unwrap();
        let grant = ControllerMemoryCaptureGrant::new(project_id, 1).unwrap();
        let authorization = grant
            .inspect_and_authorize(project_id, &proposal, |proposal| Ok(authorize(proposal)))
            .unwrap();
        assert_eq!(grant.remaining_actions().unwrap(), 0);
        assert!(matches!(
            execute(&proposal, Some(authorization), project_id + 1, &service),
            ControllerMemoryMutationExecutionResult::FreshValidationRejected { .. }
        ));
        assert_eq!(grant.remaining_actions().unwrap(), 0);
        assert_eq!(grant.state(), ControllerMemoryCaptureGrantState::Exhausted);
    }
}
