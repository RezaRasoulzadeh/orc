//! Opaque, bounded permission for supervised Controller memory maintenance.
//!
//! This capability is separate from Controller memory capture and task
//! continuation. It can only mint the existing M06-009 one-shot
//! authorization for an already validated project-bound Correct, Supersede,
//! or Remove proposal targeting Project or Episodic memory.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use thiserror::Error;

use crate::controller_memory_mutation::{
    ControllerMemoryMutationAuthorization, ControllerMemoryMutationOperation,
    ControllerMemoryMutationProposal,
};
use crate::memory::{MemoryId, MemoryKind, MemoryScope};

/// The maximum number of maintenance mutations one grant can authorize.
pub const MAX_CONTROLLER_MEMORY_MAINTENANCE_ACTIONS: usize = 128;

static NEXT_GRANT_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque identity for one in-process memory-maintenance grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ControllerMemoryMaintenanceGrantId(u64);

/// Observable lifecycle of a memory-maintenance grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerMemoryMaintenanceGrantState {
    Active,
    Exhausted,
    Revoked,
}

#[derive(Debug)]
struct GrantState {
    id: ControllerMemoryMaintenanceGrantId,
    project_id: i64,
    remaining_actions: usize,
    state: ControllerMemoryMaintenanceGrantState,
}

/// Trusted, finite, project-bound memory-maintenance permission.
///
/// The grant contains no runtime, memory service, database, workflow, or
/// mutation handle and has no serialization or persistence implementation.
/// Clones share all mutable state so copying cannot reset budget or revocation.
#[derive(Clone, Debug)]
pub struct ControllerMemoryMaintenanceGrant {
    state: Arc<Mutex<GrantState>>,
}

#[derive(Debug, Error)]
pub enum ControllerMemoryMaintenanceGrantError {
    #[error("memory maintenance grant project binding must be positive")]
    InvalidProject,
    #[error(
        "memory maintenance grant budget must be between 1 and {MAX_CONTROLLER_MEMORY_MAINTENANCE_ACTIONS}"
    )]
    InvalidBudget,
    #[error(
        "memory maintenance grant is bound to project {expected}, not current project {actual}"
    )]
    WrongProject { expected: i64, actual: i64 },
    #[error("memory maintenance grant is exhausted")]
    Exhausted,
    #[error("memory maintenance grant is revoked")]
    Revoked,
    #[error("memory maintenance grant state is unavailable")]
    StateUnavailable,
    #[error("memory maintenance proposal is bound to project {actual}, expected {expected}")]
    ProposalProjectMismatch { expected: i64, actual: i64 },
    #[error("memory maintenance proposal operation {0:?} is not eligible")]
    UnsupportedOperation(ControllerMemoryMutationOperation),
    #[error("memory kind {0:?} is not eligible for automatic maintenance")]
    UnsupportedKind(MemoryKind),
    #[error("memory maintenance target must use the exact project scope")]
    InvalidScope,
    #[error("memory maintenance target was not found")]
    TargetNotFound,
    #[error("memory maintenance target lookup failed: {0}")]
    TargetLookupFailed(String),
}

impl ControllerMemoryMaintenanceGrant {
    pub(crate) fn new(
        project_id: i64,
        action_budget: usize,
    ) -> Result<Self, ControllerMemoryMaintenanceGrantError> {
        if project_id <= 0 {
            return Err(ControllerMemoryMaintenanceGrantError::InvalidProject);
        }
        if action_budget == 0 || action_budget > MAX_CONTROLLER_MEMORY_MAINTENANCE_ACTIONS {
            return Err(ControllerMemoryMaintenanceGrantError::InvalidBudget);
        }
        let id = ControllerMemoryMaintenanceGrantId(NEXT_GRANT_ID.fetch_add(1, Ordering::Relaxed));
        Ok(Self {
            state: Arc::new(Mutex::new(GrantState {
                id,
                project_id,
                remaining_actions: action_budget,
                state: ControllerMemoryMaintenanceGrantState::Active,
            })),
        })
    }

    pub fn id(
        &self,
    ) -> Result<ControllerMemoryMaintenanceGrantId, ControllerMemoryMaintenanceGrantError> {
        Ok(self.lock()?.id)
    }

    pub fn project_id(&self) -> Result<i64, ControllerMemoryMaintenanceGrantError> {
        Ok(self.lock()?.project_id)
    }

    pub fn remaining_actions(&self) -> Result<usize, ControllerMemoryMaintenanceGrantError> {
        Ok(self.lock()?.remaining_actions)
    }

    pub fn state(&self) -> ControllerMemoryMaintenanceGrantState {
        self.state
            .lock()
            .map(|state| state.state)
            .unwrap_or(ControllerMemoryMaintenanceGrantState::Revoked)
    }

    /// Revoke this grant. All clones observe the same terminal state.
    pub fn revoke(&self) -> Result<(), ControllerMemoryMaintenanceGrantError> {
        let mut state = self.lock()?;
        if state.state != ControllerMemoryMaintenanceGrantState::Exhausted {
            state.state = ControllerMemoryMaintenanceGrantState::Revoked;
        }
        Ok(())
    }

