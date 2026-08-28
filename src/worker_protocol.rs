//! Provider-independent Worker preparation and execution evidence.
//!
//! This module deliberately contains no provider or review policy.  It describes
//! the contract a worker must satisfy before it is allowed to execute.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const WORKER_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannedOperation {
    Inspect,
    Create,
    Modify,
    Delete,
    Move,
    Command,
    Validate,
    NoMutation,
}

/// Derive the declared intent of an expected-change entry.  The prefix is a
/// task-contract convention, not a provider command, so plans remain portable.
pub fn operation_for_expected_change(value: &str) -> PlannedOperation {
    let value = value.trim().to_ascii_lowercase();
    if value.starts_with("create:") {
        PlannedOperation::Create
    } else if value.starts_with("delete:") {
        PlannedOperation::Delete
    } else if value.starts_with("move:") {
        PlannedOperation::Move
    } else if value.starts_with("inspect:") {
        PlannedOperation::Inspect
    } else if value.starts_with("command:") {
        PlannedOperation::Command
    } else if value.starts_with("validate:") {
        PlannedOperation::Validate
    } else if value.starts_with("no-mutation") {
        PlannedOperation::NoMutation
    } else {
        PlannedOperation::Modify
    }
}

pub fn operation_name(operation: &PlannedOperation) -> &'static str {
    match operation {
        PlannedOperation::Inspect => "inspect",
        PlannedOperation::Create => "create",
        PlannedOperation::Modify => "modify",
        PlannedOperation::Delete => "delete",
        PlannedOperation::Move => "move",
        PlannedOperation::Command => "command",
        PlannedOperation::Validate => "validate",
        PlannedOperation::NoMutation => "no_mutation",
    }
}

pub fn reported_operations(output: &str) -> Vec<PlannedOperation> {
    parse_reported_operations(output).unwrap_or_default()
}

/// Parse the protocol operation lines without silently dropping malformed
/// declarations.  A provider may return arbitrary narrative, but once it
/// starts the protocol prefix every such line must name a supported operation.
pub fn parse_reported_operations(output: &str) -> anyhow::Result<Vec<PlannedOperation>> {
    let mut operations = Vec::new();
    for line in output.lines() {
        let Some(value) = line.trim().strip_prefix("OPERATION PERFORMED: ") else {
            continue;
        };
        let value = value.trim();
        let operation = [
            PlannedOperation::Inspect,
            PlannedOperation::Create,
            PlannedOperation::Modify,
            PlannedOperation::Delete,
            PlannedOperation::Move,
            PlannedOperation::Command,
            PlannedOperation::Validate,
            PlannedOperation::NoMutation,
        ]
        .into_iter()
        .find(|operation| operation_name(operation) == value)
        .ok_or_else(|| anyhow::anyhow!("unknown planned operation '{value}'"))?;
        operations.push(operation);
    }
    if operations.is_empty()
        && let Some(values) = serde_json::from_str::<serde_json::Value>(output)
            .ok()
            .and_then(|value| {
                value
                    .get("worker_protocol")
                    .and_then(|protocol| protocol.get("operations_performed"))
                    .cloned()
            })
            .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
    {
        for value in values {
            let operation = [
                PlannedOperation::Inspect,
                PlannedOperation::Create,
                PlannedOperation::Modify,
                PlannedOperation::Delete,
                PlannedOperation::Move,
                PlannedOperation::Command,
                PlannedOperation::Validate,
                PlannedOperation::NoMutation,
            ]
            .into_iter()
            .find(|operation| operation_name(operation) == value)
            .ok_or_else(|| anyhow::anyhow!("unknown planned operation '{value}'"))?;
            operations.push(operation);
        }
    }
    Ok(operations)
}

