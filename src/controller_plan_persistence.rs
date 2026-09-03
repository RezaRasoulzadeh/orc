//! Trusted, supervised persistence of a validated Controller plan proposal.
//!
//! M05-001 produces a read-only [`ControllerPlanResult`]. This module keeps
//! that result separate from the trusted one-shot authorization required to
//! persist it through the M05-002 Controller-origin storage seam.

use crate::controller_planning::ControllerPlanResult;
use crate::protocol::PlanResponse;
use crate::storage::db::{PlanProvenance, PlanStatus};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_CONTROLLER_PLAN_PERSISTENCE_BYTES: usize = 64 * 1024;

/// Errors while deriving a persistence proposal from Controller output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ControllerPlanPersistenceProposalError {
    #[error("Controller plan project identity is invalid")]
    InvalidProject,
    #[error("Controller plan result is not valid")]
    InvalidControllerResult,
    #[error("Controller plan exceeds the persistence bound")]
    PlanTooLarge,
}

/// A typed persistence proposal derived from a validated M05-001 result.
///
/// The fields are private so callers cannot manufacture a proposal by
/// deserializing model-owned data or by supplying durable provenance. The
/// rationale and uncertainty from the Controller result are deliberately not
/// carried into the persistence boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerPlanPersistenceProposal {
    project_id: i64,
    workflow_id: Option<i64>,
    plan: PlanResponse,
}

impl ControllerPlanPersistenceProposal {
    pub fn from_controller_result(
        project_id: i64,
        result: &ControllerPlanResult,
    ) -> Result<Self, ControllerPlanPersistenceProposalError> {
        Self::from_controller_result_for_workflow(project_id, None, result)
    }

    pub fn from_controller_result_for_workflow(
        project_id: i64,
        workflow_id: Option<i64>,
        result: &ControllerPlanResult,
    ) -> Result<Self, ControllerPlanPersistenceProposalError> {
        if project_id <= 0 {
            return Err(ControllerPlanPersistenceProposalError::InvalidProject);
        }
        if workflow_id.is_some_and(|id| id <= 0) {
            return Err(ControllerPlanPersistenceProposalError::InvalidProject);
        }
        result
            .validate()
            .map_err(|_| ControllerPlanPersistenceProposalError::InvalidControllerResult)?;
        let proposal = Self {
            project_id,
            workflow_id,
            plan: result.plan.clone(),
        };
        proposal.validate()?;
        Ok(proposal)
    }

    pub const fn project_id(&self) -> i64 {
        self.project_id
    }

    pub const fn workflow_id(&self) -> Option<i64> {
        self.workflow_id
    }

    pub const fn plan(&self) -> &PlanResponse {
        &self.plan
    }

    /// Deterministic validation used both during proposal derivation and
    /// immediately before the canonical storage mutation.
    pub fn validate(&self) -> Result<(), ControllerPlanPersistenceProposalError> {
        if self.project_id <= 0 {
            return Err(ControllerPlanPersistenceProposalError::InvalidProject);
        }
        if self.workflow_id.is_some_and(|id| id <= 0) {
            return Err(ControllerPlanPersistenceProposalError::InvalidProject);
        }
        self.plan
            .validate()
            .map_err(|_| ControllerPlanPersistenceProposalError::InvalidControllerResult)?;
        let serialized = serde_json::to_vec(&self.plan)
            .map_err(|_| ControllerPlanPersistenceProposalError::InvalidControllerResult)?;
        if serialized.len() > MAX_CONTROLLER_PLAN_PERSISTENCE_BYTES {
            return Err(ControllerPlanPersistenceProposalError::PlanTooLarge);
        }
        Ok(())
    }
}

