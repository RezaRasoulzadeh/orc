use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::thread;

use crate::backend::WorkerFactory;
use crate::contract;
use crate::git;
use crate::queue::QueueEntry;
use crate::registry::{self, AgentDefinition, ReasoningEffort};
use crate::review::DispatchSummary;
use crate::storage::Database;
use crate::task::{Task, TaskScopeMode, TaskStatus};
use crate::validation::{
    self, SystemValidationRunner, ValidationConfig, ValidationReport, ValidationRunner,
};
use crate::worker::{Worker, WorkerOutcome};

/// Evidence observations come from the worker output and post-execution
/// checks; declarations in PREPARE are never observations.
fn worker_observations(
    output: Option<&str>,
    validation: &str,
    worktree: &git::WorktreeChanges,
    verification: &[String],
    allow_aggregate_validation: bool,
) -> Vec<String> {
    // Provider output is retained on the run as reported evidence, but is not
    // promoted into Orc-observed evidence. Only independent worktree and
    // validation observations enter this collection.
    let mut observations = Vec::new();
    let files = worktree
        .files
        .iter()
        .map(|file| format!("{} {}", file.status, file.path))
        .collect::<Vec<_>>();
    let file_summary = if files.is_empty() {
        "clean".to_owned()
    } else {
        files.join(", ")
    };
    observations.push(format!(
        "post-step worktree inspection observed {} affected file(s): {}",
        files.len(),
        file_summary
    ));
    if !validation.trim().is_empty() {
        observations.push(format!("configured validation observed: {validation}"));
    }
    // A declaration in the plan is not evidence. A marker must be observed in
    // provider output; aggregate validation cannot prove a step-specific check.
    observations.extend(verification.iter().filter_map(|check| {
        (crate::worker_protocol::reported_verifications(output.unwrap_or_default())
            .iter()
            .any(|reported| reported == check)
            || (allow_aggregate_validation
                && check.trim() == "configured validation evidence"
                && validation.contains("PASS")))
        .then_some(format!("verification passed: {check}"))
    }));
    observations
}

fn performed_operations_for_step(
    step: &crate::worker_protocol::PlannedStep,
    output: Option<&str>,
    enforce_protocol: bool,
) -> Result<Vec<crate::worker_protocol::PlannedOperation>> {
    let output = output.unwrap_or_default();
    let reported = crate::worker_protocol::parse_reported_operations(output)?;
    if reported.is_empty() {
        if enforce_protocol {
            anyhow::bail!(
                "worker did not report the performed operation for step '{}'",
                step.id
            );
        }
        // Rows created before the canonical TaskProposal was persisted use the
        // original Worker seam. They remain executable, but never enter the
        // strict protocol path used by canonical task contracts.
        return Ok(step.operations.clone());
    }
    if reported != step.operations {
        anyhow::bail!("worker did not report the persisted step operations in order")
    }
    Ok(reported)
}

fn failed_execution_evidence(
    plan: &crate::worker_protocol::WorkerPlan,
    outputs: &[Option<String>],
    snapshots: &[(git::WorktreeChanges, git::WorktreeChanges)],
    configured_validation: &[String],
    issue: &str,
    enforce_protocol: bool,
) -> crate::worker_protocol::WorkerExecutionResult {
    let performed_operations = plan
        .steps
        .iter()
        .enumerate()
        .flat_map(|(index, step)| {
            let output = outputs
                .get(index)
                .and_then(Option::as_deref)
                .unwrap_or_default();
            let reported =
                crate::worker_protocol::parse_reported_operations(output).unwrap_or_default();
            if reported.is_empty() && !enforce_protocol {
                step.operations.clone()
            } else {
                reported
            }
        })
        .collect();
    let focused_verification = plan
        .steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| {
            let (_before, after) = snapshots.get(index)?;
            let output = outputs.get(index).and_then(Option::as_deref);
            Some(crate::worker_protocol::StepEvidence {
                step_id: step.id.clone(),
                observed: worker_observations(output, "", after, &step.verification, false),
                verification: step.verification.clone(),
                passed: false,
            })
        })
        .collect();
    crate::worker_protocol::WorkerExecutionResult {
        protocol_version: crate::worker_protocol::WORKER_PROTOCOL_VERSION,
        performed_operations,
        affected_files: snapshots
            .last()
            .map(|(_, after)| after.files.iter().map(|file| file.path.clone()).collect())
            .unwrap_or_default(),
        requirement_coverage: requirement_coverage(plan),
        focused_verification,
        configured_validation: configured_validation.to_vec(),
        unresolved_issues: vec![issue.to_owned()],
    }
}

fn requirement_coverage(plan: &crate::worker_protocol::WorkerPlan) -> Vec<(String, String)> {
    plan.steps
        .iter()
        .flat_map(|step| {
            step.acceptance_criteria
                .iter()
                .chain(step.required_tests.iter())
                .chain(step.active_review_blockers.iter())
                .map(move |id| (id.clone(), step.id.clone()))
        })
        .chain(
            plan.plan_acceptance_criteria
                .iter()
                .chain(plan.plan_required_tests.iter())
                .chain(plan.plan_review_blockers.iter())
                .map(|id| (id.clone(), "plan".to_owned())),
        )
        .collect()
}

/// Build one semantic implementation checkpoint from the authoritative task
/// contract. `expected_changes` describes scope; it is not an execution plan
/// and must never multiply provider calls or fabricate checkpoint boundaries.
fn plan_steps(
    proposal: &crate::protocol::TaskProposal,
    acceptance_criteria: &[crate::worker_protocol::WorkerRequirement],
    required_tests: &[crate::worker_protocol::WorkerRequirement],
    active_review_blockers: &[crate::worker_protocol::ReviewBlockerRequirement],
    verification: &[String],
    intent: &str,
) -> Vec<crate::worker_protocol::PlannedStep> {
    let entries = if proposal.expected_changes.is_empty() {
        let capabilities = crate::registry::normalize_capability_names(&proposal.capabilities);
        let implementation =
            proposal.role == "developer" && capabilities.iter().any(|value| value == "code");
        if implementation {
            let bounded_targets = if proposal.context_files.is_empty() {
                vec!["task-scoped repository files".to_owned()]
            } else {
                proposal.context_files.clone()
            };
            let mut entries = bounded_targets
                .iter()
                .map(|target| format!("inspect: {target}"))
                .chain(
                    bounded_targets
                        .iter()
                        .map(|target| format!("modify: {target}")),
                )
                .collect::<Vec<_>>();
            if capabilities
                .iter()
                .any(|value| value == "command_execution")
            {
                entries.push("command: task-required implementation commands".to_owned());
                entries.push("validate: persisted task requirements".to_owned());
            }
            entries
        } else {
            vec!["no-mutation".to_owned()]
        }
    } else {
        proposal.expected_changes.clone()
    };
    let (operations, operation_targets) = entries
        .into_iter()
        .map(|entry| {
            let operation = crate::worker_protocol::operation_for_expected_change(&entry);
            let target = entry.split_once(':').map_or_else(
                || {
                    if entry.eq_ignore_ascii_case("no-mutation") {
                        "worktree".to_owned()
                    } else {
                        entry.trim().to_owned()
                    }
                },
                |(_, target)| target.trim().to_owned(),
            );
            (operation, target)
        })
        .unzip();
    vec![crate::worker_protocol::PlannedStep {
        id: "implementation".into(),
        objective: intent.to_owned(),
        intent: intent.to_owned(),
        operations,
        operation_targets,
        acceptance_criteria: acceptance_criteria
            .iter()
            .map(|item| item.id.clone())
            .collect(),
        required_tests: required_tests.iter().map(|item| item.id.clone()).collect(),
        active_review_blockers: active_review_blockers
            .iter()
            .map(|item| item.id.clone())
            .collect(),
        verification: verification.to_vec(),
    }]
}

fn worker_requirements(
    values: &[String],
    prefix: &str,
) -> Vec<crate::worker_protocol::WorkerRequirement> {
    values
        .iter()
        .enumerate()
        .map(|(index, text)| crate::worker_protocol::WorkerRequirement {
            id: format!("{prefix}-{}", index + 1),
            text: text.clone(),
        })
        .collect()
}

fn blocker_requirements(
    blockers: &[crate::storage::db::ReviewBlockerRecord],
) -> Vec<crate::worker_protocol::ReviewBlockerRequirement> {
    blockers
        .iter()
        .map(|blocker| crate::worker_protocol::ReviewBlockerRequirement {
            id: blocker.blocker_id.clone(),
            text: blocker.acceptance_condition.clone(),
        })
        .collect()
}

fn performed_operations_for_plan(
    steps: &[crate::worker_protocol::PlannedStep],
    outputs: &[Option<String>],
    enforce_protocol: bool,
) -> Result<Vec<crate::worker_protocol::PlannedOperation>> {
    steps
        .iter()
        .enumerate()
        .try_fold(Vec::new(), |mut all, (index, step)| {
            all.extend(performed_operations_for_step(
                step,
                outputs.get(index).and_then(Option::as_deref),
                enforce_protocol,
            )?);
            Ok(all)
        })
}

const ENGINEERING_CONTRACT_PATH: &str = ".orc/engineering.md";
const ARCHITECTURE_DECISION_MARKER: &str = "ORC-ARCHITECTURE-DECISION:";
const MAX_COMPLETION_REPAIRS: usize = 2;

