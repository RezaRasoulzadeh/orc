//! Opaque, bounded continuation grants for routine Controller actions.
//!
//! A grant is trusted operator/application state, not Controller data.  It
//! carries no execution context and deliberately has no serde implementation.
//! Cloning a grant shares its state, so copying a value cannot reset the
//! finite budget or create a second authorization source.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use thiserror::Error;

use crate::controller_actions::{
    ControllerActionAuthorization, ControllerActionError, ControllerActionExecutionResult,
    ControllerActionIntent, ControllerActionKind, ControllerActionLegality,
    ControllerActionProposal,
};
use crate::operations::ProjectOperations;

const MAX_CONTROLLER_CONTINUATION_TASK_ID_BYTES: usize = 256;

/// The maximum number of routine actions one continuation grant can authorize.
/// This is a grant bound, independent of workflow transition and revision
/// limits.
pub const MAX_CONTROLLER_CONTINUATION_ACTIONS: usize = 128;

static NEXT_GRANT_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque identity for one in-process continuation grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ControllerContinuationGrantId(u64);

/// Explicit allowed-action set for M07-001.  `Accept` is intentionally not a
/// member of this type and cannot be represented by a grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerContinuationAllowedActions {
    dispatch: bool,
    semantic_review: bool,
    revise: bool,
}

impl ControllerContinuationAllowedActions {
    pub const fn routine() -> Self {
        Self {
            dispatch: true,
            semantic_review: true,
            revise: true,
        }
    }

    /// Build a grantable subset from the existing canonical action kinds.
    /// `Accept` and any future action kind are rejected rather than silently
    /// widening the continuation surface.
    pub fn from_actions<I>(actions: I) -> Result<Self, ControllerContinuationGrantError>
    where
        I: IntoIterator<Item = ControllerActionKind>,
    {
        let mut allowed = Self {
            dispatch: false,
            semantic_review: false,
            revise: false,
        };
        for action in actions {
            match action {
                ControllerActionKind::Dispatch => allowed.dispatch = true,
                ControllerActionKind::SemanticReview => allowed.semantic_review = true,
                ControllerActionKind::Revise => allowed.revise = true,
                ControllerActionKind::Accept => {
                    return Err(ControllerContinuationGrantError::AcceptNotGrantable);
                }
            }
        }
        if !allowed.any() {
            return Err(ControllerContinuationGrantError::EmptyAllowedActions);
        }
        Ok(allowed)
    }

    pub const fn allows(self, action: ControllerActionKind) -> bool {
        match action {
            ControllerActionKind::Dispatch => self.dispatch,
            ControllerActionKind::SemanticReview => self.semantic_review,
            ControllerActionKind::Revise => self.revise,
            ControllerActionKind::Accept => false,
        }
    }

    pub const fn includes_dispatch(self) -> bool {
        self.dispatch
    }

    pub const fn includes_semantic_review(self) -> bool {
        self.semantic_review
    }

    pub const fn includes_revise(self) -> bool {
        self.revise
    }

    const fn any(self) -> bool {
        self.dispatch || self.semantic_review || self.revise
    }
}

/// Observable lifecycle of a continuation grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerContinuationGrantState {
    Active,
    Exhausted,
    Revoked,
}

#[derive(Debug)]
struct GrantState {
    id: ControllerContinuationGrantId,
    project_id: i64,
    allowed_actions: ControllerContinuationAllowedActions,
    remaining_actions: usize,
    state: ControllerContinuationGrantState,
}

/// Trusted, bounded continuation permission.  This type is intentionally not
/// serializable and exposes no persistence, workflow, provider, or execution
/// handles.  Clones share the same state and therefore the same budget.
#[derive(Clone, Debug)]
pub struct ControllerContinuationGrant {
    state: Arc<Mutex<GrantState>>,
}

