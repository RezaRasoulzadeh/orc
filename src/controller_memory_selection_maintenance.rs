//! One-step composition of bounded memory-target selection and maintenance.
//!
//! This capability carries only the selected canonical memory identity from
//! the read-only selector into the existing maintenance boundary. It does not
//! add target selection, proposal, authorization, or mutation behavior.

use crate::controller_memory_maintenance::{
    ControllerMemoryMaintenanceMutationSuccess, ControllerMemoryMaintenanceStepError,
};
use crate::controller_memory_selection::ControllerMemorySelectionError;
use crate::memory::MemoryId;

/// The selected target that was kept by the existing maintenance judgment.
#[derive(Debug)]
pub struct ControllerMemorySelectionMaintenanceKept {
    target: MemoryId,
}

impl ControllerMemorySelectionMaintenanceKept {
    pub fn target(&self) -> &MemoryId {
        &self.target
    }

    pub(crate) fn from_selected(target: MemoryId) -> Self {
        Self { target }
    }
}

/// A successful canonical M06-009 mutation for the selected target.
#[derive(Debug)]
pub struct ControllerMemorySelectionMaintenanceMutationSuccess {
    target: MemoryId,
    result: ControllerMemoryMaintenanceMutationSuccess,
}

impl ControllerMemorySelectionMaintenanceMutationSuccess {
    pub fn target(&self) -> &MemoryId {
        &self.target
    }

    pub fn maintenance_result(&self) -> &ControllerMemoryMaintenanceMutationSuccess {
        &self.result
    }

    pub(crate) fn from_selected(
        target: MemoryId,
        result: ControllerMemoryMaintenanceMutationSuccess,
    ) -> Self {
        Self { target, result }
    }
}

/// A maintenance outcome paired with the exact target selected by this call.
/// Its private fields and crate-local constructor prevent callers from
/// manufacturing an unrelated target/outcome combination.
#[derive(Debug, thiserror::Error)]
#[error("selected target maintenance failed: {error}")]
pub struct ControllerMemorySelectionMaintenanceRejection {
    target: MemoryId,
    error: ControllerMemoryMaintenanceStepError,
}

impl ControllerMemorySelectionMaintenanceRejection {
    pub fn target(&self) -> &MemoryId {
        &self.target
    }

    pub fn maintenance_error(&self) -> &ControllerMemoryMaintenanceStepError {
        &self.error
    }

    pub(crate) fn from_selected(
        target: MemoryId,
        error: ControllerMemoryMaintenanceStepError,
    ) -> Self {
        Self { target, error }
    }
}

/// A failure before or after a valid selector result.
#[derive(Debug, thiserror::Error)]
pub enum ControllerMemorySelectionMaintenanceStepError {
    #[error("memory target selection failed: {0}")]
    Selection(#[source] ControllerMemorySelectionError),
    #[error(transparent)]
    Maintenance(ControllerMemorySelectionMaintenanceRejection),
}

/// Result of one bounded target-selection and maintenance composition.
#[derive(Debug)]
pub enum ControllerMemorySelectionMaintenanceStepResult {
    NoTarget,
    Kept {
        result: ControllerMemorySelectionMaintenanceKept,
    },
    Mutated {
        result: ControllerMemorySelectionMaintenanceMutationSuccess,
    },
    Rejected {
        error: ControllerMemorySelectionMaintenanceStepError,
    },
}