fn start_provider_invocation_bounded(
    db: &Database,
    run_id: i64,
    task_id: &str,
    purpose: &str,
    attempt: usize,
    effort: ReasoningEffort,
) -> Result<i64> {
    match db.start_provider_invocation(run_id, purpose, attempt, Some(effort)) {
        Ok(id) => Ok(id),
        Err(error) => {
            let diagnostics =
                format!("provider budget prevented {purpose} attempt {attempt}: {error}");
            db.update_agent_run_status(run_id, "failed", Some(&diagnostics))?;
            db.update_task_status(task_id, TaskStatus::Blocked)?;
            anyhow::bail!(diagnostics)
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct RevisionExecutionOverrides {
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
}
const CODER_PROMPT_PRECEDENCE: &str = "## Instruction precedence\n\n1. Orc execution and safety rules have the highest precedence.\n2. The `.orc/engineering.md` content below is the authoritative, mandatory project engineering contract and applies automatically; it does not need to be repeated in the task or user prompt.\n3. Role- and action-specific instructions follow the engineering contract.\n4. Task objectives and context, revision feedback, validation diagnostics, and all other run-specific instructions follow the engineering contract.\n\nLater task, revision, or repair text must not override or contradict mandatory requirements in `.orc/engineering.md`. If task-specific instructions conflict with the engineering contract, follow the engineering contract and report the conflict rather than silently overriding it.\n";

fn task_contract_effort(db: &Database, task: &crate::task::Task) -> Result<ReasoningEffort> {
    let persisted = db
        .get_task(&task.id)?
        .context("task disappeared while resolving its execution contract")?;
    let effort = persisted
        .reasoning_effort
        .context("persisted task contract has no execution effort")?;
    if effort == ReasoningEffort::None {
        bail!("persisted task contract has invalid execution effort 'none'")
    }
    let effort_reason = persisted
        .effort_reason
        .as_deref()
        .context("persisted task contract has no execution-effort reason")?;
    if effort_reason.trim().is_empty() || effort_reason.chars().count() > 240 {
        bail!("persisted task contract has an invalid execution-effort reason")
    }
    Ok(effort)
}

/// Build the execution contract from the authoritative persisted Task row.
/// Proposal metadata is intentionally not consulted here: it is only retained
/// as provenance for how a Task was created.
fn worker_task_contract(
    db: &Database,
    task: &Task,
) -> Result<(crate::protocol::TaskProposal, bool)> {
    let effort = task_contract_effort(db, task)?;
    let task_contract = db
        .get_task_contract(&task.id)?
        .unwrap_or_else(|| crate::task::TaskContract::defaults(&task.objective));
    let execution_hints = db
        .get_task_execution_hints(&task.id)?
        .context("task disappeared while loading execution hints")?;
    let proposal = crate::protocol::TaskProposal {
        local_id: task.id.clone(),
        title: task.title.clone(),
        objective: task.objective.clone(),
        role: task.role.clone(),
        priority: task.priority,
        depends_on: vec![],
        capabilities: task.required_capabilities(),
        scope_mode: task.scope_mode,
        context_files: task.context_files.clone(),
        expected_changes: task.expected_changes.clone(),
        unchanged: task_contract.unchanged,
        acceptance_criteria: task_contract.acceptance_criteria,
        required_tests: task_contract.required_tests,
        validation: task_contract.validation,
        execution_hints: crate::protocol::ExecutionHints {
            effort: Some(effort.as_str().to_owned()),
            ..execution_hints
        },
        risk_factors: task.risk_factors.clone(),
    };
    let strict_protocol = !task.expected_changes.is_empty();
    // Existing manually-created tasks retain their original worker seam. A
    // task with declared expected changes has the complete persisted contract
    // and enters the strict operation/evidence protocol.
    Ok((proposal, strict_protocol))
}

fn apply_task_effort(
    mut resolution: crate::execution::ExecutionResolution,
    task_effort: Option<ReasoningEffort>,
) -> crate::execution::ExecutionResolution {
    if let Some(effort) = task_effort {
        resolution.reasoning_effort = Some(effort);
        resolution.source = "task-contract".into();
    }
    resolution
}

fn effective_revision_effort(
    db: &Database,
    task: &crate::task::Task,
    source_review_id: i64,
    explicit_override: Option<ReasoningEffort>,
) -> Result<ReasoningEffort> {
    let base = task_contract_effort(db, task)?;
    let blockers = db
        .review_blocker_observations(source_review_id)?
        .into_iter()
        .filter(|blocker| blocker.status != "resolved")
        .collect::<Vec<_>>();
    let mut effective = base;
    for blocker in blockers {
        if let Some(previous) = db.completed_revision_effort_for_blocker(
            &task.id,
            source_review_id,
            &blocker.blocker_id,
        )? {
            if previous == ReasoningEffort::High {
                let details = serde_json::json!({
                    "blocker_id": blocker.blocker_id,
                    "source_review_id": source_review_id,
                    "previous_effort": previous.as_str(),
                    "condition": "same substantive blocker survived a completed high-effort revision"
                });
                db.set_task_execution_condition(
                    &task.id,
                    "non_convergence_replan_required",
                    &details.to_string(),
                )?;
                bail!(
                    "REPLAN_REQUIRED: task '{}' has a blocker that survived a completed high-effort revision",
                    task.id
                );
            }
            let escalated = previous.next();
            if escalated.rank() > effective.rank() {
                effective = escalated;
            }
        }
    }
    let selected = explicit_override.unwrap_or(effective);
    Ok(if selected.rank() < effective.rank() {
        effective
    } else {
        selected
    })
}

fn validate_worker_step_completion(
    step: &crate::worker_protocol::PlannedStep,
    snapshot: Option<&(git::WorktreeChanges, git::WorktreeChanges)>,
    output: Option<&str>,
    enforce_protocol: bool,
    unchanged: &[String],
) -> Result<()> {
    let Some((_before, after)) = snapshot else {
        anyhow::bail!("Worker did not execute persisted step '{}'", step.id);
    };
    performed_operations_for_step(step, output, enforce_protocol)?;
    if enforce_protocol {
        let affected = crate::worker_protocol::reported_affected_files(output.unwrap_or_default());
        for (operation, target) in step.operations.iter().zip(&step.operation_targets) {
            if matches!(
                operation,
                crate::worker_protocol::PlannedOperation::Create
                    | crate::worker_protocol::PlannedOperation::Modify
                    | crate::worker_protocol::PlannedOperation::Delete
                    | crate::worker_protocol::PlannedOperation::Move
            ) && !affected
                .iter()
                .any(|path| path == target || target.ends_with(path))
            {
                anyhow::bail!(
                    "Worker did not attribute affected file '{}' to step '{}'",
                    target,
                    step.id
                );
            }
            let effect_target = target
                .split_once("->")
                .map_or(target.as_str(), |(_, value)| value.trim());
            if matches!(
                operation,
                crate::worker_protocol::PlannedOperation::Create
                    | crate::worker_protocol::PlannedOperation::Modify
                    | crate::worker_protocol::PlannedOperation::Move
            ) && !after.files.iter().any(|file| file.path == effect_target)
            {
                anyhow::bail!(
                    "Worker checkpoint '{}' has no matching worktree effect for '{}'",
                    step.id,
                    effect_target
                );
            }
        }
        validate_unchanged_constraints(after, unchanged)?;
        let reported = crate::worker_protocol::reported_verifications(output.unwrap_or_default());
        for check in &step.verification {
            if !reported.iter().any(|value| value == check) {
                anyhow::bail!(
                    "Worker did not report verification '{}' for step '{}'",
                    check,
                    step.id
                );
            }
        }
    }
    Ok(())
}

fn completion_repair_prompt(
    diff: &str,
    step: &crate::worker_protocol::PlannedStep,
    failure: &str,
    attempt: usize,
) -> String {
    format!(
        "WORKER COMPLETION SELF-CHECK REPAIR (attempt {attempt} of {MAX_COMPLETION_REPAIRS}). Repair only the exact failed checkpoint below. Preserve the existing worktree and unrelated changes.\n\nEXACT FAILURE:\n{failure}\n\nCURRENT DIFF:\n{diff}\n\nPERSISTED STEP AND NECESSARY CONSTRAINTS:\n{}\n\nAfter inspecting the worktree, emit the required operation, `AFFECTED FILE: <path>`, and verification protocol lines. Do not claim a check from the plan alone.",
        serde_json::to_string_pretty(step).unwrap_or_else(|_| step.id.clone())
    )
}

fn validate_unchanged_constraints(
    changes: &git::WorktreeChanges,
    unchanged: &[String],
) -> Result<()> {
    let changed_paths = changes
        .files
        .iter()
        .flat_map(|file| {
            file.path
                .split_once(" -> ")
                .map(|(source, destination)| vec![source, destination])
                .unwrap_or_else(|| vec![file.path.as_str()])
        })
        .collect::<HashSet<_>>();
    if let Some(constraint) = unchanged
        .iter()
        .map(|value| value.trim())
        .find(|value| changed_paths.contains(value))
    {
        anyhow::bail!("Worker changed an unchanged path '{constraint}'");
    }
    Ok(())
}

fn architecture_decisions(output: &str) -> Vec<&str> {
    let mut decisions = Vec::new();
    let mut reported = HashSet::new();
    for line in output.lines() {
        let Some(decision) = line.strip_prefix(ARCHITECTURE_DECISION_MARKER) else {
            continue;
        };
        let decision = decision.trim();
        if !decision.is_empty() && reported.insert(decision) {
            decisions.push(decision);
        }
    }
    decisions
}

fn block_automated_run(db: &Database, run_id: i64, task_id: &str, output: &str) -> Result<()> {
    db.update_agent_run_status(run_id, "failed", Some(output))
        .context("failed to update agent run status to failed")?;
    db.update_task_status(task_id, TaskStatus::Blocked)
        .context("failed to set task status to blocked")?;
    Ok(())
}

fn build_worker_prompt(contract: &str, project: &str, task: &Task) -> String {
    let guidance = match task.scope_mode {
        Some(TaskScopeMode::Focused) => format!("\n\n## Targeted Context\n\nRelevant implementation context has already been identified.\n\nRead these files first:\n{}\n\nExpected changes:\n{}\n\nDo not perform broad repository discovery unless this scope proves insufficient. If additional files are necessary, inspect only the minimum required and report which ones were needed.", task.context_files.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"), task.expected_changes.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")),
        Some(TaskScopeMode::Module) => format!("\n\n## Targeted Context\n\nSupplied modules/directories:\n{}\nConstrain discovery to these areas where practical.\n\nExpected changes:\n{}", task.context_files.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"), task.expected_changes.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")),
        Some(TaskScopeMode::Project) => "\n\n## Targeted Context\n\nProject scope is configured; broader repository inspection is allowed.".into(),
        None => String::new(),
    };
    let objective = format!("{}{}", task.objective, guidance);
    let execution_contract = task
        .reasoning_effort
        .map(|effort| {
            format!(
                "\n\n## Execution contract\n\nReasoning effort: {}\nEffort reason: {}\nRisk factors: {}\n",
                effort.as_str(),
                task.effort_reason.as_deref().unwrap_or("not recorded"),
                if task.risk_factors.is_empty() {
                    "none".to_owned()
                } else {
                    format!("{:?}", task.risk_factors)
                }
            )
        })
        .unwrap_or_default();
    format!(
        "# Orc Coder Instructions\n\n{precedence}\n\n## Engineering Contract\n\n{contract}\n\n---\n\n# Task\n\nProject: {project}\nTask ID: {id}\nTitle: {title}\nObjective: {objective}\nRole: {role}{execution_contract}\n\nInspect the repository rooted at the current working directory and implement ONLY the changes required to complete this single task. Stay within the specified scope; do not modify unrelated files or change task status. Do not run the project's validation/test suite, focused checks, or any other command to prove completion \u{2014} automated review owns validation and will run the task-specific checks it needs after this session ends. Stop as soon as the implementation is complete and summarize what you changed and any follow-up steps.\n",
        precedence = CODER_PROMPT_PRECEDENCE,
        contract = contract,
        project = project,
        id = task.id,
        title = task.title,
        objective = objective,
        role = task.role,
        execution_contract = execution_contract,
    )
}

/// Whether the formatted revision contract already carries this feedback
/// text, so the revision prompt does not repeat the same review feedback
/// under two separate headings.
fn contract_already_contains_feedback(
    contract: &crate::automated::RevisionContract,
    feedback: &str,
) -> bool {
    contract
        .reviewer_revision_feedback
        .iter()
        .any(|recorded| recorded.trim() == feedback.trim())
}

pub fn build_manual_packet(contract: &str, project: &str, task: &Task, agent_id: &str) -> String {
    let guidance = match task.scope_mode {
        Some(TaskScopeMode::Focused) => format!(
            "\n\nRelevant implementation context has already been identified. Read these files first:\n{}\nExpected changes:\n{}\nDo not perform broad repository discovery unless this scope proves insufficient.",
            task.context_files
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n"),
            task.expected_changes
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        Some(TaskScopeMode::Module) => {
            "\n\nConstrain discovery to the supplied modules/directories where practical.".into()
        }
        Some(TaskScopeMode::Project) => {
            "\n\nProject scope allows broader repository inspection.".into()
        }
        None => String::new(),
    };
    let objective = format!("{}{}", task.objective, guidance);
    let execution_contract = task
        .reasoning_effort
        .map(|effort| {
            format!(
                "\n\n## Execution contract\n\nReasoning effort: {}\nEffort reason: {}\nRisk factors: {}\n",
                effort.as_str(),
                task.effort_reason.as_deref().unwrap_or("not recorded"),
                if task.risk_factors.is_empty() {
                    "none".to_owned()
                } else {
                    format!("{:?}", task.risk_factors)
                }
            )
        })
        .unwrap_or_default();
    format!(
        "# Orc Manual Task Packet\n\nAgent ID: {agent_id}\nProject: {project}\n\n{precedence}\n\n## Engineering Contract\n\n{contract}\n\n## Task\n\nTask ID: {id}\nTitle: {title}\nObjective: {objective}\nRole: {role}{execution_contract}\n\n## Constraints\n\nStay strictly inside this task's scope. Do not modify unrelated project work or assume access to credentials, private memory, or external systems.\n\n## Required validation\n\nDescribe the checks and tests you performed. If you could not run a check, say why.\n\n## Required response / handoff format\n\nSummarize changes or recommendations, list files affected (if any), report validation results, and identify follow-up risks or questions.\n",
        precedence = CODER_PROMPT_PRECEDENCE,
        id = task.id,
        title = task.title,
        objective = objective,
        role = task.role,
        execution_contract = execution_contract
    )
}

/// Build a worker prompt with engineering contract and task information.
pub fn build_worker_prompt_for_testing(contract: &str, project: &str, task: &Task) -> String {
    build_worker_prompt(contract, project, task)
}

/// Dispatch a task for execution using the provided worker and custom DB path.
/// This is the internal implementation that handles the full lifecycle.
/// For testing purposes, accepts a custom db_path parameter.
pub fn dispatch_with_worker_and_db(
    task_id: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
) -> Result<()> {
    dispatch_with_worker_and_db_as(task_id, worker, db_path, repo_path, "copilot").map(|_| ())
}

pub fn dispatch_with_worker_and_db_as(
    task_id: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
) -> Result<()> {
    dispatch_with_worker_and_db_as_with_runner(
        task_id,
        worker,
        db_path,
        repo_path,
        agent_id,
        &SystemValidationRunner,
    )
    .map(|_| ())
}

pub fn dispatch_with_worker_and_db_as_with_runner(
    task_id: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
) -> Result<DispatchSummary> {
    let db = Database::open(db_path)
        .with_context(|| format!("failed to open orc DB ({}); run `orc init` first", db_path))?;
    dispatch_with_worker_on_db(task_id, worker, &db, repo_path, agent_id, validation_runner)
}

pub fn dispatch_with_worker_on_db(
    task_id: &str,
    worker: &dyn Worker,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
) -> Result<DispatchSummary> {
    dispatch_with_worker_on_db_cancellable(
        task_id,
        worker,
        db,
        repo_path,
        agent_id,
        validation_runner,
        None,
    )
}

pub fn dispatch_with_worker_on_db_cancellable(
    task_id: &str,
    worker: &dyn Worker,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    // Configured project validation is no longer run by dispatch; it is
    // owned by automated review. The parameter is retained for API
    // compatibility with existing callers.
    _validation_runner: &dyn ValidationRunner,
    cancellation: Option<&crate::worker::CancellationControl>,
) -> Result<DispatchSummary> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;

    let project_id = db
        .get_project_id()
        .with_context(|| "failed to read project id from DB")?
        .with_context(|| "no project found in DB")?;

    let project = db
        .get_project_name()
        .with_context(|| "failed to read project name from DB")?;
    let project_name = project.unwrap_or_else(|| "orc".into());

    let task = db
        .get_task(task_id)
        .with_context(|| format!("failed to fetch task '{}' from DB", task_id))?
        .with_context(|| format!("task '{}' not found in DB", task_id))?;

    // Check if task is already active or done
    if task.status == TaskStatus::Active {
        anyhow::bail!("Task {} is already active; cannot dispatch again", task_id);
    }
    if task.status == TaskStatus::Done {
        anyhow::bail!("Task {} is already done; cannot dispatch", task_id);
    }
    if task.status == TaskStatus::Cancelled {
        anyhow::bail!("Task {} is cancelled; cannot dispatch", task_id);
    }
    // Injecting a worker only controls execution after the task has passed the
    // same queue eligibility policy used by every other dispatch path.
    crate::queue::ensure_dispatchable(db, task_id)
        .map_err(|e| anyhow::anyhow!("dispatch eligibility check failed: {e}"))?;

    // PREPARE is intentionally completed before task status, run, or worktree
    // mutation.  The snapshot is captured from the authoritative repository.
    let snapshot = git::inspect_worktree(repo_path, repo_path)
        .context("failed to inspect repository during Worker PREPARE")?;
    let (proposal, enforce_worker_protocol) =
        worker_task_contract(db, &task).context("persisted task contract is invalid")?;
    let proposal_effort = task_contract_effort(db, &task)?;
    let acceptance_criteria =
        worker_requirements(&proposal.acceptance_criteria, "acceptance-criterion");
    let required_tests = worker_requirements(&proposal.required_tests, "required-test");
    // Configured project validation is owned by automated review, not the
    // implementation session; no validation checkpoints are demanded here.
    let verification = Vec::new();
    let plan = crate::worker_protocol::WorkerPlan {
        protocol_version: crate::worker_protocol::WORKER_PROTOCOL_VERSION,
        read_only_snapshot: serde_json::to_string(&snapshot)
            .context("failed to serialize Worker PREPARE snapshot")?,
        unchanged: proposal.unchanged.clone(),
        acceptance_criteria: acceptance_criteria.clone(),
        required_tests: required_tests.clone(),
        active_review_blockers: Vec::new(),
        resolved_review_blockers: Vec::new(),
        verification: verification.clone(),
        plan_acceptance_criteria: Vec::new(),
        plan_required_tests: Vec::new(),
        plan_review_blockers: Vec::new(),
        steps: plan_steps(
            &proposal,
            &acceptance_criteria,
            &required_tests,
            &[],
            &verification,
            &task.objective,
        ),
    };
    plan.validate_contract(&acceptance_criteria, &required_tests, &proposal.unchanged)
        .context("Worker PREPARE plan is incomplete")?;

    // Set task status to active
    db.update_task_status(task_id, TaskStatus::Active)
        .with_context(|| "failed to set task status to active")?;

    // Create an agent run
    let run_id = db
        .create_agent_run_with_execution(
            project_id,
            task_id,
            agent_id,
            registry::AUTOMATED,
            crate::storage::AgentRunExecution {
                class: "general",
                model: None,
                effort: Some(proposal_effort),
                source: "task-contract",
            },
        )
        .with_context(|| "failed to create agent run")?;
    let _run_finalizer = db.run_finalizer(run_id);
    db.store_worker_prepare(run_id, &plan)
        .context("failed to persist Worker PREPARE plan")?;
    // Execution must consume the database record, not the in-memory plan that
    // happened to be used to write it. This makes restart/inspection semantics
    // identical to the execution semantics.
    let plan = db
        .load_worker_protocol(run_id)
        .context("failed to reopen persisted Worker PREPARE plan")?
        .context("persisted Worker PREPARE plan disappeared")?
        .0;
    db.record_lifecycle_event(
        "worker_prepare",
        Some(task_id),
        Some(run_id),
        Some(agent_id),
        Some(&serde_json::to_string(&plan)?),
    )?;

    // Create a worktree for the task
    let (branch_name, worktree_path) = match git::ensure_worktree(task_id, repo_path) {
        Ok((branch, path)) => (branch, path),
        Err(e) => {
            let error_msg = format!("Failed to create worktree: {}", e);
            block_automated_run(db, run_id, task_id, &error_msg)
                .context("failed to record worktree creation failure")?;
            anyhow::bail!("{}", error_msg);
        }
    };
    let progress = |phase: &str| {
        if let Err(error) = db.update_agent_run_phase(run_id, phase) {
            eprintln!("warning: failed to persist run progress: {error}");
        }
        println!("[orc] {phase}");
    };
    let worker_output = |line: &str| {
        if let Err(error) = db.record_worker_output(run_id, line) {
            eprintln!("warning: failed to persist worker output: {error}");
        }
        println!("[orc] worker output: {line}");
    };
    progress("worktree prepared");

    // Store worktree metadata
    if let Err(e) = db.store_worktree_metadata(
        run_id,
        task_id,
        &branch_name,
        &worktree_path.to_string_lossy(),
    ) {
        let error_msg = format!("Failed to store worktree metadata: {}", e);
        block_automated_run(db, run_id, task_id, &error_msg)
            .context("failed to record worktree metadata failure")?;
        anyhow::bail!("{}", error_msg);
    }

    let prompt = format!(
        "{}\n\nWORKER EXECUTION PROTOCOL (mandatory):\nExecute the persisted PREPARE plan in the exact order below. Do not perform any operation outside a listed step. After each step, perform and report its declared verification before continuing. For every operation, emit exactly `OPERATION PERFORMED: <inspect|create|modify|delete|move|command|validate|no_mutation>` in the planned order. Emit `VERIFICATION PASSED: <check>` only after actually observing that check.\n{}",
        build_worker_prompt(&contract, &project_name, &task),
        serde_json::to_string_pretty(&plan).context("failed to serialize execution plan")?
    );

    // Execute the worker in the worktree directory
    let worktree_dir = repo_path.join(&worktree_path);
    let before_plan = git::inspect_worktree(&worktree_dir, repo_path)
        .context("failed to inspect worktree before Worker plan")?;
    progress("worker spawned");
    progress("worker running");
    let invocation_id = start_provider_invocation_bounded(
        db,
        run_id,
        task_id,
        "implementation",
        1,
        proposal_effort,
    )?;
    let execution = match cancellation {
        Some(cancellation) => worker.execute_structured_with_progress_and_usage_cancellable(
            &prompt,
            &worktree_dir,
            &crate::worker_protocol::plan_completion_schema(),
            &|line| worker_output(line),
            cancellation,
        ),
        None => worker.execute_structured_with_progress_and_usage(
            &prompt,
            &worktree_dir,
            &crate::worker_protocol::plan_completion_schema(),
            &|line| worker_output(line),
        ),
    };
    db.finish_provider_invocation(
        invocation_id,
        if cancellation.is_some_and(crate::worker::CancellationControl::is_cancelled) {
            "cancelled"
        } else if execution.is_ok() {
            "completed"
        } else {
            "failed"
        },
        execution.as_ref().ok().and_then(|value| value.token_usage),
    )?;
    let after_plan = git::inspect_worktree(&worktree_dir, repo_path)
        .context("failed to inspect worktree after Worker plan")?;
    // The provider session owns the checkpoint boundaries. Orc verifies the
    // resulting aggregate worktree, but does not invent per-step snapshots.
    let mut step_snapshots = vec![(before_plan, after_plan)];
    let mut step_outputs = vec![None; plan.steps.len()];
    if let Ok(result) = &execution
        && let Some(full_output) = result.output.as_deref()
        && let Ok(completion) = crate::worker_protocol::parse_plan_completion(full_output)
    {
        for reported in completion.step_results {
            if let Some(index) = plan
                .steps
                .iter()
                .position(|step| step.id == reported.step_id)
            {
                let mut evidence = reported.observed.join("\n");
                for operation in reported.operations_performed {
                    evidence.push_str(&format!(
                        "\nOPERATION PERFORMED: {}",
                        crate::worker_protocol::operation_name(&operation)
                    ));
                }
                for path in reported.affected_files {
                    evidence.push_str(&format!("\nAFFECTED FILE: {path}"));
                }
                for check in reported.verification_passed {
                    evidence.push_str(&format!("\nVERIFICATION PASSED: {check}"));
                }
                step_outputs[index] = Some(evidence);
            }
        }
    }
    match execution {
        Ok(execution) => {
            let outcome = execution.outcome;
            let mut output = execution.output;
            let mut token_usage = execution.token_usage;
            match outcome {
                WorkerOutcome::Success => {
                    progress("worker completed");
                    let changes;
                    if enforce_worker_protocol {
                        let mut completion_repair = 0;
                        loop {
                            let failed_step =
                                plan.steps.iter().enumerate().find_map(|(index, step)| {
                                    validate_worker_step_completion(
                                        step,
                                        step_snapshots.get(index),
                                        step_outputs.get(index).and_then(Option::as_deref),
                                        true,
                                        &plan.unchanged,
                                    )
                                    .err()
                                    .map(|error| (index, error))
                                });
                            let Some((index, gate_error)) = failed_step else {
                                break;
                            };
                            if completion_repair >= MAX_COMPLETION_REPAIRS {
                                let evidence = failed_execution_evidence(
                                    &plan,
                                    &step_outputs,
                                    &step_snapshots,
                                    &[],
                                    &gate_error.to_string(),
                                    true,
                                );
                                db.store_worker_execution(run_id, &evidence)?;
                                let message = format!(
                                    "Worker completion self-check failed (verification/evidence): {gate_error:#}"
                                );
                                db.update_agent_run_status_with_usage(
                                    run_id,
                                    "failed",
                                    Some(&message),
                                    token_usage,
                                )?;
                                db.update_task_status(task_id, TaskStatus::Blocked)?;
                                anyhow::bail!(message);
                            }
                            completion_repair += 1;
                            let repair_diff = git::inspect_worktree(&worktree_dir, repo_path)?.diff;
                            let repair_prompt = completion_repair_prompt(
                                &repair_diff,
                                &plan.steps[index],
                                &gate_error.to_string(),
                                completion_repair,
                            );
                            db.record_lifecycle_event(
                                "worker_completion_repair_started",
                                Some(task_id),
                                Some(run_id),
                                Some(agent_id),
                                Some(
                                    &serde_json::json!({
                                        "attempt": completion_repair,
                                        "step_id": plan.steps[index].id,
                                        "failure": gate_error.to_string(),
                                    })
                                    .to_string(),
                                ),
                            )?;
                            let before = step_snapshots
                                .get(index)
                                .map(|(before, _)| before.clone())
                                .context("completion repair lost the step snapshot")?;
                            progress(&format!("completion repair attempt {completion_repair}"));
                            let repair_invocation = start_provider_invocation_bounded(
                                db,
                                run_id,
                                task_id,
                                "completion_repair",
                                completion_repair,
                                ReasoningEffort::Low,
                            )?;
                            let repaired = worker.execute_planned_step_repair(
                                &plan.steps[index],
                                &repair_prompt,
                                &worktree_dir,
                                &crate::automated::revision_handoff_schema(),
                                &|line| worker_output(line),
                                cancellation,
                            );
                            db.finish_provider_invocation(
                                repair_invocation,
                                if repaired.is_ok() {
                                    "completed"
                                } else {
                                    "failed"
                                },
                                repaired.as_ref().ok().and_then(|value| value.token_usage),
                            )?;
                            let repaired = match repaired {
                                Ok(value) => value,
                                Err(error) => {
                                    let message = format!(
                                        "Worker completion repair failed after self-check '{gate_error}': {error}"
                                    );
                                    db.update_agent_run_status_with_usage(
                                        run_id,
                                        "failed",
                                        Some(&message),
                                        token_usage,
                                    )?;
                                    db.update_task_status(task_id, TaskStatus::Blocked)?;
                                    anyhow::bail!(message);
                                }
                            };
                            if let WorkerOutcome::Failure(error) = repaired.outcome {
                                let message = format!(
                                    "Worker completion repair failed after self-check '{gate_error}': {error}"
                                );
                                db.update_agent_run_status_with_usage(
                                    run_id,
                                    "failed",
                                    Some(&message),
                                    token_usage,
                                )?;
                                db.update_task_status(task_id, TaskStatus::Blocked)?;
                                anyhow::bail!(message);
                            }
                            if let Some(repair_output) = repaired.output {
                                step_outputs[index] = Some(repair_output);
                            }
                            if repaired.token_usage.is_some() {
                                token_usage = repaired.token_usage;
                            }
                            let after = git::inspect_worktree(&worktree_dir, repo_path)
                                .context("failed to inspect worktree after completion repair")?;
                            step_snapshots[index] = (before, after);
                            db.record_lifecycle_event(
                                "worker_completion_repair_completed",
                                Some(task_id),
                                Some(run_id),
                                Some(agent_id),
                                Some(
                                    &serde_json::json!({
                                        "attempt": completion_repair,
                                        "step_id": plan.steps[index].id,
                                    })
                                    .to_string(),
                                ),
                            )?;
                        }
                        output = (!step_outputs.is_empty()).then(|| {
                            step_outputs
                                .iter()
                                .filter_map(|value| value.as_deref())
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        });
                    }
                    changes = match git::inspect_worktree(&worktree_dir, repo_path) {
                        Ok(changes) => changes,
                        Err(error) => {
                            let output = format!(
                                "{}\n\nPost-worker inspection failed: {error:#}",
                                output.as_deref().unwrap_or_default()
                            );
                            db.update_agent_run_status_with_usage(
                                run_id,
                                "failed",
                                Some(&output),
                                token_usage,
                            )?;
                            db.update_task_status(task_id, TaskStatus::Blocked)?;
                            anyhow::bail!(
                                "could not inspect task worktree after Worker completion gate"
                            )
                        }
                    };
                    if changes.files.is_empty() {
                        let output = format!(
                            "{}\n\nDispatch result: no meaningful project changes.",
                            output.as_deref().unwrap_or_default()
                        );
                        let explicitly_no_mutation = if enforce_worker_protocol {
                            plan.steps.iter().all(|step| {
                                step.operations.iter().all(|operation| {
                                    matches!(
                                        operation,
                                        crate::worker_protocol::PlannedOperation::NoMutation
                                            | crate::worker_protocol::PlannedOperation::Inspect
                                            | crate::worker_protocol::PlannedOperation::Command
                                            | crate::worker_protocol::PlannedOperation::Validate
                                    )
                                })
                            })
                        } else {
                            proposal.expected_changes.iter().any(|value| {
                                value.trim().eq_ignore_ascii_case("no-mutation")
                                    || value
                                        .trim()
                                        .to_ascii_lowercase()
                                        .starts_with("no-mutation:")
                            })
                        };
                        if !explicitly_no_mutation {
                            db.update_agent_run_status_with_usage(
                                run_id,
                                "no_changes",
                                Some(&output),
                                token_usage,
                            )?;
                            db.update_task_status(task_id, TaskStatus::Blocked)?;
                            anyhow::bail!(
                                "worker completed without meaningful project changes; task remains blocked"
                            );
                        }
                    }
                    db.store_change_evidence(run_id, &changes)?;
                    // Configured project validation is owned by automated review, not
                    // dispatch. Dispatch publishes implementation/change evidence and
                    // transitions straight into review.
                    let combined_output = output.clone().unwrap_or_default();
                    for decision in architecture_decisions(&combined_output) {
                        db.insert_approval_request(project_id, decision)
                            .with_context(
                                || "failed to record architecture decision approval request",
                            )?;
                    }
                    let performed_operations = performed_operations_for_plan(
                        &plan.steps,
                        &step_outputs,
                        enforce_worker_protocol,
                    )
                    .context("Worker operation evidence failed")?;
                    let evidence = crate::worker_protocol::WorkerExecutionResult {
                        protocol_version: crate::worker_protocol::WORKER_PROTOCOL_VERSION,
                        performed_operations,
                        affected_files: changes
                            .files
                            .iter()
                            .map(|file| file.path.clone())
                            .collect(),
                        requirement_coverage: requirement_coverage(&plan),
                        focused_verification: plan
                            .steps
                            .iter()
                            .enumerate()
                            .map(|(index, step)| {
                                let step_output =
                                    step_outputs.get(index).and_then(Option::as_deref);
                                crate::worker_protocol::StepEvidence {
                                    step_id: step.id.clone(),
                                    observed: worker_observations(
                                        step_output,
                                        "",
                                        step_snapshots
                                            .get(index)
                                            .map(|(_, after)| after)
                                            .unwrap_or(&changes),
                                        &step.verification,
                                        !enforce_worker_protocol,
                                    ),
                                    verification: step.verification.clone(),
                                    passed: step.verification.iter().all(|check| {
                                        crate::worker_protocol::reported_verifications(
                                            step_output.unwrap_or_default(),
                                        )
                                        .iter()
                                        .any(|reported| reported == check)
                                    }),
                                }
                            })
                            .collect(),
                        configured_validation: Vec::new(),
                        unresolved_issues: Vec::new(),
                    };
                    if let Err(error) = evidence.validate_against_plan(&plan) {
                        db.store_worker_execution(run_id, &evidence)?;
                        let message = format!("Worker verification evidence failed: {error:#}");
                        db.update_agent_run_status_with_usage(
                            run_id,
                            "failed",
                            Some(&message),
                            token_usage,
                        )?;
                        db.update_task_status(task_id, TaskStatus::Blocked)?;
                        anyhow::bail!(message);
                    }
                    db.record_lifecycle_event(
                        "worker_completion_gate",
                        Some(task_id),
                        Some(run_id),
                        Some(agent_id),
                        Some(
                            &serde_json::json!({
                                "status": "passed",
                                "contract": "authoritative task contract",
                                "evidence": evidence,
                            })
                            .to_string(),
                        ),
                    )?;
                    db.store_worker_execution(run_id, &evidence)
                        .context("failed to persist Worker execution evidence")?;
                    db.record_lifecycle_event(
                        "worker_execution_evidence",
                        Some(task_id),
                        Some(run_id),
                        Some(agent_id),
                        Some(&serde_json::to_string(&evidence)?),
                    )?;
                    db.complete_agent_run_for_review(
                        task_id,
                        run_id,
                        &combined_output,
                        token_usage,
                    )
                    .with_context(|| "failed to complete agent run and publish task for review")?;
                    progress("review transition");
                    let task = db
                        .get_task(task_id)?
                        .context("task disappeared after dispatch")?;
                    Ok(DispatchSummary {
                        task,
                        agent: agent_id.to_owned(),
                        backend: "unknown".to_owned(),
                        profile: None,
                        model: None,
                        reasoning_effort: Some(proposal_effort),
                        worktree_path: worktree_path.display().to_string(),
                        run_id,
                        run_status: "completed".to_owned(),
                        validation: "deferred to review".to_owned(),
                        changes,
                    })
                }
                WorkerOutcome::Failure(error) => {
                    // Mark agent run as failed and task as blocked
                    let error_msg = format!("Worker failed: {}", error);
                    db.update_agent_run_status_with_usage(
                        run_id,
                        "failed",
                        Some(&error_msg),
                        token_usage,
                    )
                    .with_context(|| "failed to update agent run status to failed")?;
                    db.update_task_status(task_id, TaskStatus::Blocked)
                        .with_context(|| "failed to set task status to blocked")?;
                    anyhow::bail!("{}", error_msg);
                }
            }
        }
        Err(spawn_error) => {
            if cancellation.is_some_and(crate::worker::CancellationControl::is_cancelled) {
                db.update_agent_run_status(
                    run_id,
                    "cancelled",
                    Some("execution cancelled at a safe boundary"),
                )?;
                db.update_task_status(task_id, TaskStatus::Blocked)?;
                anyhow::bail!("execution cancelled at a safe boundary");
            }
            // Spawn failed, mark run as failed and task as blocked
            db.update_agent_run_status(run_id, "failed", Some(&spawn_error))
                .with_context(|| "failed to update agent run status after spawn failure")?;
            db.update_task_status(task_id, TaskStatus::Blocked)
                .with_context(|| "failed to set task status to blocked after spawn failure")?;
            anyhow::bail!("{}", spawn_error);
        }
    }
}

/// Dispatch a task for execution using the provided worker.
/// Uses the default DB path (.orc/orc.db).
pub fn dispatch_with_worker(task_id: &str, worker: &dyn Worker) -> Result<()> {
    dispatch_with_worker_and_db(task_id, worker, ".orc/orc.db", ".")
}

pub fn revise_with_worker_and_db_as_with_runner(
    task_id: &str,
    feedback: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
) -> Result<DispatchSummary> {
    revise_with_worker_and_db_as_with_runner_with_overrides(
        task_id,
        feedback,
        worker,
        db_path,
        repo_path,
        agent_id,
        validation_runner,
        &RevisionExecutionOverrides::default(),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps the CLI revision seam explicit"
)]
pub fn revise_with_factory_and_db_as_with_runner<F>(
    task_id: &str,
    feedback: &str,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
    overrides: &RevisionExecutionOverrides,
    factory: F,
) -> Result<DispatchSummary>
where
    F: FnOnce(
        &AgentDefinition,
        Option<String>,
        Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String>,
{
    let db = Database::open(db_path)?;
    revise_with_factory_on_db_as_with_runner(
        task_id,
        feedback,
        &db,
        repo_path,
        agent_id,
        validation_runner,
        overrides,
        factory,
    )
}

/// Operator-facing revision entry point backed by the authoritative global
/// agent registry. The existing path-based helper remains isolated for tests
/// and embedders which explicitly own their registry path.
#[expect(
    clippy::too_many_arguments,
    reason = "keeps the CLI revision seam explicit"
)]
pub fn revise_with_factory_and_global_db_as_with_runner<F>(
    task_id: &str,
    feedback: &str,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
    overrides: &RevisionExecutionOverrides,
    factory: F,
) -> Result<DispatchSummary>
where
    F: FnOnce(
        &AgentDefinition,
        Option<String>,
        Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String>,
{
    let db = Database::open_global(db_path)?;
    revise_with_factory_on_db_as_with_runner(
        task_id,
        feedback,
        &db,
        repo_path,
        agent_id,
        validation_runner,
        overrides,
        factory,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared revision resolution boundary"
)]
fn revise_with_factory_on_db_as_with_runner<F>(
    task_id: &str,
    feedback: &str,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
    overrides: &RevisionExecutionOverrides,
    factory: F,
) -> Result<DispatchSummary>
where
    F: FnOnce(
        &AgentDefinition,
        Option<String>,
        Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String>,
{
    let task = db.get_task(task_id)?.context("task not found")?;
    let agent = db
        .list_schedulable_agents()?
        .into_iter()
        .find(|candidate| candidate.id == agent_id)
        .with_context(|| format!("agent '{}' not found in registry", agent_id))?;
    let source_review_id = db
        .actionable_revision_review(task_id)?
        .map(|(id, _)| id)
        .context("task has no actionable revision review")?;
    let revision_effort = effective_revision_effort(db, &task, source_review_id, overrides.effort)?;
    let task_hints = db
        .get_task_execution_hints(&task.id)?
        .context("task execution hints are missing")?;
    let execution_class = task_hints
        .class
        .as_deref()
        .map(crate::execution::ExecutionClass::parse)
        .transpose()
        .map_err(anyhow::Error::msg)?
        .unwrap_or_else(|| crate::execution::class_for_role(&task.role));
    let resolution = crate::execution::resolve_with_template(
        execution_class.as_str(),
        &db.execution_template(execution_class)?,
        agent.model.as_deref(),
        agent.reasoning_effort,
        overrides.model.clone().or_else(|| task_hints.model.clone()),
        overrides.effort,
    );
    let resolution = if overrides.effort == Some(revision_effort) {
        resolution
    } else {
        apply_task_effort(resolution, Some(revision_effort))
    };
    let worker = factory(
        &agent,
        resolution.model.clone(),
        resolution.reasoning_effort,
    )
    .map_err(anyhow::Error::msg)?;
    revise_with_worker_on_db_with_overrides(
        task_id,
        feedback,
        worker.as_ref(),
        db,
        repo_path,
        agent_id,
        validation_runner,
        overrides,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps the revision helper parallel to the existing worker API"
)]
pub fn revise_with_worker_and_db_as_with_runner_with_overrides(
    task_id: &str,
    feedback: &str,
    worker: &dyn Worker,
    db_path: &str,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
    overrides: &RevisionExecutionOverrides,
) -> Result<DispatchSummary> {
    let db = Database::open(db_path)?;
    revise_with_worker_on_db_with_overrides(
        task_id,
        feedback,
        worker,
        &db,
        repo_path,
        agent_id,
        validation_runner,
        overrides,
    )
}

pub fn revise_with_worker_on_db(
    task_id: &str,
    feedback: &str,
    worker: &dyn Worker,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
) -> Result<DispatchSummary> {
    revise_with_worker_on_db_with_overrides(
        task_id,
        feedback,
        worker,
        db,
        repo_path,
        agent_id,
        validation_runner,
        &RevisionExecutionOverrides::default(),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps the revision helper parallel to the existing worker API"
)]
pub fn revise_with_worker_on_db_with_overrides(
    task_id: &str,
    feedback: &str,
    worker: &dyn Worker,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    // Configured project validation is no longer run by revision; it is
    // owned by automated review. The parameter is retained for API
    // compatibility with existing callers.
    _validation_runner: &dyn ValidationRunner,
    overrides: &RevisionExecutionOverrides,
) -> Result<DispatchSummary> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    let project_name = db.get_project_name()?.unwrap_or_else(|| "orc".into());
    let task = db.get_task(task_id)?.context("task not found in DB")?;
    if matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled) {
        anyhow::bail!(
            "task {} cannot be revised from terminal status {}",
            task_id,
            task.status
        );
    }
    let Some((source_review_id, _source_feedback)) = db.actionable_revision_review(task_id)? else {
        anyhow::bail!(
            "task {} has no actionable REVISE review (currently {}); the prior review may already have been consumed by a completed revision. Run `orc review {} --automated` to publish a fresh review before revising again",
            task_id,
            task.status,
            task_id
        );
    };
    if let Some(condition) = db.get_task_execution_condition(task_id)? {
        bail!(
            "{}: task '{}' cannot enter another revision ({})",
            condition.kind,
            task_id,
            condition.details
        );
    }
    let (source_review_id, contract_id, revision_contract) =
        if let Some((source, json, id)) = db.actionable_revision_contract(task_id)? {
            (
                source,
                Some(id),
                serde_json::from_str(&json).context("persisted revision contract is invalid")?,
            )
        } else {
            (
                source_review_id,
                None,
                crate::automated::build_revision_contract_from_db(
                    db,
                    task_id,
                    &crate::review::build_review(db, task_id, repo_path)?.prior_reviews,
                    source_review_id,
                )?,
            )
        };
    let (_, worktree_path) = db
        .get_worktree_metadata(task_id)?
        .context("task has no worktree")?;
    let worktree_dir = repo_path.join(&worktree_path);
    if !worktree_dir.exists() {
        anyhow::bail!("task worktree does not exist: {}", worktree_dir.display());
    }
    let revision_snapshot = git::inspect_worktree(&worktree_dir, repo_path)
        .context("failed to inspect revision worktree during Worker PREPARE")?;
    let (proposal, enforce_worker_protocol) =
        worker_task_contract(db, &task).context("persisted task contract is invalid")?;
    let active_blockers = if revision_contract.active_blockers.is_empty() {
        revision_contract
            .unresolved
            .iter()
            .chain(&revision_contract.regressions)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        revision_contract.active_blockers.clone()
    };
    let resolved_blockers = if revision_contract.resolved_blockers.is_empty() {
        revision_contract.regression_constraints.clone()
    } else {
        revision_contract.resolved_blockers.clone()
    };
    let original_acceptance = if revision_contract
        .original_task_requirements
        .acceptance_criteria
        .is_empty()
    {
        proposal.acceptance_criteria.clone()
    } else {
        revision_contract
            .original_task_requirements
            .acceptance_criteria
            .clone()
    };
    let original_tests = if revision_contract
        .original_task_requirements
        .required_tests
        .is_empty()
    {
        proposal.required_tests.clone()
    } else {
        revision_contract
            .original_task_requirements
            .required_tests
            .clone()
    };
    // Initial implementation and revision share validation ownership: Orc
    // executes the complete configured gate after the single provider call.
    let verification = Vec::new();
    let acceptance_criteria = worker_requirements(&original_acceptance, "acceptance-criterion");
    let required_tests = worker_requirements(&original_tests, "required-test");
    let active_review_blockers = blocker_requirements(&active_blockers);
    let revision_plan = crate::worker_protocol::WorkerPlan {
        protocol_version: crate::worker_protocol::WORKER_PROTOCOL_VERSION,
        read_only_snapshot: serde_json::to_string(&revision_snapshot)?,
        unchanged: proposal.unchanged.clone(),
        acceptance_criteria: acceptance_criteria.clone(),
        required_tests: required_tests.clone(),
        active_review_blockers: active_review_blockers.clone(),
        resolved_review_blockers: resolved_blockers
            .iter()
            .map(|blocker| blocker.blocker_id.clone())
            .collect(),
        verification: verification.clone(),
        plan_acceptance_criteria: Vec::new(),
        plan_required_tests: Vec::new(),
        plan_review_blockers: Vec::new(),
        steps: plan_steps(
            &proposal,
            &acceptance_criteria,
            &required_tests,
            &active_review_blockers,
            &verification,
            "Address the persisted revision requirements",
        ),
    };
    revision_plan
        .validate_contract(&acceptance_criteria, &required_tests, &proposal.unchanged)
        .context("Worker revision PREPARE is incomplete")?;
    let revision_effort = effective_revision_effort(db, &task, source_review_id, overrides.effort)?;
    let agent = db
        .list_schedulable_agents()?
        .into_iter()
        .find(|agent| agent.id == agent_id)
        .with_context(|| format!("agent '{}' not found in registry", agent_id))?;
    let task_hints = db
        .get_task_execution_hints(&task.id)?
        .context("task execution hints are missing")?;
    let execution_class = task_hints
        .class
        .as_deref()
        .map(crate::execution::ExecutionClass::parse)
        .transpose()
        .map_err(anyhow::Error::msg)?
        .unwrap_or_else(|| crate::execution::class_for_role(&task.role));
    let resolution = crate::execution::resolve_with_template(
        execution_class.as_str(),
        &db.execution_template(execution_class)?,
        agent.model.as_deref(),
        agent.reasoning_effort,
        overrides.model.clone().or_else(|| task_hints.model.clone()),
        overrides.effort,
    );
    let resolution = if overrides.effort == Some(revision_effort) {
        resolution
    } else {
        apply_task_effort(resolution, Some(revision_effort))
    };
    let run_id = db.create_agent_run_with_execution(
        project_id,
        task_id,
        agent_id,
        registry::AUTOMATED,
        crate::storage::AgentRunExecution {
            class: resolution.class.as_str(),
            model: resolution.model.as_deref(),
            effort: resolution.reasoning_effort,
            source: &resolution.source,
        },
    )?;
    let _run_finalizer = db.run_finalizer(run_id);
    db.store_worker_prepare(run_id, &revision_plan)
        .context("failed to persist Worker revision PREPARE")?;
    let revision_plan = db
        .load_worker_protocol(run_id)
        .context("failed to reopen persisted Worker revision PREPARE plan")?
        .context("persisted Worker revision PREPARE plan disappeared")?
        .0;
    db.update_task_status(task_id, TaskStatus::Active)?;
    db.record_lifecycle_event(
        "review_revision",
        Some(task_id),
        Some(run_id),
        Some(agent_id),
        Some(feedback),
    )?;
    let progress = |phase: &str| {
        if let Err(error) = db.update_agent_run_phase(run_id, phase) {
            eprintln!("warning: failed to persist run progress: {error}");
        }
        println!("[orc] {phase}");
    };
    let worker_output = |line: &str| {
        if let Err(error) = db.record_worker_output(run_id, line) {
            eprintln!("warning: failed to persist worker output: {error}");
        }
        println!("[orc] worker output: {line}");
    };
    progress("revision/worker starting");
    // The revision contract already carries the source review's feedback
    // once (see format_revision_contract). Only append `feedback` again when
    // it adds information the contract does not already contain, e.g. an
    // operator-supplied override distinct from the persisted review.
    let extra_feedback = (!feedback.trim().is_empty()
        && !contract_already_contains_feedback(&revision_contract, feedback))
    .then(|| format!("\n\n## Additional operator feedback\n\n{feedback}"))
    .unwrap_or_default();
    let prompt = format!(
        "{}\n\n## Selected revision effort\n\nReasoning effort: {}\n\n{}{}\n\nFix ONLY the active blockers listed above, using the supplied blocker and relevant-file context. Do not broadly rediscover the repository and do not run the project's validation/test suite \u{2014} automated review will validate the result. Stop as soon as the fixes are implemented.",
        build_worker_prompt(&contract, &project_name, &task),
        revision_effort.as_str(),
        crate::automated::format_revision_contract(&revision_contract),
        extra_feedback,
    );
    let prompt = format!(
        "{}\n\nWORKER EXECUTION PROTOCOL (mandatory): execute the persisted revision PREPARE plan in exact order and verify each step before continuing.\n{}",
        prompt,
        serde_json::to_string_pretty(&revision_plan)?
    );
    let fail = |message: String| -> Result<DispatchSummary> {
        progress(if message.to_ascii_lowercase().contains("timeout") {
            "worker timeout"
        } else {
            "revision failed"
        });
        block_automated_run(db, run_id, task_id, &message)?;
        anyhow::bail!("{message}")
    };
    progress("worker running");
    let baseline_changes = git::inspect_worktree(&worktree_dir, repo_path)
        .context("failed to capture pre-revision change evidence")?;
    let invocation_id =
        start_provider_invocation_bounded(db, run_id, task_id, "revision", 1, revision_effort)?;
    let execution = worker.execute_structured_with_progress_and_usage(
        &prompt,
        &worktree_dir,
        &crate::automated::revision_handoff_schema(),
        &|line| worker_output(line),
    );
    db.finish_provider_invocation(
        invocation_id,
        if execution.is_ok() {
            "completed"
        } else {
            "failed"
        },
        execution.as_ref().ok().and_then(|value| value.token_usage),
    )?;
    let after_revision = git::inspect_worktree(&worktree_dir, repo_path)
        .context("failed to inspect worktree after Worker revision")?;
    let mut revision_step_snapshots = vec![(baseline_changes.clone(), after_revision)];
    let mut revision_step_outputs = vec![None; revision_plan.steps.len()];
    let revision_output = execution
        .as_ref()
        .ok()
        .and_then(|result| result.output.as_deref());
    if let Some(full_output) = revision_output {
        if let Ok(completion) = crate::worker_protocol::parse_plan_completion(full_output) {
            for reported in completion.step_results {
                if let Some(index) = revision_plan
                    .steps
                    .iter()
                    .position(|step| step.id == reported.step_id)
                {
                    let mut evidence = reported.observed.join("\n");
                    for path in reported.affected_files {
                        evidence.push_str(&format!("\nAFFECTED FILE: {path}"));
                    }
                    for check in reported.verification_passed {
                        evidence.push_str(&format!("\nVERIFICATION PASSED: {check}"));
                    }
                    revision_step_outputs[index] = Some(evidence);
                }
            }
        } else {
            for step_output in &mut revision_step_outputs {
                *step_output = Some(full_output.to_owned());
            }
        }
    }
    let execution = match execution {
        Ok(result) => result,
        Err(error) => {
            progress("worker failed");
            return fail(error);
        }
    };
    let outcome = execution.outcome;
    let mut output = execution.output;
    let token_usage = execution.token_usage;
    let fail = |message: String| -> Result<DispatchSummary> {
        progress(if message.to_ascii_lowercase().contains("timeout") {
            "worker timeout"
        } else {
            "revision failed"
        });
        db.update_agent_run_status_with_usage(run_id, "failed", Some(&message), token_usage)?;
        db.update_task_status(task_id, TaskStatus::Blocked)?;
        anyhow::bail!("{message}")
    };
    if let WorkerOutcome::Failure(error) = outcome {
        progress("worker failed");
        return fail(format!("Worker failed: {error}"));
    }
    progress("worker completed");
    if enforce_worker_protocol {
        let mut completion_repair = 0;
        loop {
            let failed_step = revision_plan
                .steps
                .iter()
                .enumerate()
                .find_map(|(index, step)| {
                    validate_worker_step_completion(
                        step,
                        revision_step_snapshots.get(index),
                        revision_step_outputs.get(index).and_then(Option::as_deref),
                        true,
                        &revision_plan.unchanged,
                    )
                    .err()
                    .map(|error| (index, error))
                });
            let Some((index, error)) = failed_step else {
                break;
            };
            if completion_repair >= MAX_COMPLETION_REPAIRS {
                let evidence = failed_execution_evidence(
                    &revision_plan,
                    &revision_step_outputs,
                    &revision_step_snapshots,
                    &[],
                    &error.to_string(),
                    true,
                );
                db.store_worker_execution(run_id, &evidence)?;
                return fail(format!(
                    "Worker revision completion self-check failed: {error:#}"
                ));
            }
            completion_repair += 1;
            let repair_diff = git::inspect_worktree(&worktree_dir, repo_path)?.diff;
            let repair_prompt = completion_repair_prompt(
                &repair_diff,
                &revision_plan.steps[index],
                &error.to_string(),
                completion_repair,
            );
            db.record_lifecycle_event(
                "worker_completion_repair_started",
                Some(task_id),
                Some(run_id),
                Some(agent_id),
                Some(
                    &serde_json::json!({
                        "attempt": completion_repair,
                        "step_id": revision_plan.steps[index].id,
                        "failure": error.to_string(),
                    })
                    .to_string(),
                ),
            )?;
            let before = revision_step_snapshots
                .get(index)
                .map(|(before, _)| before.clone())
                .context("completion repair lost the revision step snapshot")?;
            progress(&format!("completion repair attempt {completion_repair}"));
            let repair_invocation = start_provider_invocation_bounded(
                db,
                run_id,
                task_id,
                "completion_repair",
                completion_repair,
                ReasoningEffort::Low,
            )?;
            let repaired = worker.execute_planned_step_repair(
                &revision_plan.steps[index],
                &repair_prompt,
                &worktree_dir,
                &crate::worker_protocol::repair_completion_schema(),
                &|line| worker_output(line),
                None,
            );
            db.finish_provider_invocation(
                repair_invocation,
                if repaired.is_ok() {
                    "completed"
                } else {
                    "failed"
                },
                repaired.as_ref().ok().and_then(|value| value.token_usage),
            )?;
            let repaired = repaired.map_err(anyhow::Error::msg)?;
            if let WorkerOutcome::Failure(error) = repaired.outcome {
                return fail(format!("Worker completion repair failed: {error}"));
            }
            if let Some(repair_output) = repaired.output {
                revision_step_outputs[index] = Some(repair_output);
                output = Some(
                    revision_step_outputs
                        .iter()
                        .filter_map(|value| value.as_deref())
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                );
            }
            let after = git::inspect_worktree(&worktree_dir, repo_path)
                .context("failed to inspect worktree after completion repair")?;
            revision_step_snapshots[index] = (before, after);
            db.record_lifecycle_event(
                "worker_completion_repair_completed",
                Some(task_id),
                Some(run_id),
                Some(agent_id),
                Some(
                    &serde_json::json!({
                        "attempt": completion_repair,
                        "step_id": revision_plan.steps[index].id,
                    })
                    .to_string(),
                ),
            )?;
        }
    }
    let changes = match git::inspect_worktree(&worktree_dir, repo_path) {
        Ok(current) => git::changes_since(&baseline_changes, &current),
        Err(error) => return fail(format!("Post-worker inspection failed: {error:#}")),
    };
    if changes.files.is_empty() {
        return fail("Revision completed without meaningful project changes.".into());
    }
    db.store_change_evidence(run_id, &changes)?;
    // Configured project validation is owned by automated review, not
    // revision. Revision publishes implementation/change evidence and
    // transitions straight back into review.
    let combined = output.clone().unwrap_or_default();
    let performed_operations = performed_operations_for_plan(
        &revision_plan.steps,
        &revision_step_outputs,
        enforce_worker_protocol,
    )
    .context("Worker revision operation evidence failed")?;
    let revision_evidence = crate::worker_protocol::WorkerExecutionResult {
        protocol_version: crate::worker_protocol::WORKER_PROTOCOL_VERSION,
        performed_operations,
        affected_files: changes.files.iter().map(|file| file.path.clone()).collect(),
        requirement_coverage: requirement_coverage(&revision_plan),
        focused_verification: revision_plan
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let step_output = revision_step_outputs.get(index).and_then(Option::as_deref);
                crate::worker_protocol::StepEvidence {
                    step_id: step.id.clone(),
                    observed: worker_observations(
                        step_output,
                        "",
                        revision_step_snapshots
                            .get(index)
                            .map(|(_, after)| after)
                            .unwrap_or(&changes),
                        &step.verification,
                        !enforce_worker_protocol,
                    ),
                    verification: step.verification.clone(),
                    passed: step.verification.iter().all(|check| {
                        crate::worker_protocol::reported_verifications(
                            step_output.unwrap_or_default(),
                        )
                        .iter()
                        .any(|reported| reported == check)
                    }),
                }
            })
            .collect(),
        configured_validation: Vec::new(),
        unresolved_issues: Vec::new(),
    };
    if let Err(error) = revision_evidence.validate_against_plan(&revision_plan) {
        db.store_worker_execution(run_id, &revision_evidence)?;
        return fail(format!(
            "Worker revision verification evidence failed: {error:#}"
        ));
    }
    db.record_lifecycle_event(
        "worker_completion_gate",
        Some(task_id),
        Some(run_id),
        Some(agent_id),
        Some(
            &serde_json::json!({
                "status": "passed",
                "contract": "authoritative task and revision contract",
                "evidence": revision_evidence,
            })
            .to_string(),
        ),
    )?;
    db.store_worker_execution(run_id, &revision_evidence)
        .context("failed to persist Worker revision execution evidence")?;
    // The subsequent automated review is authoritative for blocker
    // resolution and validation. Consume and link the source review only
    // after the revision has produced inspectable, evidenced changes.
    if !db.complete_revision_run_for_review(
        task_id,
        run_id,
        source_review_id,
        contract_id,
        &combined,
        token_usage,
    )? {
        return fail("Revision review was consumed before this revision completed.".into());
    }
    progress("review transition");
    Ok(DispatchSummary {
        task: db
            .get_task(task_id)?
            .context("task disappeared after revision")?,
        agent: agent_id.into(),
        backend: agent.backend,
        profile: agent.profile_path,
        model: resolution.model,
        reasoning_effort: resolution.reasoning_effort,
        worktree_path,
        run_id,
        run_status: "completed".into(),
        validation: "deferred to review".into(),
        changes,
    })
}