pub fn reported_verifications(output: &str) -> Vec<String> {
    let mut checks = output
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("VERIFICATION PASSED: ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    if checks.is_empty()
        && let Some(values) = serde_json::from_str::<serde_json::Value>(output)
            .ok()
            .and_then(|value| {
                value
                    .get("worker_protocol")
                    .and_then(|protocol| protocol.get("verification_passed"))
                    .cloned()
            })
            .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
    {
        checks = values;
    }
    checks
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedStep {
    pub id: String,
    pub intent: String,
    pub operations: Vec<PlannedOperation>,
    pub acceptance_criteria: Vec<String>,
    pub required_tests: Vec<String>,
    pub verification: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerPlan {
    pub protocol_version: u32,
    pub read_only_snapshot: String,
    pub unchanged: Vec<String>,
    pub steps: Vec<PlannedStep>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepEvidence {
    pub step_id: String,
    /// Observations produced while checking this step (command output, a
    /// worktree inspection, or another concrete result).  The plan's
    /// requested checks are not evidence by themselves.
    pub observed: Vec<String>,
    pub verification: Vec<String>,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerExecutionResult {
    pub protocol_version: u32,
    pub performed_operations: Vec<PlannedOperation>,
    pub affected_files: Vec<String>,
    pub requirement_coverage: Vec<(String, String)>,
    pub focused_verification: Vec<StepEvidence>,
    pub configured_validation: Vec<String>,
    pub unresolved_issues: Vec<String>,
}

impl WorkerPlan {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.protocol_version != WORKER_PROTOCOL_VERSION {
            anyhow::bail!(
                "unsupported worker protocol version {}",
                self.protocol_version
            );
        }
        if self.read_only_snapshot.trim().is_empty() {
            anyhow::bail!("PREPARE must record a read-only project snapshot");
        }
        if self.steps.is_empty() {
            anyhow::bail!("PREPARE must contain at least one ordered step");
        }
        let mut ids = BTreeSet::new();
        let mut criteria = BTreeSet::new();
        let mut tests = BTreeSet::new();
        let unchanged: BTreeSet<_> = self.unchanged.iter().map(|v| v.trim()).collect();
        for step in &self.steps {
            if step.id.trim().is_empty() || !ids.insert(step.id.clone()) {
                anyhow::bail!("PREPARE contains a duplicate or empty step id");
            }
            if step.intent.trim().is_empty()
                || step.operations.is_empty()
                || step.verification.is_empty()
            {
                anyhow::bail!(
                    "step '{}' is missing intent, operation, or verification",
                    step.id
                );
            }
            for value in &step.acceptance_criteria {
                criteria.insert(value.trim());
            }
            for value in &step.required_tests {
                tests.insert(value.trim());
            }
            if step
                .operations
                .iter()
                .any(|op| matches!(op, PlannedOperation::NoMutation))
                && step.operations.len() != 1
            {
                anyhow::bail!(
                    "NoMutation must be the only operation in step '{}'",
                    step.id
                );
            }
        }
        if criteria.iter().any(|v| v.is_empty()) || tests.iter().any(|v| v.is_empty()) {
            anyhow::bail!("PREPARE contains an empty requirement");
        }
        if criteria.iter().any(|v| unchanged.contains(v)) {
            anyhow::bail!("PREPARE contradicts an unchanged constraint");
        }
        if tests.is_empty() {
            anyhow::bail!("PREPARE omits required tests");
        }
        if criteria.is_empty() {
            anyhow::bail!("PREPARE omits acceptance criteria");
        }
        Ok(())
    }

    /// Validate the plan against the contract which was persisted when the task was created.
    pub fn validate_contract(
        &self,
        acceptance: &[String],
        tests: &[String],
        unchanged: &[String],
    ) -> anyhow::Result<()> {
        self.validate()?;
        let covered: BTreeSet<_> = self
            .steps
            .iter()
            .flat_map(|s| s.acceptance_criteria.iter().map(|v| v.trim()))
            .collect();
        let tested: BTreeSet<_> = self
            .steps
            .iter()
            .flat_map(|s| s.required_tests.iter().map(|v| v.trim()))
            .collect();
        for value in acceptance {
            if !covered.contains(value.trim()) {
                anyhow::bail!("PREPARE omits acceptance criterion '{}'", value);
            }
        }
        for value in tests {
            if !tested.contains(value.trim()) {
                anyhow::bail!("PREPARE omits required test '{}'", value);
            }
        }
        let unchanged: BTreeSet<_> = unchanged.iter().map(|v| v.trim()).collect();
        if self
            .steps
            .iter()
            .flat_map(|s| s.acceptance_criteria.iter())
            .any(|v| unchanged.contains(v.trim()))
        {
            anyhow::bail!("PREPARE contradicts an unchanged constraint");
        }
        Ok(())
    }
}

impl WorkerExecutionResult {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.protocol_version != WORKER_PROTOCOL_VERSION {
            anyhow::bail!("unsupported execution protocol version");
        }
        if self.focused_verification.iter().any(|e| !e.passed) {
            anyhow::bail!("a declared step verification failed");
        }
        for evidence in &self.focused_verification {
            if evidence.step_id.trim().is_empty()
                || evidence.observed.is_empty()
                || evidence.observed.iter().any(|v| v.trim().is_empty())
                || evidence.verification.is_empty()
                || evidence.verification.iter().any(|v| v.trim().is_empty())
            {
                anyhow::bail!(
                    "step evidence must identify a step and contain concrete verification"
                );
            }
            if !evidence.observed.iter().any(|value| {
                value.contains("worktree inspection") || value.contains("validation observed")
            }) {
                anyhow::bail!("step evidence must contain a post-step observation");
            }
            for check in &evidence.verification {
                if !evidence
                    .observed
                    .iter()
                    .any(|value| value == &format!("verification passed: {check}"))
                {
                    anyhow::bail!(
                        "step evidence does not independently observe verification '{}',",
                        check
                    );
                }
            }
        }
        if self.focused_verification.is_empty() {
            anyhow::bail!("execution must record focused verification evidence");
        }
        Ok(())
    }

    pub fn validate_against_plan(&self, plan: &WorkerPlan) -> anyhow::Result<()> {
        self.validate()?;
        let expected: Vec<_> = plan.steps.iter().map(|s| s.id.as_str()).collect();
        let actual: Vec<_> = self
            .focused_verification
            .iter()
            .map(|e| e.step_id.as_str())
            .collect();
        if expected != actual {
            anyhow::bail!("execution evidence does not cover the prepared steps");
        }
        let operations: Vec<_> = plan
            .steps
            .iter()
            .flat_map(|s| s.operations.clone())
            .collect();
        if self.performed_operations != operations {
            anyhow::bail!("performed operations do not match the prepared plan order");
        }
        let covered: BTreeSet<_> = self
            .requirement_coverage
            .iter()
            .map(|(r, _)| r.trim())
            .collect();
        for requirement in plan
            .steps
            .iter()
            .flat_map(|s| s.acceptance_criteria.iter().chain(s.required_tests.iter()))
        {
            if !covered.contains(requirement.trim()) {
                anyhow::bail!(
                    "execution evidence omits planned requirement '{}',",
                    requirement
                );
            }
        }
        if self
            .requirement_coverage
            .iter()
            .any(|(_, step)| !expected.contains(&step.as_str()))
        {
            anyhow::bail!("execution evidence contains an unknown step");
        }
        for (step, evidence) in plan.steps.iter().zip(&self.focused_verification) {
            if evidence.verification != step.verification {
                anyhow::bail!(
                    "verification evidence does not match step '{}'; it must be observed for the declared checks",
                    step.id
                );
            }
        }
        if self.requirement_coverage.iter().any(|(requirement, step)| {
            !plan.steps.iter().any(|candidate| {
                candidate.id == *step
                    && candidate
                        .acceptance_criteria
                        .iter()
                        .chain(candidate.required_tests.iter())
                        .any(|value| value == requirement)
            })
        }) {
            anyhow::bail!("execution evidence contains invalid requirement coverage");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan(op: PlannedOperation) -> WorkerPlan {
        WorkerPlan {
            protocol_version: 1,
            read_only_snapshot: "clean".into(),
            unchanged: vec![],
            steps: vec![PlannedStep {
                id: "s1".into(),
                intent: "do task".into(),
                operations: vec![op],
                acceptance_criteria: vec!["works".into()],
                required_tests: vec!["test".into()],
                verification: vec!["check".into()],
            }],
        }
    }
    #[test]
    fn valid_operations_are_accepted() {
        for op in [
            PlannedOperation::Create,
            PlannedOperation::Modify,
            PlannedOperation::Delete,
            PlannedOperation::Move,
            PlannedOperation::Inspect,
            PlannedOperation::Command,
            PlannedOperation::Validate,
            PlannedOperation::NoMutation,
        ] {
            assert!(plan(op).validate().is_ok());
        }
    }
    #[test]
    fn incomplete_and_contradictory_plans_are_rejected() {
        let mut p = plan(PlannedOperation::Inspect);
        p.steps[0].required_tests.clear();
        assert!(p.validate().is_err());
        let mut p = plan(PlannedOperation::Modify);
        p.unchanged = vec!["works".into()];
        assert!(p.validate().is_err());
    }
    #[test]
    fn failed_evidence_is_not_success() {
        let r = WorkerExecutionResult {
            protocol_version: 1,
            performed_operations: vec![],
            affected_files: vec![],
            requirement_coverage: vec![],
            focused_verification: vec![StepEvidence {
                step_id: "s".into(),
                observed: vec!["observed failure".into()],
                verification: vec!["no".into()],
                passed: false,
            }],
            configured_validation: vec![],
            unresolved_issues: vec![],
        };
        assert!(r.validate().is_err());
    }
}