/// Trusted application authorization for one exact Controller plan proposal.
///
/// This type intentionally has no public constructor, `Clone`, or serde
/// implementation. Execution consumes it by value, and the private binding
/// compares both project identity and the complete canonical PlanResponse.
#[derive(Debug, PartialEq, Eq)]
pub struct ControllerPlanPersistenceAuthorization {
    project_id: i64,
    workflow_id: Option<i64>,
    plan: PlanResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanPersistenceAuthorizationRejection {
    Missing,
    NotAuthorizedForProposal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanPersistenceFailure {
    InvalidProposal,
    CanonicalStorage,
}

/// Bounded typed result of the trusted Controller Plan persistence boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanPersistenceResult {
    AuthorizationRejected {
        reason: ControllerPlanPersistenceAuthorizationRejection,
    },
    FreshValidationRejected,
    PersistenceFailed {
        reason: ControllerPlanPersistenceFailure,
    },
    Persisted {
        plan_id: i64,
        status: PlanStatus,
        provenance: PlanProvenance,
    },
}

impl ControllerPlanPersistenceResult {
    pub const fn persisted(plan_id: i64) -> Self {
        Self::Persisted {
            plan_id,
            status: PlanStatus::Proposed,
            provenance: PlanProvenance::controller(),
        }
    }
}

pub(crate) fn authorization_for(
    proposal: &ControllerPlanPersistenceProposal,
) -> ControllerPlanPersistenceAuthorization {
    ControllerPlanPersistenceAuthorization {
        project_id: proposal.project_id,
        workflow_id: proposal.workflow_id,
        plan: proposal.plan.clone(),
    }
}

pub(crate) fn matches_authorization(
    proposal: &ControllerPlanPersistenceProposal,
    authorization: &ControllerPlanPersistenceAuthorization,
) -> bool {
    authorization.project_id == proposal.project_id
        && authorization.workflow_id == proposal.workflow_id
        && authorization.plan == proposal.plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::OrcApp;
    use crate::lead::LeadDecisionKind;
    use crate::protocol::{PROTOCOL_VERSION, PlanResponse};
    use crate::storage::db::LeadDecisionMetadata;

    fn result(objective: &str) -> ControllerPlanResult {
        ControllerPlanResult {
            plan: PlanResponse {
                protocol_version: PROTOCOL_VERSION,
                objective: objective.into(),
                assumptions: vec![],
                risks: vec![],
                questions: vec![],
                tasks: vec![],
            },
            rationale: "bounded controller proposal".into(),
            uncertainty: None,
        }
    }

    fn app_with_pending_lead_decision() -> (tempfile::TempDir, OrcApp, i64, i64) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("orc.db");
        let database = crate::storage::Database::init(&path).unwrap();
        let project_id = database.create_project("controller persistence").unwrap();
        let decision_id = database
            .record_lead_decision(
                project_id,
                &LeadDecisionKind::PlanRequired,
                &serde_json::json!({"plan": "legacy decision remains pending"}),
                LeadDecisionMetadata {
                    snapshot: "snapshot",
                    run_id: None,
                    source_request: "request",
                    summary: "summary",
                },
            )
            .unwrap();
        drop(database);
        let app = OrcApp::open(&path, directory.path()).unwrap();
        (directory, app, project_id, decision_id)
    }

    #[test]
    fn valid_controller_result_derives_a_read_only_typed_proposal() {
        let controller_result = result("proposal");
        let proposal =
            ControllerPlanPersistenceProposal::from_controller_result(1, &controller_result)
                .unwrap();
        assert_eq!(proposal.project_id(), 1);
        assert_eq!(proposal.plan(), &controller_result.plan);
        assert!(
            serde_json::to_value(&controller_result)
                .unwrap()
                .get("authorization")
                .is_none()
        );
    }