pub fn revise_manual(
    task_id: &str,
    feedback: &str,
    agent: &AgentDefinition,
    db: &Database,
    repo_path: impl AsRef<Path>,
) -> Result<()> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;
    let project = db.get_project_name()?.unwrap_or_else(|| "orc".into());
    let task = db.get_task(task_id)?.context("task not found in DB")?;
    if matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled) {
        anyhow::bail!(
            "task {} cannot be revised from terminal status {}",
            task_id,
            task.status
        );
    }
    let Some((source_review_id, _source_feedback)) = db.actionable_revision_review(task_id)? else {
        anyhow::bail!(
            "task {} has no actionable REVISE review (currently {}); the prior review may already have been consumed by a completed revision. Run `orc review {} --automated` to publish a fresh review before revising again",
            task_id,
            task.status,
            task_id
        );
    };
    let (source_review_id, contract_id, revision_contract) =
        if let Some((source, json, id)) = db.actionable_revision_contract(task_id)? {
            (
                source,
                Some(id),
                serde_json::from_str(&json).context("persisted revision contract is invalid")?,
            )
        } else {
            (
                source_review_id,
                None,
                crate::automated::build_revision_contract_from_db(
                    db,
                    task_id,
                    &crate::review::build_review(db, task_id, repo_path)?.prior_reviews,
                    source_review_id,
                )?,
            )
        };
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    let run_id = db.create_agent_run_with_mode(project_id, task_id, &agent.id, registry::MANUAL)?;
    db.update_task_status(task_id, TaskStatus::Active)?;
    db.set_agent_run_waiting_external(run_id)?;
    if !db.start_revision_execution(run_id, source_review_id)? {
        anyhow::bail!("revision review was consumed before execution started");
    }
    if let Some(id) = contract_id {
        db.consume_revision_contract(id)?;
    }
    println!(
        "\n{}\n\n{}\n\n## Review feedback\n\n{}",
        build_manual_packet(&contract, &project, &task, &agent.id),
        crate::automated::format_revision_contract(&revision_contract),
        feedback
    );
    Ok(())
}