#[derive(Debug, Error)]
pub enum ControllerContinuationGrantError {
    #[error("continuation grant project binding must be positive")]
    InvalidProject,
    #[error("continuation grant must allow at least one routine action")]
    EmptyAllowedActions,
    #[error("Accept is not grantable by M07-001")]
    AcceptNotGrantable,
    #[error(
        "continuation action budget must be between 1 and {MAX_CONTROLLER_CONTINUATION_ACTIONS}"
    )]
    InvalidBudget,
    #[error("continuation grant is bound to project {expected}, not current project {actual}")]
    WrongProject { expected: i64, actual: i64 },
    #[error("continuation grant is exhausted")]
    Exhausted,
    #[error("continuation grant is revoked")]
    Revoked,
    #[error("continuation grant state is unavailable")]
    StateUnavailable,
    #[error("action {0:?} is not allowed by the continuation grant")]
    UnsupportedAction(ControllerActionKind),
    #[error("invalid Controller action intent: {0}")]
    InvalidIntent(#[source] ControllerActionError),
    #[error("current Controller action legality inspection failed: {0}")]
    LegalityInspection(#[source] ControllerActionError),
    #[error("current Controller action is not legal: {0:?}")]
    CanonicallyIllegal(Box<ControllerActionLegality>),
    #[error("Controller action authorization failed: {0}")]
    Authorization(#[source] ControllerActionError),
}

/// Bounded grant-stage rejection exposed by the one-step application seam.
/// Detailed kernel legality and storage diagnostics remain in the existing
/// grant/M03 boundaries rather than becoming a new result protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControllerContinuationStepGrantRejection {
    InvalidGrant,
    WrongProject,
    Exhausted,
    Revoked,
    StateUnavailable,
    UnsupportedAction { action: ControllerActionKind },
    InvalidIntent,
    LegalityInspection,
    CanonicallyIllegal,
    Authorization,
}

/// Bounded result of exactly one supervised Controller continuation attempt.
/// The existing proposal, intent, and M03 execution result remain the typed
/// payloads; this enum adds no parallel action or lifecycle schema.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ControllerContinuationStepResult {
    NoActionableProposal {
        task_id: String,
        proposal: ControllerActionProposal,
    },
    ProposalTaskMismatch {
        task_id: String,
        intent: ControllerActionIntent,
    },
    GrantRejected {
        task_id: String,
        intent: ControllerActionIntent,
        reason: ControllerContinuationStepGrantRejection,
    },
    Executed {
        task_id: String,
        intent: ControllerActionIntent,
        result: ControllerActionExecutionResult,
    },
    ExecutionRejected {
        task_id: String,
        intent: ControllerActionIntent,
        result: ControllerActionExecutionResult,
    },
}

impl ControllerContinuationGrant {
    pub(crate) fn new(
        project_id: i64,
        allowed_actions: ControllerContinuationAllowedActions,
        remaining_actions: usize,
    ) -> Result<Self, ControllerContinuationGrantError> {
        if project_id <= 0 {
            return Err(ControllerContinuationGrantError::InvalidProject);
        }
        if remaining_actions == 0 || remaining_actions > MAX_CONTROLLER_CONTINUATION_ACTIONS {
            return Err(ControllerContinuationGrantError::InvalidBudget);
        }
        if !allowed_actions.any() {
            return Err(ControllerContinuationGrantError::EmptyAllowedActions);
        }
        let id = ControllerContinuationGrantId(NEXT_GRANT_ID.fetch_add(1, Ordering::Relaxed));
        Ok(Self {
            state: Arc::new(Mutex::new(GrantState {
                id,
                project_id,
                allowed_actions,
                remaining_actions,
                state: ControllerContinuationGrantState::Active,
            })),
        })
    }

    pub fn id(&self) -> Result<ControllerContinuationGrantId, ControllerContinuationGrantError> {
        Ok(self.lock()?.id)
    }

    pub fn project_id(&self) -> Result<i64, ControllerContinuationGrantError> {
        Ok(self.lock()?.project_id)
    }

    pub fn allowed_actions(
        &self,
    ) -> Result<ControllerContinuationAllowedActions, ControllerContinuationGrantError> {
        Ok(self.lock()?.allowed_actions)
    }

    pub fn remaining_actions(&self) -> Result<usize, ControllerContinuationGrantError> {
        Ok(self.lock()?.remaining_actions)
    }

    pub fn state(&self) -> ControllerContinuationGrantState {
        self.state
            .lock()
            .map(|state| state.state)
            .unwrap_or(ControllerContinuationGrantState::Revoked)
    }

    /// Revoke this grant.  All clones observe the same terminal state.
    pub fn revoke(&self) -> Result<(), ControllerContinuationGrantError> {
        let mut state = self.lock()?;
        if state.state == ControllerContinuationGrantState::Exhausted {
            return Ok(());
        }
        state.state = ControllerContinuationGrantState::Revoked;
        Ok(())
    }

    pub(crate) fn inspect_and_authorize<F>(
        &self,
        current_project_id: i64,
        intent: &ControllerActionIntent,
        operations: &ProjectOperations<'_>,
        authorize: F,
    ) -> Result<ControllerActionAuthorization, ControllerContinuationGrantError>
    where
        F: FnOnce(
            &ControllerActionIntent,
        ) -> Result<ControllerActionAuthorization, ControllerActionError>,
    {
        let mut state = self.lock()?;
        if state.project_id != current_project_id {
            return Err(ControllerContinuationGrantError::WrongProject {
                expected: state.project_id,
                actual: current_project_id,
            });
        }
        match state.state {
            ControllerContinuationGrantState::Active => {}
            ControllerContinuationGrantState::Exhausted => {
                return Err(ControllerContinuationGrantError::Exhausted);
            }
            ControllerContinuationGrantState::Revoked => {
                return Err(ControllerContinuationGrantError::Revoked);
            }
        }

        let action = intent.action_kind();
        if !state.allowed_actions.allows(action) {
            return Err(ControllerContinuationGrantError::UnsupportedAction(action));
        }
        let legality = intent.inspect(operations).map_err(|error| {
            if matches!(error, ControllerActionError::InvalidTaskId) {
                ControllerContinuationGrantError::InvalidIntent(error)
            } else {
                ControllerContinuationGrantError::LegalityInspection(error)
            }
        })?;
        if matches!(legality, ControllerActionLegality::Rejected { .. }) {
            return Err(ControllerContinuationGrantError::CanonicallyIllegal(
                Box::new(legality),
            ));
        }

        // The authorization is created through the existing M03 constructor
        // while this grant's state is exclusively held.  The budget changes
        // only after that constructor succeeds, so failed inspection/minting
        // never consumes a unit.
        let authorization =
            authorize(intent).map_err(ControllerContinuationGrantError::Authorization)?;
        state.remaining_actions -= 1;
        if state.remaining_actions == 0 {
            state.state = ControllerContinuationGrantState::Exhausted;
        }
        Ok(authorization)
    }