    #[test]
    fn invalid_controller_result_is_rejected_without_persistence() {
        let (_directory, app, project_id, _decision_id) = app_with_pending_lead_decision();
        let mut invalid = result("valid");
        invalid.plan.protocol_version = PROTOCOL_VERSION + 1;
        assert!(matches!(
            ControllerPlanPersistenceProposal::from_controller_result(project_id, &invalid),
            Err(ControllerPlanPersistenceProposalError::InvalidControllerResult)
        ));
        assert!(
            app.database()
                .list_plan_history(project_id)
                .unwrap()
                .is_empty()
        );
        assert!(app.database().list_tasks().unwrap().is_empty());
    }

    #[test]
    fn missing_and_mismatched_authorization_cannot_persist() {
        let (_directory, app, project_id, _decision_id) = app_with_pending_lead_decision();
        let first =
            ControllerPlanPersistenceProposal::from_controller_result(project_id, &result("first"))
                .unwrap();
        let second = ControllerPlanPersistenceProposal::from_controller_result(
            project_id,
            &result("substituted"),
        )
        .unwrap();
        assert!(matches!(
            app.execute_authorized_controller_plan_persistence(&first, None),
            ControllerPlanPersistenceResult::AuthorizationRejected {
                reason: ControllerPlanPersistenceAuthorizationRejection::Missing
            }
        ));
        let authorization = app.authorize_controller_plan_persistence(&first);
        assert!(matches!(
            app.execute_authorized_controller_plan_persistence(&second, Some(authorization)),
            ControllerPlanPersistenceResult::AuthorizationRejected {
                reason: ControllerPlanPersistenceAuthorizationRejection::NotAuthorizedForProposal
            }
        ));
        let different_project = ControllerPlanPersistenceProposal::from_controller_result(
            project_id + 1,
            &result("different project"),
        )
        .unwrap();
        let authorization = app.authorize_controller_plan_persistence(&first);
        assert!(matches!(
            app.execute_authorized_controller_plan_persistence(
                &different_project,
                Some(authorization),
            ),
            ControllerPlanPersistenceResult::AuthorizationRejected {
                reason: ControllerPlanPersistenceAuthorizationRejection::NotAuthorizedForProposal
            }
        ));
        assert!(
            app.database()
                .list_plan_history(project_id)
                .unwrap()
                .is_empty()
        );
        assert!(app.database().list_tasks().unwrap().is_empty());
    }

    #[test]
    fn authorization_is_consumed_and_success_persists_exactly_one_controller_plan() {
        let (_directory, app, project_id, decision_id) = app_with_pending_lead_decision();
        let proposal = ControllerPlanPersistenceProposal::from_controller_result(
            project_id,
            &result("persist once"),
        )
        .unwrap();
        let authorization = app.authorize_controller_plan_persistence(&proposal);
        let persisted_id = match app
            .execute_authorized_controller_plan_persistence(&proposal, Some(authorization))
        {
            ControllerPlanPersistenceResult::Persisted {
                plan_id,
                status: PlanStatus::Proposed,
                provenance,
            } => {
                assert_eq!(provenance, PlanProvenance::controller());
                plan_id
            }
            other => panic!("unexpected persistence result: {other:?}"),
        };
        assert!(matches!(
            app.execute_authorized_controller_plan_persistence(&proposal, None),
            ControllerPlanPersistenceResult::AuthorizationRejected {
                reason: ControllerPlanPersistenceAuthorizationRejection::Missing
            }
        ));

        let database = app.database();
        let plan = database.get_plan(persisted_id).unwrap().unwrap();
        assert_eq!(plan.status, PlanStatus::Proposed);
        assert_eq!(plan.provenance, PlanProvenance::controller());
        assert_eq!(database.list_plan_history(project_id).unwrap().len(), 1);
        assert!(database.list_tasks().unwrap().is_empty());
        assert!(database.list_plan_reviews(project_id).unwrap().is_empty());
        assert_eq!(
            database
                .pending_lead_decision(project_id)
                .unwrap()
                .unwrap()
                .id,
            decision_id
        );
        assert!(
            database
                .list_agent_runs(project_id, usize::MAX)
                .unwrap()
                .is_empty()
        );
    }
}