/// Public dispatch function using the Copilot worker and default DB path
pub fn dispatch(task_id: &str) -> Result<()> {
    dispatch_selected(task_id, None)
}

pub fn dispatch_selected(task_id: &str, requested_agent: Option<&str>) -> Result<()> {
    dispatch_selected_with_options(task_id, requested_agent, None, None).map(|summary| {
        println!("{}", crate::review::format_dispatch(&summary));
    })
}

pub fn dispatch_selected_with_summary(
    task_id: &str,
    requested_agent: Option<&str>,
) -> Result<DispatchSummary> {
    dispatch_selected_with_options(task_id, requested_agent, None, None)
}

pub fn dispatch_selected_with_options(
    task_id: &str,
    requested_agent: Option<&str>,
    model_override: Option<String>,
    effort_override: Option<crate::registry::ReasoningEffort>,
) -> Result<DispatchSummary> {
    let db_path = ".orc/orc.db";
    let db = Database::open_global(db_path)
        .with_context(|| format!("failed to open orc DB ({db_path}); run `orc init` first"))?;
    dispatch_selected_with_db_and_repo(
        &db,
        ".",
        task_id,
        requested_agent,
        model_override,
        effort_override,
    )
}

pub fn dispatch_selected_with_db_and_repo(
    db: &Database,
    repo_path: impl AsRef<Path>,
    task_id: &str,
    requested_agent: Option<&str>,
    model_override: Option<String>,
    effort_override: Option<crate::registry::ReasoningEffort>,
) -> Result<DispatchSummary> {
    dispatch_selected_with_db_and_repo_cancellable(
        db,
        repo_path,
        task_id,
        requested_agent,
        model_override,
        effort_override,
        None,
    )
}