    pub(crate) fn inspect_and_authorize<F, K>(
        &self,
        current_project_id: i64,
        proposal: &ControllerMemoryMutationProposal,
        resolve_target_kind: F,
        authorize: K,
    ) -> Result<ControllerMemoryMutationAuthorization, ControllerMemoryMaintenanceGrantError>
    where
        F: FnOnce(&MemoryId) -> Result<MemoryKind, ControllerMemoryMaintenanceGrantError>,
        K: FnOnce(
            &ControllerMemoryMutationProposal,
        ) -> Result<
            ControllerMemoryMutationAuthorization,
            ControllerMemoryMaintenanceGrantError,
        >,
    {
        let mut state = self.lock()?;
        if state.project_id != current_project_id {
            return Err(ControllerMemoryMaintenanceGrantError::WrongProject {
                expected: state.project_id,
                actual: current_project_id,
            });
        }
        match state.state {
            ControllerMemoryMaintenanceGrantState::Active => {}
            ControllerMemoryMaintenanceGrantState::Exhausted => {
                return Err(ControllerMemoryMaintenanceGrantError::Exhausted);
            }
            ControllerMemoryMaintenanceGrantState::Revoked => {
                return Err(ControllerMemoryMaintenanceGrantError::Revoked);
            }
        }
        if proposal.project_id() != state.project_id {
            return Err(
                ControllerMemoryMaintenanceGrantError::ProposalProjectMismatch {
                    expected: state.project_id,
                    actual: proposal.project_id(),
                },
            );
        }

        let target = match proposal.intent() {
            crate::controller_memory_mutation::ControllerMemoryMutationIntent::Correct {
                target,
                ..
            }
            | crate::controller_memory_mutation::ControllerMemoryMutationIntent::Supersede {
                target,
                ..
            }
            | crate::controller_memory_mutation::ControllerMemoryMutationIntent::Remove {
                target,
            } => target,
            crate::controller_memory_mutation::ControllerMemoryMutationIntent::Create {
                ..
            } => {
                return Err(ControllerMemoryMaintenanceGrantError::UnsupportedOperation(
                    ControllerMemoryMutationOperation::Create,
                ));
            }
        };
        match target.scope() {
            MemoryScope::Project { project_id } if project_id == state.project_id => {}
            MemoryScope::Global | MemoryScope::Project { .. } => {
                return Err(ControllerMemoryMaintenanceGrantError::InvalidScope);
            }
        }
        let kind = resolve_target_kind(target)?;
        if !matches!(kind, MemoryKind::Project | MemoryKind::Episodic) {
            return Err(ControllerMemoryMaintenanceGrantError::UnsupportedKind(kind));
        }

        // M06-009 authorization is the exact mint point. Budget changes only
        // after it succeeds; later execution is intentionally not refundable.
        let authorization = authorize(proposal)?;
        state.remaining_actions -= 1;
        if state.remaining_actions == 0 {
            state.state = ControllerMemoryMaintenanceGrantState::Exhausted;
        }
        Ok(authorization)
    }

    fn lock(&self) -> Result<MutexGuard<'_, GrantState>, ControllerMemoryMaintenanceGrantError> {
        self.state
            .lock()
            .map_err(|_| ControllerMemoryMaintenanceGrantError::StateUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_memory_mutation::{
        ControllerMemoryMutationIntent, ControllerMemoryMutationProposal, authorize,
    };
    use crate::memory::{MemoryDraft, MemoryProvenance, MemoryProvenanceKind};
    use crate::storage::Database;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Database, i64, i64) {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::init(directory.path().join("maintenance-grant.db")).unwrap();
        let first = db.create_project("maintenance grant first").unwrap();
        let second = db.create_project("maintenance grant second").unwrap();
        (directory, db, first, second)
    }

    fn project_draft(project_id: i64) -> MemoryDraft {
        MemoryDraft {
            kind: MemoryKind::Project,
            scope: MemoryScope::Project { project_id },
            subject: "wrong-project-target".into(),
            content: "maintenance grant target".into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ControllerApproved,
                source_reference: Some("controller:maintenance-grant-unit-test".into()),
            },
            confidence: Some(0.8),
        }
    }

    #[test]
    fn wrong_proposal_project_is_rejected_before_authorization_and_budget_use() {
        let (_directory, db, current_project, other_project) = setup();
        let other_memories = crate::memory::MemoryService::new(&db, other_project);
        let target = other_memories
            .create(&project_draft(other_project))
            .unwrap();
        let proposal = ControllerMemoryMutationProposal::from_intent(
            other_project,
            ControllerMemoryMutationIntent::Remove { target: target.id },
            &other_memories,
        )
        .unwrap();
        let grant = ControllerMemoryMaintenanceGrant::new(current_project, 1).unwrap();
        let result = grant.inspect_and_authorize(
            current_project,
            &proposal,
            |_target| Ok(MemoryKind::Project),
            |exact_proposal| Ok(authorize(exact_proposal)),
        );
        assert!(matches!(
            result,
            Err(ControllerMemoryMaintenanceGrantError::ProposalProjectMismatch { .. })
        ));
        assert_eq!(grant.remaining_actions().unwrap(), 1);
    }
}
