use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::thread;

use crate::backend::WorkerFactory;
use crate::contract;
use crate::git;
use crate::queue::QueueEntry;
use crate::registry::{self, AgentDefinition, EscalationRequest, ReasoningEffort};
use crate::review::DispatchSummary;
use crate::storage::Database;
use crate::task::{Task, TaskScopeMode, TaskStatus};
use crate::validation::{self, SystemValidationRunner, ValidationReport, ValidationRunner};
use crate::worker::{TokenUsage, Worker, WorkerOutcome};

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

/// Convert the canonical structured completion envelope into the per-step
/// evidence retained with the implementation result.
fn completion_step_evidence(reported: crate::worker_protocol::ReportedStepCompletion) -> String {
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
    evidence
}

fn normalized_completion_step_output(output: &str, step_id: &str) -> Result<String> {
    let completion = crate::worker_protocol::parse_plan_completion(output)?;
    let reported = completion
        .step_results
        .into_iter()
        .find(|reported| reported.step_id == step_id)
        .with_context(|| format!("structured completion repair did not report step '{step_id}'"))?;
    Ok(completion_step_evidence(reported))
}

fn merge_completion_step_output(existing: Option<&str>, repair: String) -> String {
    match existing.filter(|value| !value.trim().is_empty()) {
        Some(existing) => format!("{existing}\n{repair}"),
        None => repair,
    }
}