pub fn dispatch_selected_with_db_and_repo_cancellable(
    db: &Database,
    repo_path: impl AsRef<Path>,
    task_id: &str,
    requested_agent: Option<&str>,
    model_override: Option<String>,
    effort_override: Option<crate::registry::ReasoningEffort>,
    cancellation: Option<&crate::worker::CancellationControl>,
) -> Result<DispatchSummary> {
    let repo_path = repo_path.as_ref();
    crate::queue::ensure_dispatchable(db, task_id)
        .map_err(|e| anyhow::anyhow!("dispatch eligibility check failed: {e}"))?;
    let task = db
        .get_task(task_id)?
        .with_context(|| format!("task '{}' not found in DB", task_id))?;
    let task_effort = task_contract_effort(db, &task)?;
    let explicit_effort = effort_override.is_some();
    let agent = if let Some(agent_id) = requested_agent {
        let agent = db
            .list_schedulable_agents()?
            .into_iter()
            .find(|agent| agent.id == agent_id)
            .with_context(|| {
                format!("agent '{agent_id}' is not referenced by the current project")
            })?;
        let busy_agents = db.list_busy_agents()?.into_iter().collect();
        crate::scheduler::validate_override_with_constraints(
            &agent,
            &task,
            &busy_agents,
            db.quota_reserve()?,
        )?;
        agent
    } else {
        let agents = db.list_schedulable_agents()?;
        let busy_agents = db.list_busy_agents()?.into_iter().collect();
        let decision = crate::scheduler::schedule_with_busy_and_quota_reserve(
            &task,
            &agents,
            None,
            &busy_agents,
            db.quota_reserve()?,
        )?;
        let selected_id = decision.selected_agent_id.ok_or_else(|| {
            anyhow::anyhow!(
                "no eligible agent found for task '{}': {}",
                task_id,
                decision.explanation
            )
        })?;
        agents
            .into_iter()
            .find(|a| a.id == selected_id)
            .ok_or_else(|| {
                anyhow::anyhow!("selected agent '{}' not found in registry", selected_id)
            })?
    };
    if agent.execution_mode == registry::MANUAL {
        if model_override.is_some() || effort_override.is_some() {
            anyhow::bail!(
                "manual agent '{}' does not support model or reasoning-effort overrides",
                agent.id
            );
        }
        dispatch_manual(task_id, &agent, db, repo_path)?;
        let task = db
            .get_task(task_id)?
            .context("task disappeared after manual dispatch")?;
        let run = db
            .list_agent_runs_for_task(task_id)?
            .into_iter()
            .next()
            .context("manual run missing")?;
        return Ok(DispatchSummary {
            task,
            agent: agent.id,
            backend: agent.backend,
            profile: agent.profile_path,
            model: None,
            reasoning_effort: None,
            worktree_path: "(created when patch is submitted)".into(),
            run_id: run.id,
            run_status: run.status,
            validation: "PENDING".into(),
            changes: Default::default(),
        });
    }
    let task_hints = db
        .get_task_execution_hints(&task.id)?
        .context("task execution hints are missing")?;
    let execution_class = task_hints
        .class
        .as_deref()
        .map(crate::execution::ExecutionClass::parse)
        .transpose()
        .map_err(anyhow::Error::msg)?
        .unwrap_or_else(|| crate::execution::class_for_role(&task.role));
    let resolution = crate::execution::resolve_with_template(
        execution_class.as_str(),
        &db.execution_template(execution_class)?,
        agent.model.as_deref(),
        agent.reasoning_effort,
        model_override.or(task_hints.model),
        effort_override,
    );
    let resolution = if explicit_effort {
        resolution
    } else {
        apply_task_effort(resolution, Some(task_effort))
    };
    let model = resolution.model.clone();
    let reasoning_effort = resolution.reasoning_effort;
    let worker = WorkerFactory::build_with_overrides(&agent, model.clone(), reasoning_effort)
        .map_err(anyhow::Error::msg)?;
    let mut summary = dispatch_with_worker_on_db_cancellable(
        task_id,
        worker.as_ref(),
        db,
        repo_path,
        &agent.id,
        &SystemValidationRunner,
        cancellation,
    )?;
    db.set_agent_run_execution(
        summary.run_id,
        resolution.class.as_str(),
        model.as_deref(),
        reasoning_effort,
        &resolution.source,
    )?;
    db.set_agent_run_profile(summary.run_id, agent.profile_path.as_deref())?;
    summary.backend = agent.backend;
    summary.profile = agent.profile_path;
    summary.model = model;
    summary.reasoning_effort = reasoning_effort;
    Ok(summary)
}