    fn lock(&self) -> Result<MutexGuard<'_, GrantState>, ControllerContinuationGrantError> {
        self.state
            .lock()
            .map_err(|_| ControllerContinuationGrantError::StateUnavailable)
    }
}

impl ControllerContinuationStepGrantRejection {
    pub(crate) fn from_grant_error(error: &ControllerContinuationGrantError) -> Self {
        match error {
            ControllerContinuationGrantError::WrongProject { .. } => Self::WrongProject,
            ControllerContinuationGrantError::Exhausted => Self::Exhausted,
            ControllerContinuationGrantError::Revoked => Self::Revoked,
            ControllerContinuationGrantError::StateUnavailable => Self::StateUnavailable,
            ControllerContinuationGrantError::UnsupportedAction(action) => {
                Self::UnsupportedAction { action: *action }
            }
            ControllerContinuationGrantError::InvalidIntent(_) => Self::InvalidIntent,
            ControllerContinuationGrantError::LegalityInspection(_) => Self::LegalityInspection,
            ControllerContinuationGrantError::CanonicallyIllegal(_) => Self::CanonicallyIllegal,
            ControllerContinuationGrantError::Authorization(_) => Self::Authorization,
            ControllerContinuationGrantError::InvalidProject
            | ControllerContinuationGrantError::EmptyAllowedActions
            | ControllerContinuationGrantError::AcceptNotGrantable
            | ControllerContinuationGrantError::InvalidBudget => Self::InvalidGrant,
        }
    }
}

impl ControllerContinuationStepResult {
    pub(crate) fn task_id(task_id: &str) -> String {
        let mut end = task_id.len().min(MAX_CONTROLLER_CONTINUATION_TASK_ID_BYTES);
        while end > 0 && !task_id.is_char_boundary(end) {
            end -= 1;
        }
        task_id[..end].to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Database;
    use tempfile::tempdir;

    #[test]
    fn only_routine_actions_are_representable() {
        let all = ControllerContinuationAllowedActions::from_actions([
            ControllerActionKind::Dispatch,
            ControllerActionKind::SemanticReview,
            ControllerActionKind::Revise,
        ])
        .unwrap();
        assert_eq!(all, ControllerContinuationAllowedActions::routine());
        assert!(!all.allows(ControllerActionKind::Accept));
        assert!(matches!(
            ControllerContinuationAllowedActions::from_actions([ControllerActionKind::Accept]),
            Err(ControllerContinuationGrantError::AcceptNotGrantable)
        ));
    }

    #[test]
    fn bounds_and_shared_copy_state_are_deterministic() {
        let actions = ControllerContinuationAllowedActions::routine();
        assert!(matches!(
            ControllerContinuationGrant::new(0, actions, 1),
            Err(ControllerContinuationGrantError::InvalidProject)
        ));
        assert!(matches!(
            ControllerContinuationGrant::new(1, actions, 0),
            Err(ControllerContinuationGrantError::InvalidBudget)
        ));
        assert!(matches!(
            ControllerContinuationGrant::new(1, actions, MAX_CONTROLLER_CONTINUATION_ACTIONS + 1),
            Err(ControllerContinuationGrantError::InvalidBudget)
        ));

        let grant = ControllerContinuationGrant::new(1, actions, 1).unwrap();
        let copied = grant.clone();
        assert_eq!(grant.id().unwrap(), copied.id().unwrap());
        grant.revoke().unwrap();
        assert_eq!(copied.state(), ControllerContinuationGrantState::Revoked);
    }

    #[test]
    fn project_binding_rejects_stale_or_cross_project_use_without_consuming_budget() {
        let directory = tempdir().unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        let operations = ProjectOperations::new(&db, directory.path());
        let grant =
            ControllerContinuationGrant::new(1, ControllerContinuationAllowedActions::routine(), 2)
                .unwrap();
        let intent = ControllerActionIntent::SemanticReview {
            task_id: "T-0001".into(),
        };

        let result = grant.inspect_and_authorize(2, &intent, &operations, |_| unreachable!());
        assert!(matches!(
            result,
            Err(ControllerContinuationGrantError::WrongProject {
                expected: 1,
                actual: 2
            })
        ));
        assert_eq!(grant.remaining_actions().unwrap(), 2);
        assert_eq!(grant.state(), ControllerContinuationGrantState::Active);
    }
}