fn failed_execution_evidence(
    plan: &crate::worker_protocol::WorkerPlan,
    outputs: &[Option<String>],
    snapshots: &[(git::WorktreeChanges, git::WorktreeChanges)],
    configured_validation: &[String],
    issue: &str,
    _enforce_protocol: bool,
) -> crate::worker_protocol::WorkerExecutionResult {
    let performed_operations = plan
        .steps
        .iter()
        .enumerate()
        .flat_map(|(index, step)| {
            let _ = outputs.get(index);
            step.operations.clone()
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

fn planned_operations_for_plan(
    steps: &[crate::worker_protocol::PlannedStep],
) -> Vec<crate::worker_protocol::PlannedOperation> {
    steps
        .iter()
        .flat_map(|step| step.operations.clone())
        .collect()
}

const ENGINEERING_CONTRACT_PATH: &str = ".orc/engineering.md";
const ARCHITECTURE_DECISION_MARKER: &str = "ORC-ARCHITECTURE-DECISION:";
const MAX_VALIDATION_REPAIRS: usize = 3;
const MAX_COMPLETION_REPAIRS: usize = 2;

fn start_provider_invocation_bounded(
    db: &Database,
    run_id: i64,
    task_id: &str,
    purpose: &str,
    attempt: usize,
    resolution: &crate::registry::ResolutionRecord,
) -> Result<i64> {
    match db.start_provider_invocation_with_resolution(run_id, purpose, attempt, resolution) {
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

fn repair_resolution_record(
    db: &Database,
    run_id: i64,
    task_id: &str,
    purpose: &str,
    effort: ReasoningEffort,
) -> Result<crate::registry::ResolutionRecord> {
    let run = db
        .get_agent_run(run_id)?
        .context("provider invocation parent run disappeared")?;
    let task = db
        .get_task(task_id)?
        .context("provider invocation task disappeared")?;
    Ok(
        crate::scheduler::resolve_run_invocation_economy_for_execution(
            db,
            &task,
            &run.agent,
            run.resolved_model,
            Some(effort),
            purpose,
            crate::scheduler::TransportEligibility::IgnoreUnsupportedBackend,
        )?
        .record,
    )
}
#[derive(Clone, Debug, Default)]
pub struct RevisionExecutionOverrides {
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "revision resolution keeps authority and policy inputs explicit"
)]
fn resolve_revision_economy(
    db: &Database,
    task: &Task,
    agent_id: &str,
    overrides: &RevisionExecutionOverrides,
    revision_effort: ReasoningEffort,
    transport: crate::scheduler::TransportEligibility,
    operator_agent_override: bool,
    escalation_request: Option<EscalationRequest>,
    quota_refresher: &dyn crate::scheduler::QuotaRefresher,
) -> Result<crate::scheduler::EconomyResolution> {
    let automatic_escalation = escalation_request.is_some()
        && !operator_agent_override
        && overrides.model.is_none()
        && overrides.effort.is_none();
    let decision = crate::scheduler::resolve_task_economy_for_execution_with_refresher(
        db,
        task,
        crate::registry::AgentAction::Code,
        crate::scheduler::EconomyOverrides {
            agent_id: operator_agent_override.then(|| agent_id.into()),
            model: overrides.model.clone(),
            effort: overrides.effort,
        },
        Some(registry::AUTOMATED),
        (!operator_agent_override && !automatic_escalation).then(|| agent_id.into()),
        Some(revision_effort),
        Some("revision_contract".into()),
        transport,
        escalation_request,
        "task_revision",
        &HashSet::new(),
        quota_refresher,
    )?;
    decision.resolution.ok_or_else(|| {
        anyhow::anyhow!(
            "agent '{agent_id}' is not eligible for revision: {}",
            decision.schedule.explanation
        )
    })
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

fn effective_revision_effort(
    db: &Database,
    task: &crate::task::Task,
    _source_review_id: i64,
    explicit_override: Option<ReasoningEffort>,
) -> Result<ReasoningEffort> {
    let base = task_contract_effort(db, task)?;
    Ok(explicit_override.unwrap_or(base))
}

fn persist_escalation_decision(
    db: &Database,
    task_id: &str,
    observation: crate::scheduler::EscalationObservation,
    previous: &crate::storage::db::ProviderResolution,
    policy_attempt: usize,
) -> Result<Option<EscalationRequest>> {
    let configuration = db.escalation_policy_configuration()?;
    match crate::scheduler::evaluate_escalation_policy(crate::scheduler::EscalationPolicyInput {
        observation,
        previous_provider_invocation_id: Some(previous.invocation_id),
        previous_resolution: Some(&previous.resolution),
        previous_attempt: previous.attempt,
        policy_attempt,
        configuration: &configuration,
    }) {
        crate::scheduler::EscalationDecision::NoEscalation { .. } => Ok(None),
        crate::scheduler::EscalationDecision::Escalate(request) => {
            let persisted = db.persist_escalation_request(task_id, &request)?;
            db.record_lifecycle_event(
                "economy_escalation_requested",
                Some(task_id),
                None,
                None,
                Some(&serde_json::to_string(&persisted)?),
            )?;
            Ok(Some(persisted.request))
        }
        crate::scheduler::EscalationDecision::Exhausted { reason } => {
            let details = serde_json::json!({
                "previous_provider_invocation_id": previous.invocation_id,
                "previous_tier": previous.resolution.tier.as_str(),
                "reason": reason,
            });
            db.set_task_execution_condition(
                task_id,
                "economy_escalation_exhausted",
                &details.to_string(),
            )?;
            Ok(None)
        }
    }
}

fn semantic_escalation_request(
    db: &Database,
    task: &Task,
    source_review_id: i64,
    overrides: &RevisionExecutionOverrides,
    operator_agent_override: bool,
) -> Result<Option<EscalationRequest>> {
    if operator_agent_override || overrides.model.is_some() || overrides.effort.is_some() {
        return Ok(None);
    }
    for blocker in db
        .review_blocker_observations(source_review_id)?
        .into_iter()
        .filter(|blocker| blocker.status != "resolved")
    {
        if let Some(previous) = db.completed_revision_resolution_for_blocker(
            &task.id,
            source_review_id,
            &blocker.blocker_id,
        )? {
            let policy_attempt = previous
                .resolution
                .escalation
                .as_ref()
                .map_or(1, |lineage| lineage.policy_attempt + 1);
            let escalation = persist_escalation_decision(
                db,
                &task.id,
                crate::scheduler::EscalationObservation::SemanticRevisionNonConvergence,
                &previous,
                policy_attempt,
            )?;
            if escalation.is_none()
                && db
                    .get_task_execution_condition(&task.id)?
                    .is_some_and(|condition| condition.kind == "economy_escalation_exhausted")
            {
                bail!(
                    "NON_CONVERGENCE: no stronger eligible economy tier remains for task '{}'",
                    task.id
                );
            }
            return Ok(escalation);
        }
    }
    Ok(None)
}

fn validate_worker_step_completion(
    step: &crate::worker_protocol::PlannedStep,
    snapshot: Option<&(git::WorktreeChanges, git::WorktreeChanges)>,
) -> Result<()> {
    let Some((_before, after)) = snapshot else {
        anyhow::bail!("Worker did not execute persisted step '{}'", step.id);
    };
    let has_mutation = step.operations.iter().any(|operation| {
        matches!(
            operation,
            crate::worker_protocol::PlannedOperation::Create
                | crate::worker_protocol::PlannedOperation::Modify
                | crate::worker_protocol::PlannedOperation::Delete
                | crate::worker_protocol::PlannedOperation::Move
        )
    });
    if has_mutation && after.files.is_empty() {
        anyhow::bail!(
            "Worker did not produce a worktree implementation effect for step '{}'",
            step.id
        );
    }
    Ok(())
}

fn completion_repair_prompt(
    diff: &str,
    step: &crate::worker_protocol::PlannedStep,
    failure: &str,
    attempt: usize,
) -> String {
    let diff =
        crate::execution_packet::BoundedText::new(diff, crate::execution_packet::MAX_DIFF_BYTES);
    let failure = crate::execution_packet::BoundedText::new(failure, 6_000);
    format!(
        "WORKER COMPLETION REPAIR (attempt {attempt} of {MAX_COMPLETION_REPAIRS}). Repair only the missing implementation effect for the checkpoint below. Preserve the existing worktree and unrelated changes. Do not run tests, validation, acceptance checks, or reviewer-style verification; Orc owns deterministic validation.\n\nEXACT FAILURE:\n{}\n\nCURRENT DIFF (bounded; omitted bytes={}):\n{}\n\nPERSISTED STEP:\n{}\n\nReturn a structured Worker completion object using the canonical `step_results` envelope. Completion metadata is descriptive only; Orc will derive changed files from the worktree.",
        failure.text,
        diff.omitted_bytes,
        diff.text,
        serde_json::to_string_pretty(step).unwrap_or_else(|_| step.id.clone())
    )
}

fn merge_token_usage(accumulated: &mut Option<TokenUsage>, additional: Option<TokenUsage>) {
    let Some(additional) = additional else {
        return;
    };
    let Some(existing) = accumulated.as_mut() else {
        *accumulated = Some(additional);
        return;
    };
    existing.total_tokens += additional.total_tokens;
    existing.input_tokens = match (existing.input_tokens, additional.input_tokens) {
        (Some(left), Some(right)) => Some(left + right),
        (left, None) => left,
        (None, right) => right,
    };
    existing.output_tokens = match (existing.output_tokens, additional.output_tokens) {
        (Some(left), Some(right)) => Some(left + right),
        (left, None) => left,
        (None, right) => right,
    };
    existing.cached_input_tokens =
        match (existing.cached_input_tokens, additional.cached_input_tokens) {
            (Some(left), Some(right)) => Some(left + right),
            (left, None) => left,
            (None, right) => right,
        };
}

#[expect(
    clippy::too_many_arguments,
    reason = "validation repair owns one implementation run"
)]
fn run_task_validation_repair_loop(
    validation_runner: &dyn ValidationRunner,
    validation_config: &crate::validation::ValidationConfig,
    required_validation: &[String],
    worker: &dyn Worker,
    db: &Database,
    worktree: &Path,
    repo_path: &Path,
    task_id: &str,
    run_id: i64,
    agent_id: &str,
    output: &mut Option<String>,
    token_usage: &mut Option<TokenUsage>,
    progress: &dyn Fn(&str),
    worker_output: &dyn Fn(&str),
    cancellation: Option<&crate::worker::CancellationControl>,
) -> Result<(ValidationReport, crate::validation::ValidationSelection)> {
    let mut repair_attempt = 0;
    let mut last_repair_resolution = None;
    loop {
        let current = git::inspect_worktree(worktree, repo_path)?;
        let changed_files = current
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        let selection = validation_config.select_for_task(&changed_files, required_validation);
        let report =
            validation::run_validation_pipeline(validation_runner, &selection.commands, worktree)
                .unwrap_or_else(|error| {
                    ValidationReport::infrastructure_failure(
                        selection
                            .commands
                            .first()
                            .map_or("validation", String::as_str),
                        format!("{error:#}"),
                    )
                });
        db.record_lifecycle_event(
            "validation_result",
            Some(task_id),
            Some(run_id),
            Some(agent_id),
            Some(&serde_json::to_string(&report)?),
        )?;
        db.record_lifecycle_event("validation_selection", Some(task_id), Some(run_id), Some(agent_id), Some(&serde_json::json!({
            "selected_groups": selection.groups, "selected_commands": selection.commands,
            "rationale": selection.rationale,
            "worktree_fingerprint": crate::automated::revision_worktree_fingerprint(&current),
        }).to_string()))?;
        if report.is_success() || report.is_infrastructure_failure() {
            return Ok((report, selection));
        }
        if repair_attempt >= MAX_VALIDATION_REPAIRS {
            if let Some(previous) = last_repair_resolution.as_ref() {
                persist_escalation_decision(
                    db,
                    task_id,
                    crate::scheduler::EscalationObservation::ValidationRepairNonConvergence,
                    previous,
                    1,
                )?;
            }
            return Ok((report, selection));
        }
        repair_attempt += 1;
        let task = db
            .get_task(task_id)?
            .context("validation repair task disappeared")?;
        let packet = crate::execution_packet::ValidationRepairPacket::build(
            worktree,
            &task,
            repair_attempt,
            &report,
            &current,
        )?;
        let repair = crate::execution_packet::render_packet(
            "Fix only the current deterministic validation failures in this packet. Preserve the existing worktree and avoid broad or unrelated changes. Do not run validation commands; Orc will rerun them after this bounded same-agent repair.",
            &packet,
        )?;
        db.record_lifecycle_event(
            "validation_repair_started",
            Some(task_id),
            Some(run_id),
            Some(agent_id),
            Some(&serde_json::json!({"repair_attempt": repair_attempt}).to_string()),
        )?;
        let repair_resolution = repair_resolution_record(
            db,
            run_id,
            task_id,
            "validation_repair",
            ReasoningEffort::Low,
        )?;
        let invocation = start_provider_invocation_bounded(
            db,
            run_id,
            task_id,
            "validation_repair",
            repair_attempt,
            &repair_resolution,
        )?;
        let repair_execution = worker.execute_repair_with_progress_and_usage(
            &repair,
            worktree,
            &crate::worker_protocol::repair_completion_schema(),
            worker_output,
            cancellation,
        );
        db.finish_provider_invocation(
            invocation,
            if repair_execution.is_ok() {
                "completed"
            } else {
                "failed"
            },
            repair_execution
                .as_ref()
                .ok()
                .and_then(|value| value.token_usage),
        )?;
        last_repair_resolution = db.provider_resolution(invocation)?;
        let repair_execution = repair_execution
            .map_err(|error| anyhow::anyhow!("validation repair worker failed: {error}"))?;
        if let WorkerOutcome::Failure(error) = repair_execution.outcome {
            anyhow::bail!("validation repair worker failed: {error}");
        }
        if repair_execution.output.is_some() {
            *output = repair_execution.output;
        }
        merge_token_usage(token_usage, repair_execution.token_usage);
        db.record_lifecycle_event(
            "validation_repair_completed",
            Some(task_id),
            Some(run_id),
            Some(agent_id),
            Some(&serde_json::json!({"repair_attempt": repair_attempt}).to_string()),
        )?;
        progress(&format!("validation repair attempt {repair_attempt}"));
    }
}

fn validation_failure_message(report: &ValidationReport) -> String {
    if report.is_infrastructure_failure() {
        format!(
            "validation infrastructure failure; deterministic validation could not run:\n{}",
            report.summary()
        )
    } else {
        format!(
            "deterministic validation did not converge after {MAX_VALIDATION_REPAIRS} repairs:\n{}",
            report.summary()
        )
    }
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
        "# Orc Coder Instructions\n\n{precedence}\n\n## Engineering Contract\n\n{contract}\n\n---\n\n# Task\n\nProject: {project}\nTask ID: {id}\nTitle: {title}\nObjective: {objective}\nRole: {role}{execution_contract}\n\nInspect the repository rooted at the current working directory and implement ONLY the changes required to complete this single task. Stay within the specified scope; do not modify unrelated files or change task status. Do not run the project's validation/test suite, focused checks, or any other command to prove completion \u{2014} Orc runs the selected deterministic checks after this session ends. Stop as soon as the implementation is complete and summarize what you changed and any follow-up steps.\n",
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
    let db = Database::open(db_path)
        .with_context(|| format!("failed to open orc DB ({db_path}); run `orc init` first"))?;
    let task = db
        .get_task(task_id)?
        .with_context(|| format!("task '{task_id}' not found in DB"))?;
    let effort = task_contract_effort(&db, &task)?;
    let decision = crate::scheduler::resolve_task_economy_for_execution_with_refresher(
        &db,
        &task,
        crate::registry::AgentAction::Code,
        crate::scheduler::EconomyOverrides::default(),
        Some(registry::AUTOMATED),
        None,
        Some(effort),
        Some("task_contract".into()),
        crate::scheduler::TransportEligibility::IgnoreUnsupportedBackend,
        None,
        "injected_worker_dispatch",
        &HashSet::new(),
        &crate::scheduler::UnsupportedQuotaRefresher,
    )?;
    let resolution = decision.resolution.ok_or_else(|| {
        anyhow::anyhow!(
            "no eligible agent found for task '{task_id}': {}",
            decision.schedule.explanation
        )
    })?;
    let selected_agent = resolution.agent.id.clone();
    dispatch_with_worker_on_db_cancellable_resolved(
        task_id,
        worker,
        &db,
        repo_path,
        &selected_agent,
        &SystemValidationRunner,
        None,
        Some(resolution),
    )
    .map(|_| ())
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
    // executed by Orc after implementation. The parameter is retained for API
    // compatibility with existing callers.
    validation_runner: &dyn ValidationRunner,
    cancellation: Option<&crate::worker::CancellationControl>,
) -> Result<DispatchSummary> {
    dispatch_with_worker_on_db_cancellable_resolved(
        task_id,
        worker,
        db,
        repo_path,
        agent_id,
        validation_runner,
        cancellation,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the injected transport seam carries an optional authoritative resolution"
)]
fn dispatch_with_worker_on_db_cancellable_resolved(
    task_id: &str,
    worker: &dyn Worker,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
    cancellation: Option<&crate::worker::CancellationControl>,
    provided_resolution: Option<crate::scheduler::EconomyResolution>,
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

    let proposal_effort = task_contract_effort(db, &task)?;
    let resolution = match provided_resolution {
        Some(resolution) => resolution,
        None => {
            let decision = crate::scheduler::resolve_task_economy_for_execution_with_refresher(
                db,
                &task,
                crate::registry::AgentAction::Code,
                crate::scheduler::EconomyOverrides {
                    agent_id: Some(agent_id.into()),
                    ..crate::scheduler::EconomyOverrides::default()
                },
                Some(registry::AUTOMATED),
                None,
                Some(proposal_effort),
                Some("task_contract".into()),
                crate::scheduler::TransportEligibility::IgnoreUnsupportedBackend,
                None,
                "injected_worker_dispatch",
                &HashSet::new(),
                &crate::scheduler::UnsupportedQuotaRefresher,
            )?;
            decision.resolution.ok_or_else(|| {
                anyhow::anyhow!(
                    "agent '{agent_id}' is not eligible for dispatch: {}",
                    decision.schedule.explanation
                )
            })?
        }
    };

    // PREPARE is intentionally completed before task status, run, or worktree
    // mutation.  The snapshot is captured from the authoritative repository.
    let snapshot = git::inspect_worktree(repo_path, repo_path)
        .context("failed to inspect repository during Worker PREPARE")?;
    let (proposal, enforce_worker_protocol) =
        worker_task_contract(db, &task).context("persisted task contract is invalid")?;
    let acceptance_criteria =
        worker_requirements(&proposal.acceptance_criteria, "acceptance-criterion");
    let required_tests = worker_requirements(&proposal.required_tests, "required-test");
    // Configured project validation is owned by Orc, not the
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
                class: resolution.execution.class.as_str(),
                model: resolution.execution.model.as_deref(),
                effort: resolution.execution.reasoning_effort,
                source: &resolution.record.source,
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

    // Execute the worker in the worktree directory
    let worktree_dir = repo_path.join(&worktree_path);
    let before_plan = git::inspect_worktree(&worktree_dir, repo_path)
        .context("failed to inspect worktree before Worker plan")?;
    let task_contract = db
        .get_task_contract(task_id)?
        .unwrap_or_else(|| crate::task::TaskContract::defaults(&task.objective));
    let packet = crate::execution_packet::DispatchPacket::build(
        &worktree_dir,
        &project_name,
        &contract,
        &task,
        &task_contract,
        &plan,
        &before_plan,
    )?;
    let prompt = crate::execution_packet::render_packet(
        &format!(
            "{CODER_PROMPT_PRECEDENCE}\nImplement this task using only the authoritative bounded Dispatch packet below. Relevant source context and current worktree state were selected deterministically by Orc. Execute the persisted plan in order. Do not run the project's validation/test suite, acceptance checks, or reviewer-style verification; Orc owns deterministic validation. Return the required structured completion envelope; changed files are derived from the worktree, not self-report."
        ),
        &packet,
    )?;
    progress("worker spawned");
    progress("worker running");
    let invocation_id = start_provider_invocation_bounded(
        db,
        run_id,
        task_id,
        "implementation",
        1,
        &resolution.record,
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
                step_outputs[index] = Some(completion_step_evidence(reported));
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

                    if enforce_worker_protocol {
                        let mut completion_repair = 0;
                        loop {
                            let failed_step =
                                plan.steps.iter().enumerate().find_map(|(index, step)| {
                                    validate_worker_step_completion(step, step_snapshots.get(index))
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
                            let repair_resolution = repair_resolution_record(
                                db,
                                run_id,
                                task_id,
                                "completion_repair",
                                ReasoningEffort::Low,
                            )?;
                            let repair_invocation = start_provider_invocation_bounded(
                                db,
                                run_id,
                                task_id,
                                "completion_repair",
                                completion_repair,
                                &repair_resolution,
                            )?;
                            let repaired = worker.execute_planned_step_repair(
                                &plan.steps[index],
                                &repair_prompt,
                                &worktree_dir,
                                &crate::worker_protocol::repair_completion_schema(),
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
                                let repair_evidence = normalized_completion_step_output(
                                    &repair_output,
                                    &plan.steps[index].id,
                                )
                                .unwrap_or(repair_output);
                                step_outputs[index] = Some(merge_completion_step_output(
                                    step_outputs[index].as_deref(),
                                    repair_evidence,
                                ));
                            }
                            if repaired.token_usage.is_some() {
                                merge_token_usage(&mut token_usage, repaired.token_usage);
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
                    let mut changes = match git::inspect_worktree(&worktree_dir, repo_path) {
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
                    let validation_config =
                        crate::validation::ValidationConfig::load(&worktree_dir)?;
                    let required_validation = db
                        .get_task_contract(task_id)?
                        .map(|contract| contract.validation)
                        .unwrap_or_default();
                    let (report, selection) = run_task_validation_repair_loop(
                        validation_runner,
                        &validation_config,
                        &required_validation,
                        worker,
                        db,
                        &worktree_dir,
                        repo_path,
                        task_id,
                        run_id,
                        agent_id,
                        &mut output,
                        &mut token_usage,
                        &progress,
                        &worker_output,
                        cancellation,
                    )?;
                    changes = git::inspect_worktree(&worktree_dir, repo_path)?;
                    db.store_change_evidence(run_id, &changes)?;
                    if !report.is_success() {
                        let message = validation_failure_message(&report);
                        db.update_agent_run_status_with_usage(
                            run_id,
                            "failed",
                            Some(&message),
                            token_usage,
                        )?;
                        db.update_task_status(task_id, TaskStatus::Blocked)?;
                        anyhow::bail!(message);
                    }
                    let combined_output = output.clone().unwrap_or_default();
                    for decision in architecture_decisions(&combined_output) {
                        db.insert_approval_request(project_id, decision)
                            .with_context(
                                || "failed to record architecture decision approval request",
                            )?;
                    }
                    let performed_operations = planned_operations_for_plan(&plan.steps);
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
                                        &[],
                                        false,
                                    ),
                                    verification: Vec::new(),
                                    passed: true,
                                }
                            })
                            .collect(),
                        configured_validation: selection.commands.clone(),
                        unresolved_issues: Vec::new(),
                    };
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
                        validation: "passed".to_owned(),
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
        crate::scheduler::TransportEligibility::IgnoreUnsupportedBackend,
        true,
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
        crate::scheduler::TransportEligibility::Strict,
        true,
        factory,
    )
}

/// Revision entry point for a workflow-owned agent reservation. The selected
/// agent remains an eligibility constraint and is not relabeled as an
/// operator override in the final resolution record.
#[expect(
    clippy::too_many_arguments,
    reason = "keeps the CLI revision seam explicit"
)]
pub fn revise_with_factory_and_global_db_as_constrained_with_runner<F>(
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
        crate::scheduler::TransportEligibility::Strict,
        false,
        factory,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared revision resolution boundary"
)]
pub(crate) fn revise_with_factory_on_db_as_with_runner<F>(
    task_id: &str,
    feedback: &str,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
    overrides: &RevisionExecutionOverrides,
    transport: crate::scheduler::TransportEligibility,
    operator_agent_override: bool,
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
    let source_review_id = db
        .actionable_revision_review(task_id)?
        .map(|(id, _)| id)
        .context("task has no actionable revision review")?;
    let revision_effort = effective_revision_effort(db, &task, source_review_id, overrides.effort)?;
    let escalation_request = semantic_escalation_request(
        db,
        &task,
        source_review_id,
        overrides,
        operator_agent_override,
    )?;
    let resolution = resolve_revision_economy(
        db,
        &task,
        agent_id,
        overrides,
        revision_effort,
        transport,
        operator_agent_override,
        escalation_request,
        &crate::scheduler::ProviderQuotaRefresher,
    )?;
    let resolved_agent_id = resolution.agent.id.clone();
    let worker = factory(
        &resolution.agent,
        resolution.execution.model.clone(),
        resolution.execution.reasoning_effort,
    )
    .map_err(anyhow::Error::msg)?;
    revise_with_worker_on_db_with_overrides_resolved(
        task_id,
        feedback,
        worker.as_ref(),
        db,
        repo_path,
        &resolved_agent_id,
        validation_runner,
        overrides,
        Some(resolution),
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
    // executed by Orc after revision. The parameter is retained for API
    // compatibility with existing callers.
    validation_runner: &dyn ValidationRunner,
    overrides: &RevisionExecutionOverrides,
) -> Result<DispatchSummary> {
    revise_with_worker_on_db_with_overrides_resolved(
        task_id,
        feedback,
        worker,
        db,
        repo_path,
        agent_id,
        validation_runner,
        overrides,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the injected transport seam carries an optional authoritative resolution"
)]
fn revise_with_worker_on_db_with_overrides_resolved(
    task_id: &str,
    feedback: &str,
    worker: &dyn Worker,
    db: &Database,
    repo_path: impl AsRef<Path>,
    agent_id: &str,
    validation_runner: &dyn ValidationRunner,
    overrides: &RevisionExecutionOverrides,
    provided_resolution: Option<crate::scheduler::EconomyResolution>,
) -> Result<DispatchSummary> {
    let repo_path = repo_path.as_ref();
    let contract = contract::load_contract(repo_path.join(ENGINEERING_CONTRACT_PATH))?;
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    let project_name = db.get_project_name()?.unwrap_or_else(|| "orc".into());
    let task = db.get_task(task_id)?.context("task not found in DB")?;
    if !matches!(
        task.status,
        TaskStatus::RevisionRequired | TaskStatus::Blocked
    ) {
        anyhow::bail!(
            "task {} can only be revised from revision_required or blocked (currently {})",
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
    let resolution = match provided_resolution {
        Some(resolution) => resolution,
        None => resolve_revision_economy(
            db,
            &task,
            agent_id,
            overrides,
            revision_effort,
            crate::scheduler::TransportEligibility::IgnoreUnsupportedBackend,
            true,
            semantic_escalation_request(db, &task, source_review_id, overrides, true)?,
            &crate::scheduler::UnsupportedQuotaRefresher,
        )?,
    };
    let run_id = db.create_agent_run_with_execution(
        project_id,
        task_id,
        agent_id,
        registry::AUTOMATED,
        crate::storage::AgentRunExecution {
            class: resolution.execution.class.as_str(),
            model: resolution.execution.model.as_deref(),
            effort: resolution.execution.reasoning_effort,
            source: &resolution.record.source,
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
    let extra_feedback = if !feedback.trim().is_empty()
        && !contract_already_contains_feedback(&revision_contract, feedback)
    {
        Some(feedback)
    } else {
        None
    };
    let packet = crate::execution_packet::RevisionPacket::build(
        &worktree_dir,
        &project_name,
        &contract,
        &task,
        &revision_contract,
        extra_feedback,
        &revision_snapshot,
        &revision_plan,
    )?;
    let prompt = crate::execution_packet::render_packet(
        &format!(
            "{CODER_PROMPT_PRECEDENCE}\nFix only the active unresolved or regressed blockers in this bounded Revision packet. Preserve unrelated and already-correct behavior. Relevant files and current changes were selected deterministically by Orc; avoid broad repository discovery. Execute the persisted revision plan in order. Do not run the project's validation/test suite or reviewer checks; Orc owns them. Return the required structured revision handoff, while Orc treats worktree evidence as authoritative for changed files."
        ),
        &packet,
    )?;
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
        start_provider_invocation_bounded(db, run_id, task_id, "revision", 1, &resolution.record)?;
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
                    revision_step_outputs[index] = Some(completion_step_evidence(reported));
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
    let mut token_usage = execution.token_usage;
    let fail = |message: String, usage: Option<TokenUsage>| -> Result<DispatchSummary> {
        progress(if message.to_ascii_lowercase().contains("timeout") {
            "worker timeout"
        } else {
            "revision failed"
        });
        db.update_agent_run_status_with_usage(run_id, "failed", Some(&message), usage)?;
        db.update_task_status(task_id, TaskStatus::Blocked)?;
        anyhow::bail!("{message}")
    };
    if let WorkerOutcome::Failure(error) = outcome {
        progress("worker failed");
        return fail(format!("Worker failed: {error}"), token_usage);
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
                    validate_worker_step_completion(step, revision_step_snapshots.get(index))
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
                return fail(
                    format!("Worker revision completion self-check failed: {error:#}"),
                    token_usage,
                );
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
            let repair_resolution = repair_resolution_record(
                db,
                run_id,
                task_id,
                "completion_repair",
                ReasoningEffort::Low,
            )?;
            let repair_invocation = start_provider_invocation_bounded(
                db,
                run_id,
                task_id,
                "completion_repair",
                completion_repair,
                &repair_resolution,
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
            merge_token_usage(&mut token_usage, repaired.token_usage);
            if let WorkerOutcome::Failure(error) = repaired.outcome {
                return fail(
                    format!("Worker completion repair failed: {error}"),
                    token_usage,
                );
            }
            if let Some(repair_output) = repaired.output {
                let repair_evidence = normalized_completion_step_output(
                    &repair_output,
                    &revision_plan.steps[index].id,
                )
                .unwrap_or(repair_output);
                revision_step_outputs[index] = Some(merge_completion_step_output(
                    revision_step_outputs[index].as_deref(),
                    repair_evidence,
                ));
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
    let mut changes = match git::inspect_worktree(&worktree_dir, repo_path) {
        Ok(current) => git::changes_since(&baseline_changes, &current),
        Err(error) => {
            return fail(
                format!("Post-worker inspection failed: {error:#}"),
                token_usage,
            );
        }
    };
    if changes.files.is_empty() {
        return fail(
            "Revision completed without meaningful project changes.".into(),
            token_usage,
        );
    }
    db.store_change_evidence(run_id, &changes)?;
    let validation_config = crate::validation::ValidationConfig::load(&worktree_dir)?;
    let required_validation = db
        .get_task_contract(task_id)?
        .map(|contract| contract.validation)
        .unwrap_or_default();
    let (report, selection) = run_task_validation_repair_loop(
        validation_runner,
        &validation_config,
        &required_validation,
        worker,
        db,
        &worktree_dir,
        repo_path,
        task_id,
        run_id,
        agent_id,
        &mut output,
        &mut token_usage,
        &progress,
        &worker_output,
        None,
    )?;
    let current = git::inspect_worktree(&worktree_dir, repo_path)?;
    changes = git::changes_since(&baseline_changes, &current);
    db.store_change_evidence(run_id, &changes)?;
    if !report.is_success() {
        return fail(validation_failure_message(&report), token_usage);
    }
    let combined = output.clone().unwrap_or_default();
    let performed_operations = planned_operations_for_plan(&revision_plan.steps);
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
                        &[],
                        false,
                    ),
                    verification: Vec::new(),
                    passed: true,
                }
            })
            .collect(),
        configured_validation: selection.commands.clone(),
        unresolved_issues: Vec::new(),
    };
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
    // Consume and link the source review only after the revision has produced
    // inspectable changes and current deterministic validation evidence.
    if !db.complete_revision_run_for_review(
        task_id,
        run_id,
        source_review_id,
        contract_id,
        &combined,
        token_usage,
    )? {
        return fail(
            "Revision review was consumed before this revision completed.".into(),
            token_usage,
        );
    }
    progress("review transition");
    Ok(DispatchSummary {
        task: db
            .get_task(task_id)?
            .context("task disappeared after revision")?,
        agent: agent_id.into(),
        backend: resolution.agent.backend.clone(),
        profile: resolution.agent.profile_path.clone(),
        model: resolution.execution.model.clone(),
        reasoning_effort: resolution.execution.reasoning_effort,
        worktree_path,
        run_id,
        run_status: "completed".into(),
        validation: "passed".into(),
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
    if !matches!(
        task.status,
        TaskStatus::RevisionRequired | TaskStatus::Blocked
    ) {
        anyhow::bail!(
            "task {} can only be revised from revision_required or blocked (currently {})",
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
    dispatch_selected_with_db_and_repo_authority(
        db,
        repo_path,
        task_id,
        requested_agent,
        true,
        model_override,
        effort_override,
        cancellation,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "automatic assignment and operator override have distinct provenance"
)]
fn dispatch_selected_with_db_and_repo_authority(
    db: &Database,
    repo_path: impl AsRef<Path>,
    task_id: &str,
    requested_agent: Option<&str>,
    operator_agent_override: bool,
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
    let has_execution_override = model_override.is_some() || effort_override.is_some();
    let pending_escalation =
        if operator_agent_override || model_override.is_some() || effort_override.is_some() {
            None
        } else {
            db.pending_escalation_request(task_id)?
                .map(|persisted| persisted.request)
        };
    let decision = crate::scheduler::resolve_task_economy_for_execution(
        db,
        &task,
        crate::registry::AgentAction::Code,
        crate::scheduler::EconomyOverrides {
            agent_id: if operator_agent_override {
                requested_agent.map(str::to_owned)
            } else {
                None
            },
            model: model_override,
            effort: effort_override,
        },
        None,
        if operator_agent_override || pending_escalation.is_some() {
            None
        } else {
            requested_agent.map(str::to_owned)
        },
        Some(task_effort),
        Some("task_contract".into()),
        crate::scheduler::TransportEligibility::Strict,
        pending_escalation,
        "task_dispatch",
    )?;
    let resolution = decision.resolution.ok_or_else(|| {
        anyhow::anyhow!(
            "no eligible agent found for task '{}': {}",
            task_id,
            decision.schedule.explanation
        )
    })?;
    let agent = resolution.agent.clone();
    if agent.execution_mode == registry::MANUAL {
        if has_execution_override {
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
    let model = resolution.execution.model.clone();
    let reasoning_effort = resolution.execution.reasoning_effort;
    let worker = WorkerFactory::build_with_overrides(&agent, model.clone(), reasoning_effort)
        .map_err(anyhow::Error::msg)?;
    let mut summary = dispatch_with_worker_on_db_cancellable_resolved(
        task_id,
        worker.as_ref(),
        db,
        repo_path,
        &agent.id,
        &SystemValidationRunner,
        cancellation,
        Some(resolution),
    )?;
    db.set_agent_run_profile(summary.run_id, agent.profile_path.as_deref())?;
    summary.backend = agent.backend;
    summary.profile = agent.profile_path;
    summary.model = model;
    summary.reasoning_effort = reasoning_effort;
    Ok(summary)
}

fn dispatch_automatic_assignment(task_id: &str, agent_id: &str) -> Result<DispatchSummary> {
    let db = Database::open_global(".orc/orc.db")?;
    dispatch_selected_with_db_and_repo_authority(
        &db,
        ".",
        task_id,
        Some(agent_id),
        false,
        None,
        None,
        None,
    )
}

pub fn plan_dispatch_assignments(
    ready: &[QueueEntry],
    agents: &[AgentDefinition],
    busy_agents: &HashSet<String>,
    quota_reserve: i64,
    concurrency: Option<usize>,
) -> Result<Vec<(String, String)>> {
    plan_dispatch_assignments_with_costs(
        ready,
        agents,
        busy_agents,
        quota_reserve,
        concurrency,
        &crate::registry::EconomyCostConfiguration::default(),
    )
}

pub fn plan_dispatch_assignments_with_costs(
    ready: &[QueueEntry],
    agents: &[AgentDefinition],
    busy_agents: &HashSet<String>,
    quota_reserve: i64,
    concurrency: Option<usize>,
    costs: &crate::registry::EconomyCostConfiguration,
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
        let profiles = std::collections::BTreeMap::new();
        let template = crate::execution::ExecutionTemplate::default();
        let decision = crate::scheduler::resolve_economy(crate::scheduler::EconomyResolverInput {
            action: crate::registry::AgentAction::Code,
            candidates: agents,
            task: Some(&entry.task),
            required_capabilities: &entry.task.required_capabilities(),
            requested_mode: Some(registry::AUTOMATED),
            busy_agents: &reserved,
            quota_reserve,
            quota_refresh_failures: &std::collections::BTreeMap::new(),
            overrides: crate::scheduler::EconomyOverrides::default(),
            constrained_agent_id: None,
            action_profiles: &profiles,
            execution_class: crate::execution::class_for_role(&entry.task.role),
            execution_template: &template,
            task_model: None,
            task_effort: entry.task.reasoning_effort,
            task_source: Some("task_contract".into()),
            policy_model: None,
            policy_effort: None,
            policy_source: None,
            cost_configuration: costs,
            transport_eligibility: crate::scheduler::TransportEligibility::Strict,
            escalation_request: None,
            lineage: "dispatch_queue_assignment".into(),
        })?
        .schedule;
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
    if concurrency == Some(0) {
        anyhow::bail!("concurrency must be greater than zero");
    }
    let mut reserved = db.list_busy_agents()?.into_iter().collect::<HashSet<_>>();
    let mut assignments = Vec::new();
    for entry in &report.ready {
        if concurrency.is_some_and(|limit| assignments.len() == limit) {
            break;
        }
        let pending_escalation = db
            .pending_escalation_request(&entry.task.id)?
            .map(|persisted| persisted.request);
        let decision = crate::scheduler::resolve_task_economy_for_execution_with_refresher(
            &db,
            &entry.task,
            crate::registry::AgentAction::Code,
            crate::scheduler::EconomyOverrides::default(),
            Some(registry::AUTOMATED),
            None,
            entry.task.reasoning_effort,
            Some("task_contract".into()),
            crate::scheduler::TransportEligibility::Strict,
            pending_escalation,
            "dispatch_queue_assignment",
            &reserved,
            &crate::scheduler::ProviderQuotaRefresher,
        )?;
        if let Some(agent_id) = decision.schedule.selected_agent_id {
            reserved.insert(agent_id.clone());
            assignments.push((entry.task.id.clone(), agent_id));
        }
    }
    let mut outcomes = BTreeMap::new();
    let handles = assignments
        .iter()
        .map(|(task_id, agent_id)| {
            let task_id = task_id.clone();
            let agent_id = agent_id.clone();
            thread::spawn(move || dispatch_automatic_assignment(&task_id, &agent_id))
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
    if task.status != TaskStatus::AcceptanceReady {
        anyhow::bail!(
            "task {} can only be accepted from acceptance_ready (currently {})",
            task_id,
            task.status
        );
    }
    let review_run = db
        .list_agent_runs_for_task(task_id)?
        .into_iter()
        .find(|run| run.execution_class == "review" && run.status == "completed")
        .context("acceptance-ready task has no completed review")?;
    let review: crate::automated::ReviewResult = serde_json::from_str(
        review_run
            .output
            .as_deref()
            .context("completed review has no verdict")?,
    )
    .context("completed review verdict is invalid")?;
    if !review.verdict.eq_ignore_ascii_case("pass") {
        db.update_task_status(task_id, TaskStatus::Review)?;
        anyhow::bail!(
            "task {} requires a current PASS review before acceptance (latest verdict: {})",
            task_id,
            review.verdict
        );
    }
    if db
        .list_agent_runs_for_task(task_id)?
        .into_iter()
        .any(|run| {
            run.id > review_run.id && run.execution_class != "review" && run.status == "completed"
        })
    {
        db.update_task_status(task_id, TaskStatus::Review)?;
        anyhow::bail!("task {task_id} PASS review is stale after a newer implementation");
    }
    let (branch, path) = db
        .get_worktree_metadata(task_id)?
        .context("task has no worktree")?;
    let worktree = repo_path.join(&path);
    if !worktree.exists() {
        anyhow::bail!("task worktree does not exist: {}", worktree.display());
    }
    let current_changes = git::inspect_worktree(&worktree, repo_path)?;
    if current_changes.files.is_empty() {
        anyhow::bail!("task {task_id} has no meaningful changes to accept");
    }
    let Some(reviewed_changes) = db.get_change_evidence(review_run.id)? else {
        db.update_task_status(task_id, TaskStatus::Review)?;
        anyhow::bail!("current PASS review has no change evidence");
    };
    if reviewed_changes != current_changes {
        db.update_task_status(task_id, TaskStatus::Review)?;
        anyhow::bail!("task {task_id} PASS review is stale because the implementation changed");
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
    db.complete_agent_run_for_review(&task_id, run_id, output, None)?;
    Ok(task_id)
}

pub fn fail_run(db: &Database, run_id: i64, reason: &str) -> Result<String> {
    let run = db.get_agent_run(run_id)?.context("run not found")?;
    if run.execution_mode != registry::MANUAL || run.status != "waiting_external" {
        anyhow::bail!("run {} is not a waiting manual run", run_id);
    }
    let task_id = db.fail_run(run_id, reason)?;
    Ok(task_id)
}

#[derive(Debug, Clone)]
pub struct PatchSubmissionOutcome {
    pub run_id: i64,
    pub task_id: String,
    pub worktree_path: PathBuf,
    pub branch_name: String,
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
    _validation_runner: &dyn ValidationRunner,
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

    // Manual patch submission records change evidence and leaves validation to
    // the current manual workflow; Task 9 will move it onto Orc's validation
    // boundary before Review.
    let success_output = format!(
        "Worktree: {}\nApplied: yes\n\nValidation: deferred to review\nPatch:\n{}",
        worktree_path.display(),
        patch_content
    );
    let changes = git::inspect_worktree(&absolute_worktree, repo_path)?;
    db.store_change_evidence(run_id, &changes)?;
    db.complete_agent_run_for_review(&task_id, run_id, &success_output, None)?;

    Ok(PatchSubmissionOutcome {
        run_id,
        task_id,
        worktree_path,
        branch_name,
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
    fn surviving_blockers_do_not_directly_promote_revision_effort() {
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
            let resolution = crate::registry::ResolutionRecord {
                selected_agent: "coder".into(),
                selected_model: Some("small".into()),
                effort: Some(effort),
                tier: crate::registry::EconomyTier::Default,
                source: "agent".into(),
                escalation_reason: None,
                input_lineage: "revision-test".into(),
                escalation: None,
            };
            let invocation = db
                .start_provider_invocation_with_resolution(run, "revision", 1, &resolution)
                .unwrap();
            db.finish_provider_invocation(invocation, "completed", None)
                .unwrap();
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
        assert_eq!(effort(second_review), ReasoningEffort::Low);
        let escalation = semantic_escalation_request(
            &db,
            &db.get_task(&task).unwrap().unwrap(),
            second_review,
            &RevisionExecutionOverrides::default(),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            escalation.lineage.trigger,
            crate::registry::EscalationTrigger::SemanticRevisionNonConvergence
        );
        assert_eq!(
            escalation.lineage.requested_minimum_tier,
            crate::registry::EconomyTier::Escalation
        );
        assert_eq!(
            effort_with_override(&db, &task, second_review, ReasoningEffort::Low),
            ReasoningEffort::Low
        );
        assert_eq!(
            effort_with_override(&db, &task, second_review, ReasoningEffort::High),
            ReasoningEffort::High
        );
        revision(&db, second_review, ReasoningEffort::Low);
        let third_review = review(&db);
        assert_eq!(effort(third_review), ReasoningEffort::Low);
        assert!(db.get_task_execution_condition(&task).unwrap().is_none());
    }

    #[test]
    fn non_convergence_recovery_rejects_invalid_attempts_without_mutation() {
        let (_dir, db, task) = setup();
        let before = db.get_task(&task).unwrap().unwrap();
        assert!(
            db.acknowledge_non_convergence_replan_required(&task)
                .is_err()
        );
        assert_eq!(db.get_task(&task).unwrap().unwrap(), before);
        assert!(db.get_task_execution_condition(&task).unwrap().is_none());

        db.set_task_execution_condition(&task, "other_condition", "{}")
            .unwrap();
        assert!(
            db.acknowledge_non_convergence_replan_required(&task)
                .is_err()
        );
        assert_eq!(
            db.get_task_execution_condition(&task)
                .unwrap()
                .unwrap()
                .kind,
            "other_condition"
        );

        db.update_task_status(&task, TaskStatus::Done).unwrap();
        db.set_task_execution_condition(&task, "non_convergence_replan_required", "{}")
            .unwrap();
        assert!(
            db.acknowledge_non_convergence_replan_required(&task)
                .is_err()
        );
        assert_eq!(
            db.get_task_execution_condition(&task)
                .unwrap()
                .unwrap()
                .kind,
            "non_convergence_replan_required"
        );
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
        assert!(runner.executed_commands().is_empty());

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

    fn completion_step(
        operations: Vec<crate::worker_protocol::PlannedOperation>,
    ) -> crate::worker_protocol::PlannedStep {
        crate::worker_protocol::PlannedStep {
            id: "step-1".into(),
            objective: "test checkpoint".into(),
            intent: "test completion protocol".into(),
            operations,
            operation_targets: vec![],
            acceptance_criteria: vec![],
            required_tests: vec![],
            active_review_blockers: vec![],
            verification: vec![],
        }
    }

    #[test]
    fn completion_repair_preserves_existing_step_evidence() {
        let step = completion_step(vec![crate::worker_protocol::PlannedOperation::Modify]);
        let repair = r#"{"completion":{"step_results":[{"step_id":"step-1","operations_performed":["modify"],"affected_files":[],"observed":["updated file"],"verification_passed":[]}],"summary":"repaired"}}"#;
        let normalized = normalized_completion_step_output(repair, &step.id).unwrap();
        let merged = merge_completion_step_output(Some("AFFECTED FILE: original.rs"), normalized);
        assert!(merged.contains("AFFECTED FILE: original.rs"));
        assert!(merged.contains("updated file"));
    }
}