pub fn plan_dispatch_assignments(
    ready: &[QueueEntry],
    agents: &[AgentDefinition],
    busy_agents: &HashSet<String>,
    quota_reserve: i64,
    concurrency: Option<usize>,
) -> Result<Vec<(String, String)>> {
    if concurrency == Some(0) {
        anyhow::bail!("concurrency must be greater than zero");
    }
    let mut reserved = busy_agents.clone();
    let mut assignments = Vec::new();
    for entry in ready {
        if concurrency.is_some_and(|limit| assignments.len() == limit) {
            break;
        }
        let decision = crate::scheduler::schedule_with_busy_and_quota_reserve(
            &entry.task,
            agents,
            Some(registry::AUTOMATED),
            &reserved,
            quota_reserve,
        )?;
        if let Some(agent_id) = decision.selected_agent_id {
            reserved.insert(agent_id.clone());
            assignments.push((entry.task.id.clone(), agent_id));
        }
    }
    Ok(assignments)
}

pub type DispatchQueueOutcomes = BTreeMap<String, Result<DispatchSummary, String>>;

pub fn dispatch_queue(concurrency: Option<usize>) -> Result<DispatchQueueOutcomes> {
    let db = Database::open_global(".orc/orc.db")?;
    let report = crate::queue::compute_queue(&db)?;
    let agents = db.list_schedulable_agents()?;
    let quota_reserve = db.quota_reserve()?;
    let assignments = plan_dispatch_assignments(
        &report.ready,
        &agents,
        &db.list_busy_agents()?.into_iter().collect::<HashSet<_>>(),
        quota_reserve,
        concurrency,
    )?;
    let mut outcomes = BTreeMap::new();
    let handles = assignments
        .iter()
        .map(|(task_id, agent_id)| {
            let task_id = task_id.clone();
            let agent_id = agent_id.clone();
            thread::spawn(move || {
                dispatch_selected_with_options(&task_id, Some(&agent_id), None, None)
            })
        })
        .collect::<Vec<_>>();
    for ((task_id, _), handle) in assignments.into_iter().zip(handles) {
        let outcome = match handle.join() {
            Ok(Ok(summary)) => Ok(summary),
            Ok(Err(error)) => Err(format!("{error:#}")),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("worker thread panicked");
                Err(format!("worker thread panicked: {message}"))
            }
        };
        outcomes.insert(task_id, outcome);
    }
    Ok(outcomes)
}

pub fn accept_task(db: &Database, task_id: &str, repo_path: impl AsRef<Path>) -> Result<()> {
    let repo_path = repo_path.as_ref();
    let task = db.get_task(task_id)?.context("task not found")?;
    if task.status != TaskStatus::Review {
        anyhow::bail!(
            "task {} can only be accepted from review (currently {})",
            task_id,
            task.status
        );
    }
    let (branch, path) = db
        .get_worktree_metadata(task_id)?
        .context("task has no worktree")?;
    let worktree = repo_path.join(&path);
    if !worktree.exists() {
        anyhow::bail!("task worktree does not exist: {}", worktree.display());
    }
    if git::inspect_worktree(&worktree, repo_path)?
        .files
        .is_empty()
    {
        anyhow::bail!("task {task_id} has no meaningful changes to accept");
    }
    git::commit_worktree_changes(&worktree, task_id, &task.title)?;
    git::merge_task_branch(repo_path, &branch, task_id)?;
    git::remove_worktree(repo_path, &path)?;
    db.update_task_status(task_id, TaskStatus::Done)?;
    db.record_lifecycle_event("review_accept", Some(task_id), None, None, None)?;
    Ok(())
}

pub fn reject_task(db: &Database, task_id: &str, reason: Option<&str>) -> Result<()> {
    let task = db.get_task(task_id)?.context("task not found")?;
    if task.status != TaskStatus::Review {
        anyhow::bail!(
            "task {} can only be rejected from review (currently {})",
            task_id,
            task.status
        );
    }
    if let (Some(reason), Some(run)) = (
        reason,
        db.list_agent_runs_for_task(task_id)?.into_iter().next(),
    ) {
        let output = format!(
            "{}\n\nReview rejected: {}",
            run.output.unwrap_or_default(),
            reason
        );
        db.update_agent_run_output(run.id, &output)?;
    }
    db.update_task_status(task_id, TaskStatus::Ready)?;
    db.record_lifecycle_event("review_reject", Some(task_id), None, None, reason)?;
    Ok(())
}

pub fn dispatch_manual(
    task_id: &str,
    agent: &AgentDefinition,
    db: &Database,
    repo_path: impl AsRef<Path>,
) -> Result<()> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    let project = db.get_project_name()?.unwrap_or_else(|| "orc".into());
    let task = db.get_task(task_id)?.context("task not found in DB")?;
    if matches!(
        task.status,
        TaskStatus::Active | TaskStatus::Done | TaskStatus::Cancelled
    ) {
        anyhow::bail!(
            "Task {} cannot be manually dispatched from status {}",
            task_id,
            task.status
        );
    }
    db.update_task_status(task_id, TaskStatus::Active)?;
    let run_id =
        db.create_agent_run_with_mode(project_id, task_id, &agent.id, &agent.execution_mode)?;
    if !db.set_agent_run_waiting_external(run_id)? {
        anyhow::bail!("failed to put run {} into waiting_external", run_id);
    }
    println!(
        "Run {} (agent={}, mode=manual, status=waiting_external)",
        run_id, agent.id
    );
    println!(
        "\n{}",
        build_manual_packet(&contract, &project, &task, &agent.id)
    );
    Ok(())
}

pub fn submit_run(db: &Database, run_id: i64, output: &str) -> Result<String> {
    let run = db.get_agent_run(run_id)?.context("run not found")?;
    if run.execution_mode != registry::MANUAL || run.status != "waiting_external" {
        anyhow::bail!("run {} is not a waiting manual run", run_id);
    }
    let task_id = run.task_id.clone().context("manual run has no task")?;
    if let Some(source_review_id) = db.source_review_run_id(run_id)? {
        let reviews = crate::review::build_review(db, &task_id, Path::new("."))?.prior_reviews;
        let contract = crate::automated::build_revision_contract_from_db(
            db,
            &task_id,
            &reviews,
            source_review_id,
        )?;
        let changes = db.get_change_evidence(run_id)?;
        let handoff = crate::automated::validate_revision_handoff_with_evidence(
            &contract,
            output,
            changes.as_ref(),
        )
        .context("invalid revision handoff")?;
        db.record_lifecycle_event(
            "revision_handoff",
            Some(&task_id),
            Some(run_id),
            Some(&run.agent),
            Some(&serde_json::to_string(&handoff)?),
        )?;
    }
    db.complete_manual_run(run_id, output)?;
    db.update_task_status(&task_id, TaskStatus::Review)?;
    Ok(task_id)
}

pub fn fail_run(db: &Database, run_id: i64, reason: &str) -> Result<String> {
    let run = db.get_agent_run(run_id)?.context("run not found")?;
    if run.execution_mode != registry::MANUAL || run.status != "waiting_external" {
        anyhow::bail!("run {} is not a waiting manual run", run_id);
    }
    let task_id = db.fail_run(run_id, reason)?;
    db.update_task_status(&task_id, TaskStatus::Blocked)?;
    Ok(task_id)
}

#[derive(Debug, Clone)]
pub struct PatchSubmissionOutcome {
    pub run_id: i64,
    pub task_id: String,
    pub worktree_path: PathBuf,
    pub branch_name: String,
    pub validation_report: ValidationReport,
}

pub fn submit_patch(
    db: &Database,
    run_id: i64,
    patch_content: &str,
    repo_path: impl AsRef<Path>,
) -> Result<PatchSubmissionOutcome> {
    submit_patch_with_runner(
        db,
        run_id,
        patch_content,
        repo_path,
        &SystemValidationRunner,
    )
}

pub fn submit_patch_with_runner(
    db: &Database,
    run_id: i64,
    patch_content: &str,
    repo_path: impl AsRef<Path>,
    validation_runner: &dyn ValidationRunner,
) -> Result<PatchSubmissionOutcome> {
    let repo_path = repo_path.as_ref();
    let run = db
        .get_agent_run(run_id)?
        .with_context(|| format!("run {} not found", run_id))?;

    if run.execution_mode != registry::MANUAL {
        anyhow::bail!(
            "run {} has execution_mode '{}'; only manual runs accept submit-patch",
            run_id,
            run.execution_mode
        );
    }
    if run.status != "waiting_external" {
        anyhow::bail!(
            "run {} is in status '{}'; only waiting_external manual runs accept submit-patch",
            run_id,
            run.status
        );
    }
    let task_id = run
        .task_id
        .clone()
        .with_context(|| format!("run {} has no associated task", run_id))?;

    let task = db
        .get_task(&task_id)?
        .with_context(|| format!("task '{}' not found in DB", task_id))?;

    if task.status == TaskStatus::Done {
        anyhow::bail!("task {} is already done; cannot submit patch", task_id);
    }

    if patch_content.trim().is_empty() {
        let err_msg = "malformed patch: patch content is empty";
        db.update_agent_run_output(run_id, err_msg)
            .context("failed to record malformed patch error")?;
        anyhow::bail!("{}", err_msg);
    }

    // Ensure task worktree exists
    let (branch_name, worktree_path) = match db.get_worktree_metadata(&task_id)? {
        Some((branch, path_str)) if repo_path.join(&path_str).exists() => {
            (branch, PathBuf::from(path_str))
        }
        _ => {
            let (branch, path) = git::ensure_worktree(&task_id, repo_path)?;
            (branch, path)
        }
    };

    // Record worktree metadata for this run
    db.store_worktree_metadata(
        run_id,
        &task_id,
        &branch_name,
        &worktree_path.to_string_lossy(),
    )
    .context("failed to store worktree metadata for patch submission")?;

    let absolute_worktree = repo_path.join(&worktree_path);

    // 1. Validate patch against worktree (git apply --check)
    if let Err(e) = git::validate_patch(&absolute_worktree, patch_content) {
        let err_msg = format!("patch validation failed: {:#}", e);
        db.update_agent_run_output(run_id, &err_msg)
            .context("failed to record patch validation error")?;
        anyhow::bail!("{}", err_msg);
    }

    // 2. Apply patch to worktree (git apply)
    if let Err(e) = git::apply_patch(&absolute_worktree, patch_content) {
        let err_msg = format!("patch apply failed: {:#}", e);
        db.fail_run(run_id, &err_msg)
            .context("failed to mark patch submission run as failed")?;
        db.update_task_status(&task_id, TaskStatus::Blocked)
            .context("failed to block task after patch apply failure")?;
        anyhow::bail!("{}", err_msg);
    }

    let changes = git::inspect_worktree(&absolute_worktree, repo_path)
        .context("failed to capture applied patch change evidence")?;
    let change_evidence = serde_json::to_string(&changes)
        .context("failed to serialize applied patch change evidence")?;
    db.record_lifecycle_event(
        "change_evidence",
        Some(&task_id),
        Some(run_id),
        Some(&run.agent),
        Some(&change_evidence),
    )?;

    // 3. Run project validation pipeline
    let validation_config = ValidationConfig::load(repo_path)?;
    let report = validation::run_validation_pipeline(
        validation_runner,
        &validation_config.commands,
        &absolute_worktree,
    )?;
    let validation_evidence =
        serde_json::to_string(&report).context("failed to serialize validation result")?;
    db.record_lifecycle_event(
        "validation_result",
        Some(&task_id),
        Some(run_id),
        Some(&run.agent),
        Some(&validation_evidence),
    )?;

    if !report.is_success() {
        let failure_summary = format!(
            "Worktree: {}\nApplied: yes\n\nValidation:\n{}\nFailure: project validation",
            worktree_path.display(),
            report.summary()
        );
        db.fail_run(run_id, &failure_summary)
            .context("failed to mark patch validation run as failed")?;
        db.update_task_status(&task_id, TaskStatus::Review)
            .context("failed to set task to review after patch validation failure")?;
        anyhow::bail!(
            "Validation failed after applying patch to {}:\n{}",
            worktree_path.display(),
            report.summary()
        );
    }

    // 4. Success: persist output and transition lifecycle
    let success_output = format!(
        "Worktree: {}\nApplied: yes\n\nValidation:\n{}\nPatch:\n{}",
        worktree_path.display(),
        report.summary(),
        patch_content
    );
    let changes = git::inspect_worktree(&absolute_worktree, repo_path)?;
    db.store_change_evidence(run_id, &changes)?;
    db.complete_manual_run(run_id, &success_output)?;
    db.update_task_status(&task_id, TaskStatus::Review)?;

    Ok(PatchSubmissionOutcome {
        run_id,
        task_id,
        worktree_path,
        branch_name,
        validation_report: report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{AVAILABLE, MANUAL};
    use crate::storage::Database;
    use crate::task::TaskPriority;
    use crate::validation::test_helpers::FakeValidationRunner;
    use std::process::Command;
    use tempfile::tempdir;

    fn manual_agent() -> AgentDefinition {
        AgentDefinition {
            id: "chatgpt-lead".into(),
            backend: "chatgpt".into(),
            execution_mode: MANUAL.into(),
            display_name: "ChatGPT Lead".into(),
            enabled: true,
            priority: 100,
            capabilities: vec!["planning".into(), "review".into()],
            status: AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: None,
            model: None,
            reasoning_effort: None,
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
            actions: vec![registry::AgentAction::Code],
        }
    }

    fn init_git_repo(repo_path: &Path) {
        Command::new("git")
            .current_dir(repo_path)
            .arg("init")
            .arg(".")
            .output()
            .expect("init git");
        Command::new("git")
            .current_dir(repo_path)
            .arg("config")
            .arg("user.email")
            .arg("test@example.com")
            .output()
            .expect("git config email");
        Command::new("git")
            .current_dir(repo_path)
            .arg("config")
            .arg("user.name")
            .arg("Test User")
            .output()
            .expect("git config name");
        std::fs::write(repo_path.join("README.md"), "initial content\n").unwrap();
        Command::new("git")
            .current_dir(repo_path)
            .arg("add")
            .arg(".")
            .output()
            .expect("git add");
        Command::new("git")
            .current_dir(repo_path)
            .arg("commit")
            .arg("-m")
            .arg("initial commit")
            .output()
            .expect("git commit");
    }

    fn setup() -> (tempfile::TempDir, Database, String) {
        let dir = tempdir().unwrap();
        init_git_repo(dir.path());
        std::fs::create_dir_all(dir.path().join(".orc")).unwrap();
        std::fs::write(dir.path().join(".orc/engineering.md"), "Do focused work.").unwrap();
        let db = Database::init(dir.path().join(".orc/orc.db")).unwrap();
        let project = db.create_project("demo").unwrap();
        db.insert_task(
            project,
            "Review API",
            "Review the API design",
            "review",
            TaskPriority::Normal,
        )
        .unwrap();
        db.insert_agent(&manual_agent()).unwrap();
        (dir, db, "T-0001".into())
    }

    #[test]
    fn revision_effort_escalates_once_per_surviving_blocker_and_stops_at_high() {
        let (_dir, db, task) = setup();
        let project = db.get_project_id().unwrap().unwrap();
        let blocker = || crate::automated::ReviewBlocker {
            id: "BLK-repeat".into(),
            prior_blocker_id: None,
            blocker_key: "repeatable blocker".into(),
            requirement_ref: "acceptance".into(),
            evidence: "still failing".into(),
            severity: "high".into(),
            acceptance_condition: "passes".into(),
            status: "unresolved".into(),
            finding: "same substantive issue".into(),
        };
        let review = |db: &Database| {
            let run = db
                .create_agent_run_with_execution(
                    project,
                    &task,
                    "reviewer",
                    registry::AUTOMATED,
                    crate::storage::AgentRunExecution {
                        class: "review",
                        model: None,
                        effort: None,
                        source: "test",
                    },
                )
                .unwrap();
            db.store_review_blockers(&task, run, &[blocker()]).unwrap();
            db.update_agent_run_status(run, "completed", Some("review"))
                .unwrap();
            run
        };
        let revision = |db: &Database, source_review: i64, effort: ReasoningEffort| {
            let run = db
                .create_agent_run_with_execution(
                    project,
                    &task,
                    "coder",
                    registry::AUTOMATED,
                    crate::storage::AgentRunExecution {
                        class: "coder",
                        model: None,
                        effort: Some(effort),
                        source: "test",
                    },
                )
                .unwrap();
            assert!(db.start_revision_execution(run, source_review).unwrap());
            db.update_agent_run_status(run, "completed", Some("revision"))
                .unwrap();
        };
        let effort = |review_id| {
            effective_revision_effort(&db, &db.get_task(&task).unwrap().unwrap(), review_id, None)
                .unwrap()
        };

        let first_review = review(&db);
        assert_eq!(effort(first_review), ReasoningEffort::Low);
        revision(&db, first_review, ReasoningEffort::Low);
        let second_review = review(&db);
        assert_eq!(effort(second_review), ReasoningEffort::Medium);
        assert_eq!(
            effort_with_override(&db, &task, second_review, ReasoningEffort::Low),
            ReasoningEffort::Medium
        );
        revision(&db, second_review, ReasoningEffort::Medium);
        let third_review = review(&db);
        assert_eq!(effort(third_review), ReasoningEffort::High);
        revision(&db, third_review, ReasoningEffort::High);
        let fourth_review = review(&db);
        let error = effective_revision_effort(
            &db,
            &db.get_task(&task).unwrap().unwrap(),
            fourth_review,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("REPLAN_REQUIRED"));
        let condition = db.get_task_execution_condition(&task).unwrap().unwrap();
        assert_eq!(condition.kind, "non_convergence_replan_required");
        let error = effective_revision_effort(
            &db,
            &db.get_task(&task).unwrap().unwrap(),
            fourth_review,
            Some(ReasoningEffort::High),
        )
        .unwrap_err();
        assert!(error.to_string().contains("REPLAN_REQUIRED"));
    }

    fn effort_with_override(
        db: &Database,
        task_id: &str,
        review_id: i64,
        override_effort: ReasoningEffort,
    ) -> ReasoningEffort {
        effective_revision_effort(
            db,
            &db.get_task(task_id).unwrap().unwrap(),
            review_id,
            Some(override_effort),
        )
        .unwrap()
    }

    #[test]
    fn manual_dispatch_creates_waiting_run_without_worker_and_packet() {
        let (dir, db, task_id) = setup();
        let agent = manual_agent();
        let packet = build_manual_packet(
            "contract text",
            "demo",
            &db.get_task(&task_id).unwrap().unwrap(),
            &agent.id,
        );
        assert!(packet.contains("contract text"));
        assert!(packet.contains("T-0001"));
        dispatch_manual(&task_id, &agent, &db, dir.path()).unwrap();
        let task = db.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Active);
        let run = db
            .list_agent_runs_for_task(&task_id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(run.execution_mode, MANUAL);
        assert_eq!(run.status, "waiting_external");
        db.set_agent_execution_mode(&agent.id, registry::AUTOMATED)
            .unwrap();
        drop(db);
        let reopened = Database::open(dir.path().join(".orc/orc.db")).unwrap();
        let run = reopened
            .list_agent_runs_for_task(&task_id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(run.execution_mode, MANUAL);
    }

    #[test]
    fn submit_and_fail_manual_runs_transition_tasks_and_preserve_output() {
        let (dir, db, task_id) = setup();
        dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
        let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;
        assert_eq!(
            submit_run(&db, run_id, "review completed").unwrap(),
            task_id
        );
        let run = db.get_agent_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, "completed");
        assert_eq!(run.output.as_deref(), Some("review completed"));
        assert_eq!(
            db.get_task(&task_id).unwrap().unwrap().status,
            TaskStatus::Review
        );
        assert!(submit_run(&db, run_id, "again").is_err());

        let second_task = db
            .insert_task(
                db.get_project_id().unwrap().unwrap(),
                "Second",
                "Second",
                "review",
                TaskPriority::Normal,
            )
            .unwrap();
        dispatch_manual(&second_task, &manual_agent(), &db, dir.path()).unwrap();
        let second_run = db.list_agent_runs_for_task(&second_task).unwrap()[0].id;
        assert_eq!(
            fail_run(&db, second_run, "needs more detail").unwrap(),
            second_task
        );
        assert_eq!(
            db.get_task(&second_task).unwrap().unwrap().status,
            TaskStatus::Blocked
        );
        assert!(fail_run(&db, second_run, "again").is_err());
    }

    #[test]
    fn submit_patch_success_flow() {
        let (dir, db, task_id) = setup();
        dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
        let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

        let patch = "diff --git a/new_file.txt b/new_file.txt
new file mode 100644
--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1 @@
+hello manual patch
";
        let runner = FakeValidationRunner::success();
        let outcome = submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner)
            .expect("submit patch");

        assert_eq!(outcome.run_id, run_id);
        assert_eq!(outcome.task_id, task_id);
        assert!(outcome.validation_report.is_success());

        // Check task status moved to review
        let task = db.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Review);

        // Check run marked completed with output
        let run = db.get_agent_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, "completed");
        assert!(run.output.as_ref().unwrap().contains("hello manual patch"));

        // Check applied in worktree, not main
        let worktree_file = dir.path().join(&outcome.worktree_path).join("new_file.txt");
        assert!(worktree_file.exists());
        assert_eq!(
            std::fs::read_to_string(worktree_file).unwrap(),
            "hello manual patch\n"
        );
        assert!(!dir.path().join("new_file.txt").exists());
    }

    #[test]
    fn submit_patch_validation_failure_leaves_run_actionable() {
        let (dir, db, task_id) = setup();
        dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
        let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

        // Invalid patch
        let bad_patch = "not a valid diff";
        let runner = FakeValidationRunner::success();
        let err =
            submit_patch_with_runner(&db, run_id, bad_patch, dir.path(), &runner).unwrap_err();
        assert!(err.to_string().contains("patch validation failed"));

        // Run is still waiting_external and task is still active
        let run = db.get_agent_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, "waiting_external");
        let task = db.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Active);

        // Can resubmit with a valid patch
        let good_patch = "diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-initial content
+updated content
";
        let outcome = submit_patch_with_runner(&db, run_id, good_patch, dir.path(), &runner)
            .expect("resubmission should succeed");
        assert_eq!(outcome.task_id, task_id);
        assert_eq!(
            db.get_task(&task_id).unwrap().unwrap().status,
            TaskStatus::Review
        );
    }
}
