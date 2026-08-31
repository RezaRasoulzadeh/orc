use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::execution::{ExecutionClass, ExecutionResolution, ExecutionTemplate};
use crate::registry::{
    self, AgentAction, AgentActionProfile, AgentDefinition, EconomyCostConfiguration, EconomyTier,
    ReasoningEffort, ResolutionRecord,
};
use crate::storage::Database;
use crate::task::Task;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    Disabled,
    Unavailable { reason: Option<String> },
    UnsupportedBackend { backend: String },
    UnsupportedMode { mode: String },
    MissingCapability { capability: String },
    QuotaExhausted,
    QuotaReserve { remaining: i64, reserve: i64 },
    Busy,
    ModeMismatch { requested: String, actual: String },
    UnsupportedAction { action: String },
    AgentConstraint { selected: String },
}

impl RejectionReason {
    pub fn description(&self) -> String {
        match self {
            Self::Disabled => "disabled".to_string(),
            Self::Unavailable { reason: Some(r) } => format!("unavailable: {r}"),
            Self::Unavailable { reason: None } => "unavailable".to_string(),
            Self::UnsupportedBackend { backend } => format!("unsupported backend: {backend}"),
            Self::UnsupportedMode { mode } => format!("unsupported execution mode: {mode}"),
            Self::MissingCapability { capability } => format!("missing capability: {capability}"),
            Self::QuotaExhausted => "quota exhausted".to_string(),
            Self::QuotaReserve { remaining, reserve } => {
                format!(
                    "quota below automatic reserve ({remaining}% remaining, {reserve}% required)"
                )
            }
            Self::Busy => "busy".to_string(),
            Self::ModeMismatch { requested, actual } => {
                format!("mode mismatch (requested: {requested}, actual: {actual})")
            }
            Self::UnsupportedAction { action } => format!("unsupported action: {action}"),
            Self::AgentConstraint { selected } => {
                format!("agent selection constrained to: {selected}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Eligible,
    Rejected(RejectionReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateEvaluation {
    pub agent_id: String,
    pub backend: String,
    pub execution_mode: String,
    pub priority: i64,
    pub quota_remaining_percent: Option<i64>,
    pub quota_reset_at: Option<String>,
    pub capacity_score: Option<i64>,
    #[serde(default)]
    pub resolved_model: Option<String>,
    #[serde(default)]
    pub economy_tier: EconomyTier,
    pub status: CandidateStatus,
}

/// The only exception transport injection may make to normal eligibility.
/// It exists so tests and embedders can supply a transport for an otherwise
/// unknown backend name; every Orc-owned eligibility rule still applies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransportEligibility {
    #[default]
    Strict,
    IgnoreUnsupportedBackend,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EconomyOverrides {
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
}

#[derive(Clone, Debug)]
pub struct EconomyResolverInput<'a> {
    pub action: AgentAction,
    pub candidates: &'a [AgentDefinition],
    pub task: Option<&'a Task>,
    pub required_capabilities: &'a [String],
    pub requested_mode: Option<&'a str>,
    pub busy_agents: &'a HashSet<String>,
    pub quota_reserve: i64,
    pub overrides: EconomyOverrides,
    /// A non-operator constraint, used for an already-owned run or workflow.
    pub constrained_agent_id: Option<String>,
    pub action_profiles: &'a BTreeMap<String, AgentActionProfile>,
    pub execution_class: ExecutionClass,
    pub execution_template: &'a ExecutionTemplate,
    pub task_model: Option<String>,
    pub task_effort: Option<ReasoningEffort>,
    pub task_source: Option<String>,
    pub policy_model: Option<String>,
    pub policy_effort: Option<ReasoningEffort>,
    pub policy_source: Option<String>,
    pub cost_configuration: &'a EconomyCostConfiguration,
    pub transport_eligibility: TransportEligibility,
    pub escalation_reason: Option<String>,
    pub lineage: String,
}

#[derive(Clone, Debug)]
pub struct EconomyResolution {
    pub agent: AgentDefinition,
    pub execution: ExecutionResolution,
    pub record: ResolutionRecord,
}

#[derive(Clone, Debug)]
pub struct EconomyDecision {
    pub schedule: ScheduleDecision,
    pub resolution: Option<EconomyResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReason {
    CheapestEconomyTier,
    HighestPriority,
    HealthierCapacity,
    LexicographicTieBreak,
    SingleEligibleCandidate,
    NoEligibleCandidates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleDecision {
    pub task_id: String,
    pub selected_agent_id: Option<String>,
    pub candidates: Vec<CandidateEvaluation>,
    pub selection_reason: SelectionReason,
    pub explanation: String,
}

impl ScheduleDecision {
    pub fn format_explanation(&self) -> String {
        let mut out = String::new();
        if let Some(ref selected) = self.selected_agent_id {
            out.push_str(&format!("Selected: {}\n\n", selected));
        } else {
            out.push_str("Selected: (none)\n\n");
        }
        out.push_str("Candidates:\n\n");
        for cand in &self.candidates {
            out.push_str(&format!("{}\n", cand.agent_id));
            match &cand.status {
                CandidateStatus::Eligible => {
                    out.push_str("  ELIGIBLE\n");
                    out.push_str(&format!("  mode: {}\n", cand.execution_mode));
                    out.push_str(&format!("  priority: {}\n", cand.priority));
                    out.push_str(&format!("  economy tier: {}\n", cand.economy_tier.as_str()));
                    if let Some(model) = &cand.resolved_model {
                        out.push_str(&format!("  model: {model}\n"));
                    }
                    let quota_str = cand
                        .quota_remaining_percent
                        .map(|q| format!("{q}%"))
                        .unwrap_or_else(|| "unknown".to_string());
                    out.push_str(&format!("  quota: {}\n", quota_str));
                }
                CandidateStatus::Rejected(reason) => {
                    out.push_str("  REJECTED\n");
                    out.push_str(&format!("  {}\n", reason.description()));
                }
            }
            out.push('\n');
        }
        out.push_str(&format!("Reason:\n{}", self.explanation));
        out
    }
}

pub fn is_backend_mode_supported(backend: &str, execution_mode: &str) -> bool {
    if registry::validate_backend(backend).is_err() {
        return false;
    }
    match execution_mode {
        registry::AUTOMATED => matches!(backend, "copilot" | "codex" | "antigravity"),
        registry::MANUAL => true,
        _ => false,
    }
}

pub fn evaluate_candidate(
    agent: &AgentDefinition,
    task: &Task,
    requested_mode: Option<&str>,
) -> CandidateEvaluation {
    evaluate_candidate_with_quota_reserve(agent, task, requested_mode, 0)
}

pub fn evaluate_candidate_with_quota_reserve(
    agent: &AgentDefinition,
    task: &Task,
    requested_mode: Option<&str>,
    quota_reserve: i64,
) -> CandidateEvaluation {
    evaluate_candidate_for_requirements(
        agent,
        &task.required_capabilities(),
        requested_mode,
        quota_reserve,
        TransportEligibility::Strict,
    )
}

fn evaluate_candidate_for_requirements(
    agent: &AgentDefinition,
    required: &[String],
    requested_mode: Option<&str>,
    quota_reserve: i64,
    transport_eligibility: TransportEligibility,
) -> CandidateEvaluation {
    let make_eval = |status: CandidateStatus| CandidateEvaluation {
        agent_id: agent.id.clone(),
        backend: agent.backend.clone(),
        execution_mode: agent.execution_mode.clone(),
        priority: agent.priority,
        quota_remaining_percent: agent.quota_remaining_percent,
        quota_reset_at: agent.quota_reset_at.clone(),
        capacity_score: capacity_score(agent),
        resolved_model: None,
        economy_tier: EconomyTier::Unknown,
        status,
    };

    // 1. agent must be enabled
    if !agent.enabled {
        return make_eval(CandidateStatus::Rejected(RejectionReason::Disabled));
    }

    // 2. configured availability must be available
    if agent.status != registry::AVAILABLE {
        return make_eval(CandidateStatus::Rejected(RejectionReason::Unavailable {
            reason: agent.unavailable_reason.clone(),
        }));
    }

    // 3. backend/mode combination must be supported
    let backend_supported = registry::validate_backend(&agent.backend).is_ok();
    if !backend_supported && transport_eligibility != TransportEligibility::IgnoreUnsupportedBackend
    {
        return make_eval(CandidateStatus::Rejected(
            RejectionReason::UnsupportedBackend {
                backend: agent.backend.clone(),
            },
        ));
    }
    let mode_supported = match agent.execution_mode.as_str() {
        registry::AUTOMATED => {
            (backend_supported
                && matches!(agent.backend.as_str(), "copilot" | "codex" | "antigravity"))
                || transport_eligibility == TransportEligibility::IgnoreUnsupportedBackend
        }
        registry::MANUAL => {
            backend_supported
                || transport_eligibility == TransportEligibility::IgnoreUnsupportedBackend
        }
        _ => false,
    };
    if !mode_supported {
        return make_eval(CandidateStatus::Rejected(
            RejectionReason::UnsupportedMode {
                mode: agent.execution_mode.clone(),
            },
        ));
    }

    // 4. required task capabilities must be satisfied
    let available = agent
        .capabilities
        .iter()
        .map(|value| crate::registry::AgentCapability::parse(value))
        .collect::<std::collections::HashSet<_>>();
    let missing: Vec<String> = required
        .iter()
        .filter(|cap| !available.contains(&crate::registry::AgentCapability::parse(cap)))
        .map(|cap| {
            crate::registry::AgentCapability::parse(cap)
                .as_str()
                .to_owned()
        })
        .collect();
    if !missing.is_empty() {
        return make_eval(CandidateStatus::Rejected(
            RejectionReason::MissingCapability {
                capability: missing.join(", "),
            },
        ));
    }

    // 5. known quota_remaining_percent == 0 excludes the agent
    if agent.quota_remaining_percent == Some(0) {
        return make_eval(CandidateStatus::Rejected(RejectionReason::QuotaExhausted));
    }
    if quota_reserve > 0
        && agent
            .quota_remaining_percent
            .is_some_and(|remaining| remaining < quota_reserve)
    {
        return make_eval(CandidateStatus::Rejected(RejectionReason::QuotaReserve {
            remaining: agent.quota_remaining_percent.unwrap(),
            reserve: quota_reserve,
        }));
    }

    // 6. optional requested execution mode must match
    if let Some(req_mode) = requested_mode
        && agent.execution_mode != req_mode
    {
        return make_eval(CandidateStatus::Rejected(RejectionReason::ModeMismatch {
            requested: req_mode.to_string(),
            actual: agent.execution_mode.clone(),
        }));
    }

    make_eval(CandidateStatus::Eligible)
}

fn capacity_score(agent: &AgentDefinition) -> Option<i64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| duration.as_secs() as i64);
    capacity_score_at(agent, now)
}

fn capacity_score_at(agent: &AgentDefinition, now_epoch: i64) -> Option<i64> {
    let remaining = agent.quota_remaining_percent?;
    let horizon_bonus = agent
        .quota_reset_at
        .as_deref()
        .and_then(parse_rfc3339_epoch)
        .map(|reset| reset - now_epoch)
        .filter(|seconds| *seconds > 0)
        .map(|seconds| (2_592_000i64.saturating_sub(seconds) / 86_400).min(30))
        .unwrap_or(0);
    Some(remaining.saturating_add(horizon_bonus))
}

fn capacity_value(capacity_score: Option<i64>) -> i64 {
    capacity_score.unwrap_or(0)
}

fn ranking_score(candidate: &CandidateEvaluation) -> i64 {
    candidate.priority * 10 + capacity_value(candidate.capacity_score)
}

fn selection_reason(eligible: &[CandidateEvaluation]) -> SelectionReason {
    if eligible.len() == 1 {
        SelectionReason::SingleEligibleCandidate
    } else if ranking_score(&eligible[0]) > ranking_score(&eligible[1])
        && capacity_value(eligible[0].capacity_score) > capacity_value(eligible[1].capacity_score)
        && eligible[0].priority <= eligible[1].priority
    {
        SelectionReason::HealthierCapacity
    } else if eligible[0].priority > eligible[1].priority {
        SelectionReason::HighestPriority
    } else {
        SelectionReason::LexicographicTieBreak
    }
}

fn execution_class_for_action(action: AgentAction) -> ExecutionClass {
    match action {
        AgentAction::Code => ExecutionClass::Coder,
        AgentAction::Review => ExecutionClass::Reviewer,
        AgentAction::Plan => ExecutionClass::Architect,
        AgentAction::Lead => ExecutionClass::General,
    }
}

fn candidate_execution(
    input: &EconomyResolverInput<'_>,
    agent: &AgentDefinition,
) -> Result<ExecutionResolution> {
    if input
        .overrides
        .model
        .as_deref()
        .is_some_and(|model| model.trim().is_empty())
    {
        bail!("model override must not be empty");
    }
    let mut resolution = crate::execution::resolve_with_template(
        input.execution_class.as_str(),
        input.execution_template,
        agent.model.as_deref(),
        agent.reasoning_effort,
        None,
        None,
    );
    if input.task_model.is_some() || input.task_effort.is_some() {
        if let Some(model) = &input.task_model {
            resolution.model = Some(model.clone());
        }
        if let Some(effort) = input.task_effort {
            resolution.reasoning_effort = Some(effort);
        }
        resolution.source = input
            .task_source
            .clone()
            .unwrap_or_else(|| "task_hint".into());
    }
    if let Some(profile) = input.action_profiles.get(&agent.id) {
        // Registry insertion mirrors agent defaults into each supported action
        // for compatibility. Only a profile that differs from those defaults
        // is an action-specific input and may outrank the template.
        let action_specific =
            profile.model != agent.model || profile.reasoning_effort != agent.reasoning_effort;
        if action_specific && (profile.model.is_some() || profile.reasoning_effort.is_some()) {
            if let Some(model) = &profile.model {
                resolution.model = Some(model.clone());
            }
            if let Some(effort) = profile.reasoning_effort {
                resolution.reasoning_effort = Some(effort);
            }
            resolution.source = "action_profile".into();
        }
    }
    if input.policy_model.is_some() || input.policy_effort.is_some() {
        if let Some(model) = &input.policy_model {
            resolution.model = Some(model.clone());
        }
        if let Some(effort) = input.policy_effort {
            resolution.reasoning_effort = Some(effort);
        }
        resolution.source = input
            .policy_source
            .clone()
            .unwrap_or_else(|| "policy_constraint".into());
    }
    if input.overrides.agent_id.is_some()
        || input.overrides.model.is_some()
        || input.overrides.effort.is_some()
    {
        if let Some(model) = &input.overrides.model {
            resolution.model = Some(model.clone());
        }
        if let Some(effort) = input.overrides.effort {
            resolution.reasoning_effort = Some(effort);
        }
        resolution.source = "operator_override".into();
    }
    Ok(resolution)
}

/// Resolve eligibility, execution identity, and economy ordering in one place.
/// Callers may supply constraints and policy inputs, but this function alone
/// creates the final provider-independent [`ResolutionRecord`].
pub fn resolve_economy(input: EconomyResolverInput<'_>) -> Result<EconomyDecision> {
    let selected_constraint = input
        .overrides
        .agent_id
        .as_ref()
        .or(input.constrained_agent_id.as_ref());
    let mut eligible = Vec::<(CandidateEvaluation, AgentDefinition, ExecutionResolution)>::new();
    let mut rejected = Vec::new();
    for agent in input.candidates {
        if let Some(selected) = selected_constraint
            && &agent.id != selected
        {
            let mut evaluation = evaluate_candidate_for_requirements(
                agent,
                input.required_capabilities,
                input.requested_mode,
                input.quota_reserve,
                input.transport_eligibility,
            );
            evaluation.status = CandidateStatus::Rejected(RejectionReason::AgentConstraint {
                selected: selected.clone(),
            });
            rejected.push(evaluation);
            continue;
        }
        let mut evaluation = evaluate_candidate_for_requirements(
            agent,
            input.required_capabilities,
            input.requested_mode,
            input.quota_reserve,
            input.transport_eligibility,
        );
        if matches!(evaluation.status, CandidateStatus::Eligible)
            && !agent.supports_action(input.action)
        {
            evaluation.status = CandidateStatus::Rejected(RejectionReason::UnsupportedAction {
                action: input.action.as_str().into(),
            });
        }
        if matches!(evaluation.status, CandidateStatus::Eligible)
            && input.busy_agents.contains(&agent.id)
        {
            evaluation.status = CandidateStatus::Rejected(RejectionReason::Busy);
        }
        if matches!(evaluation.status, CandidateStatus::Eligible) {
            let execution = candidate_execution(&input, agent)?;
            evaluation.resolved_model = execution.model.clone();
            evaluation.economy_tier = input
                .cost_configuration
                .tier_for(execution.model.as_deref());
            eligible.push((evaluation, agent.clone(), execution));
        } else {
            rejected.push(evaluation);
        }
    }
    eligible.sort_by(|(left, _, _), (right, _, _)| {
        left.economy_tier
            .rank()
            .cmp(&right.economy_tier.rank())
            .then_with(|| ranking_score(right).cmp(&ranking_score(left)))
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    rejected.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));

    let selected_agent_id = eligible.first().map(|(item, _, _)| item.agent_id.clone());
    let selection_reason = match eligible.as_slice() {
        [] => SelectionReason::NoEligibleCandidates,
        [_] => SelectionReason::SingleEligibleCandidate,
        [first, second, ..] if first.0.economy_tier != second.0.economy_tier => {
            SelectionReason::CheapestEconomyTier
        }
        _ => selection_reason(
            &eligible
                .iter()
                .map(|(evaluation, _, _)| evaluation.clone())
                .collect::<Vec<_>>(),
        ),
    };
    let task_id = input
        .task
        .map(|task| task.id.clone())
        .unwrap_or_else(|| format!("action:{}", input.action.as_str()));
    let explanation = match eligible.first() {
        Some((winner, _, _)) if selection_reason == SelectionReason::CheapestEconomyTier => {
            format!(
                "{} selected from the cheapest eligible economy tier ({}).",
                winner.agent_id,
                winner.economy_tier.as_str()
            )
        }
        Some((winner, _, _)) => format!(
            "{} selected by deterministic capacity, priority, and lexicographic order within economy tier {}.",
            winner.agent_id,
            winner.economy_tier.as_str()
        ),
        None => format!("No eligible agent satisfies requirements for '{task_id}'."),
    };

    let resolution = eligible.first().map(|(evaluation, agent, execution)| {
        let input_lineage = serde_json::json!({
            "lineage": input.lineage,
            "action": input.action.as_str(),
            "task_id": input.task.map(|task| task.id.as_str()),
            "execution_class": input.execution_class.as_str(),
            "requested_mode": input.requested_mode,
            "operator_agent": input.overrides.agent_id,
            "operator_model": input.overrides.model,
            "operator_effort": input.overrides.effort.map(ReasoningEffort::as_str),
            "task_model": input.task_model,
            "task_effort": input.task_effort.map(ReasoningEffort::as_str),
            "policy_model": input.policy_model,
            "policy_effort": input.policy_effort.map(ReasoningEffort::as_str),
            "source": execution.source,
        })
        .to_string();
        EconomyResolution {
            agent: agent.clone(),
            execution: execution.clone(),
            record: ResolutionRecord {
                selected_agent: agent.id.clone(),
                selected_model: execution.model.clone(),
                effort: execution.reasoning_effort,
                tier: evaluation.economy_tier,
                source: execution.source.clone(),
                escalation_reason: input.escalation_reason.clone(),
                input_lineage,
            },
        }
    });
    let mut candidates = eligible
        .into_iter()
        .map(|(evaluation, _, _)| evaluation)
        .collect::<Vec<_>>();
    candidates.extend(rejected);
    Ok(EconomyDecision {
        schedule: ScheduleDecision {
            task_id,
            selected_agent_id,
            candidates,
            selection_reason,
            explanation,
        },
        resolution,
    })
}

fn parse_rfc3339_epoch(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |start: usize, end: usize| value.get(start..end)?.parse::<i64>().ok();
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    let offset_start = value[19..].find(['Z', '+', '-']).map(|index| index + 19)?;
    let offset = if bytes[offset_start] == b'Z' {
        0
    } else {
        let sign = if bytes[offset_start] == b'+' { 1 } else { -1 };
        let offset_hour = value
            .get(offset_start + 1..offset_start + 3)?
            .parse::<i64>()
            .ok()?;
        let offset_minute = value
            .get(offset_start + 4..offset_start + 6)?
            .parse::<i64>()
            .ok()?;
        sign * (offset_hour * 3600 + offset_minute * 60)
    };
    let days = (1970..year)
        .map(|y| {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                366
            } else {
                365
            }
        })
        .sum::<i64>()
        + [
            31,
            28 + if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                1
            } else {
                0
            },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ][..(month - 1) as usize]
            .iter()
            .sum::<i64>()
        + day
        - 1;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

pub fn evaluate_candidate_with_busy(
    agent: &AgentDefinition,
    task: &Task,
    requested_mode: Option<&str>,
    busy_agents: &HashSet<String>,
) -> CandidateEvaluation {
    evaluate_candidate_with_busy_and_quota_reserve(agent, task, requested_mode, busy_agents, 0)
}

pub fn evaluate_candidate_with_busy_and_quota_reserve(
    agent: &AgentDefinition,
    task: &Task,
    requested_mode: Option<&str>,
    busy_agents: &HashSet<String>,
    quota_reserve: i64,
) -> CandidateEvaluation {
    let evaluation =
        evaluate_candidate_with_quota_reserve(agent, task, requested_mode, quota_reserve);
    if matches!(evaluation.status, CandidateStatus::Eligible) && busy_agents.contains(&agent.id) {
        CandidateEvaluation {
            status: CandidateStatus::Rejected(RejectionReason::Busy),
            ..evaluation
        }
    } else {
        evaluation
    }
}

pub fn schedule(
    task: &Task,
    agents: &[AgentDefinition],
    requested_mode: Option<&str>,
) -> Result<ScheduleDecision> {
    schedule_with_quota_reserve(task, agents, requested_mode, 0)
}

pub fn schedule_with_quota_reserve(
    task: &Task,
    agents: &[AgentDefinition],
    requested_mode: Option<&str>,
    quota_reserve: i64,
) -> Result<ScheduleDecision> {
    schedule_with_busy_and_quota_reserve(
        task,
        agents,
        requested_mode,
        &HashSet::new(),
        quota_reserve,
    )
}

pub fn schedule_with_busy(
    task: &Task,
    agents: &[AgentDefinition],
    requested_mode: Option<&str>,
    busy_agents: &HashSet<String>,
) -> Result<ScheduleDecision> {
    schedule_with_busy_and_quota_reserve(task, agents, requested_mode, busy_agents, 0)
}

pub fn schedule_with_busy_and_quota_reserve(
    task: &Task,
    agents: &[AgentDefinition],
    requested_mode: Option<&str>,
    busy_agents: &HashSet<String>,
    quota_reserve: i64,
) -> Result<ScheduleDecision> {
    let profiles = BTreeMap::new();
    let template = ExecutionTemplate::default();
    let costs = EconomyCostConfiguration::default();
    Ok(resolve_economy(EconomyResolverInput {
        action: AgentAction::Code,
        candidates: agents,
        task: Some(task),
        required_capabilities: &task.required_capabilities(),
        requested_mode,
        busy_agents,
        quota_reserve,
        overrides: EconomyOverrides::default(),
        constrained_agent_id: None,
        action_profiles: &profiles,
        execution_class: crate::execution::class_for_role(&task.role),
        execution_template: &template,
        task_model: None,
        task_effort: task.reasoning_effort,
        task_source: Some("task_contract".into()),
        policy_model: None,
        policy_effort: None,
        policy_source: None,
        cost_configuration: &costs,
        transport_eligibility: TransportEligibility::Strict,
        escalation_reason: None,
        lineage: "scheduler".into(),
    })?
    .schedule)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the authoritative boundary keeps every policy input explicit"
)]
pub fn resolve_task_economy(
    db: &Database,
    task: &Task,
    action: AgentAction,
    overrides: EconomyOverrides,
    requested_mode: Option<&str>,
    constrained_agent_id: Option<String>,
    task_effort: Option<ReasoningEffort>,
    task_source: Option<String>,
    transport_eligibility: TransportEligibility,
    escalation_reason: Option<String>,
    lineage: impl Into<String>,
) -> Result<EconomyDecision> {
    resolve_task_economy_with_additional_busy(
        db,
        task,
        action,
        overrides,
        requested_mode,
        constrained_agent_id,
        task_effort,
        task_source,
        transport_eligibility,
        escalation_reason,
        lineage,
        &HashSet::new(),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the authoritative boundary keeps every policy input explicit"
)]
pub(crate) fn resolve_task_economy_with_additional_busy(
    db: &Database,
    task: &Task,
    action: AgentAction,
    overrides: EconomyOverrides,
    requested_mode: Option<&str>,
    constrained_agent_id: Option<String>,
    task_effort: Option<ReasoningEffort>,
    task_source: Option<String>,
    transport_eligibility: TransportEligibility,
    escalation_reason: Option<String>,
    lineage: impl Into<String>,
    additional_busy: &HashSet<String>,
) -> Result<EconomyDecision> {
    let candidates = db.list_schedulable_agents()?;
    let mut profiles = BTreeMap::new();
    for agent in &candidates {
        if let Some(profile) = db
            .agent_action_profiles(&agent.id)?
            .into_iter()
            .find(|profile| profile.action == action)
        {
            profiles.insert(agent.id.clone(), profile);
        }
    }
    let hints = db
        .get_task_execution_hints(&task.id)?
        .ok_or_else(|| anyhow::anyhow!("task execution hints are missing"))?;
    let execution_class = hints
        .class
        .as_deref()
        .map(ExecutionClass::parse)
        .transpose()
        .map_err(anyhow::Error::msg)?
        .unwrap_or_else(|| crate::execution::class_for_role(&task.role));
    let template = db.execution_template(execution_class)?;
    let costs = db.economy_cost_configuration()?;
    let mut busy_agents = db.list_busy_agents()?.into_iter().collect::<HashSet<_>>();
    busy_agents.extend(additional_busy.iter().cloned());
    // An invocation continuing an existing run already owns this reservation.
    if let Some(agent) = constrained_agent_id.as_ref() {
        busy_agents.remove(agent);
    }
    resolve_economy(EconomyResolverInput {
        action,
        candidates: &candidates,
        task: Some(task),
        required_capabilities: &task.required_capabilities(),
        requested_mode,
        busy_agents: &busy_agents,
        quota_reserve: db.quota_reserve()?,
        overrides,
        constrained_agent_id,
        action_profiles: &profiles,
        execution_class,
        execution_template: &template,
        task_model: hints.model,
        task_effort,
        task_source,
        policy_model: None,
        policy_effort: None,
        policy_source: None,
        cost_configuration: &costs,
        transport_eligibility,
        escalation_reason,
        lineage: lineage.into(),
    })
}

pub fn resolve_action_economy(
    db: &Database,
    action: AgentAction,
    overrides: EconomyOverrides,
    transport_eligibility: TransportEligibility,
) -> Result<EconomyDecision> {
    let candidates = db.list_schedulable_agents()?;
    let mut profiles = BTreeMap::new();
    for agent in &candidates {
        if let Some(profile) = db
            .agent_action_profiles(&agent.id)?
            .into_iter()
            .find(|profile| profile.action == action)
        {
            profiles.insert(agent.id.clone(), profile);
        }
    }
    let class = execution_class_for_action(action);
    let template = db.execution_template(class)?;
    let costs = db.economy_cost_configuration()?;
    let busy_agents = db.list_busy_agents()?.into_iter().collect::<HashSet<_>>();
    resolve_economy(EconomyResolverInput {
        action,
        candidates: &candidates,
        task: None,
        required_capabilities: &[],
        requested_mode: Some(registry::AUTOMATED),
        busy_agents: &busy_agents,
        quota_reserve: db.quota_reserve()?,
        overrides,
        constrained_agent_id: None,
        action_profiles: &profiles,
        execution_class: class,
        execution_template: &template,
        task_model: None,
        task_effort: None,
        task_source: None,
        policy_model: None,
        policy_effort: None,
        policy_source: None,
        cost_configuration: &costs,
        transport_eligibility,
        escalation_reason: None,
        lineage: format!("action:{}", action.as_str()),
    })
}

pub fn resolve_run_invocation_economy(
    db: &Database,
    task: &Task,
    agent_id: &str,
    model: Option<String>,
    effort: Option<ReasoningEffort>,
    purpose: &str,
    transport_eligibility: TransportEligibility,
) -> Result<EconomyResolution> {
    let candidates = db.list_schedulable_agents()?;
    let mut profiles = BTreeMap::new();
    for agent in &candidates {
        if let Some(profile) = db
            .agent_action_profiles(&agent.id)?
            .into_iter()
            .find(|profile| profile.action == AgentAction::Code)
        {
            profiles.insert(agent.id.clone(), profile);
        }
    }
    let hints = db
        .get_task_execution_hints(&task.id)?
        .ok_or_else(|| anyhow::anyhow!("task execution hints are missing"))?;
    let class = hints
        .class
        .as_deref()
        .map(ExecutionClass::parse)
        .transpose()
        .map_err(anyhow::Error::msg)?
        .unwrap_or_else(|| crate::execution::class_for_role(&task.role));
    let template = db.execution_template(class)?;
    let costs = db.economy_cost_configuration()?;
    let mut busy_agents = db.list_busy_agents()?.into_iter().collect::<HashSet<_>>();
    busy_agents.remove(agent_id);
    let decision = resolve_economy(EconomyResolverInput {
        action: AgentAction::Code,
        candidates: &candidates,
        task: Some(task),
        required_capabilities: &task.required_capabilities(),
        requested_mode: Some(registry::AUTOMATED),
        busy_agents: &busy_agents,
        quota_reserve: db.quota_reserve()?,
        overrides: EconomyOverrides::default(),
        constrained_agent_id: Some(agent_id.into()),
        action_profiles: &profiles,
        execution_class: class,
        execution_template: &template,
        task_model: hints.model,
        task_effort: task.reasoning_effort,
        task_source: Some("task_contract".into()),
        policy_model: model,
        policy_effort: effort,
        policy_source: Some(purpose.into()),
        cost_configuration: &costs,
        transport_eligibility,
        escalation_reason: (purpose.contains("repair"))
            .then(|| "bounded evidence-backed repair".into()),
        lineage: format!("{purpose}:task:{}", task.id),
    })?;
    decision.resolution.ok_or_else(|| {
        anyhow::anyhow!(
            "agent '{agent_id}' is not eligible for provider invocation '{purpose}': {}",
            decision.schedule.explanation
        )
    })
}

pub fn validate_override(agent: &AgentDefinition, task: &Task) -> Result<()> {
    validate_override_with_constraints(agent, task, &HashSet::new(), 0)
}

pub fn validate_override_with_constraints(
    agent: &AgentDefinition,
    task: &Task,
    busy_agents: &HashSet<String>,
    quota_reserve: i64,
) -> Result<()> {
    let evaluation = evaluate_candidate_with_busy_and_quota_reserve(
        agent,
        task,
        None,
        busy_agents,
        quota_reserve,
    );
    if let CandidateStatus::Rejected(reason) = evaluation.status {
        bail!(
            "agent '{}' is not eligible: {}",
            agent.id,
            reason.description()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskPriority;

    fn test_task(capabilities: Vec<&str>) -> Task {
        Task {
            id: "T-0001".to_string(),
            title: "Test Task".to_string(),
            objective: "Do testing".to_string(),
            role: "dev".to_string(),
            priority: TaskPriority::Normal,
            status: crate::task::TaskStatus::Ready,
            cancellation_reason: None,
            required_capabilities: capabilities.into_iter().map(String::from).collect(),
            scope_mode: None,
            context_files: Vec::new(),
            expected_changes: Vec::new(),
            reasoning_effort: None,
            effort_reason: None,
            risk_factors: Vec::new(),
        }
    }

    fn test_agent(id: &str, priority: i64, capabilities: Vec<&str>) -> AgentDefinition {
        AgentDefinition {
            id: id.to_string(),
            backend: "codex".to_string(),
            execution_mode: "automated".to_string(),
            display_name: id.to_string(),
            enabled: true,
            priority,
            capabilities: capabilities.into_iter().map(String::from).collect(),
            status: registry::AVAILABLE.to_string(),
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
            actions: vec![crate::registry::AgentAction::Code],
        }
    }

    fn economy(
        task: &Task,
        agents: &[AgentDefinition],
        costs: &EconomyCostConfiguration,
        overrides: EconomyOverrides,
        profiles: &BTreeMap<String, AgentActionProfile>,
        template: &ExecutionTemplate,
        transport: TransportEligibility,
    ) -> EconomyDecision {
        resolve_economy(EconomyResolverInput {
            action: AgentAction::Code,
            candidates: agents,
            task: Some(task),
            required_capabilities: &task.required_capabilities(),
            requested_mode: Some(registry::AUTOMATED),
            busy_agents: &HashSet::new(),
            quota_reserve: 0,
            overrides,
            constrained_agent_id: None,
            action_profiles: profiles,
            execution_class: ExecutionClass::Coder,
            execution_template: template,
            task_model: None,
            task_effort: task.reasoning_effort,
            task_source: None,
            policy_model: None,
            policy_effort: None,
            policy_source: None,
            cost_configuration: costs,
            transport_eligibility: transport,
            escalation_reason: None,
            lineage: "test".into(),
        })
        .unwrap()
    }

    #[test]
    fn cheapest_economy_tier_precedes_agent_priority() {
        let task = test_task(vec!["code"]);
        let mut expensive = test_agent("expensive", 1_000, vec!["code"]);
        expensive.model = Some("large".into());
        let mut cheap = test_agent("cheap", 1, vec!["code"]);
        cheap.model = Some("small".into());
        let costs = EconomyCostConfiguration {
            model_costs: BTreeMap::from([("small".into(), 1.0), ("large".into(), 4.0)]),
            unknown_tier: EconomyTier::Unknown,
        };
        let decision = economy(
            &task,
            &[expensive, cheap],
            &costs,
            EconomyOverrides::default(),
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
        );
        assert_eq!(
            decision.schedule.selected_agent_id.as_deref(),
            Some("cheap")
        );
        assert_eq!(
            decision.schedule.selection_reason,
            SelectionReason::CheapestEconomyTier
        );
        assert_eq!(
            decision.resolution.unwrap().record.tier,
            EconomyTier::Default
        );
    }

    #[test]
    fn same_economy_tier_preserves_scheduler_ordering() {
        let task = test_task(vec!["code"]);
        let mut high = test_agent("high", 200, vec!["code"]);
        high.model = Some("small-a".into());
        let mut low = test_agent("low", 100, vec!["code"]);
        low.model = Some("small-b".into());
        let costs = EconomyCostConfiguration {
            model_costs: BTreeMap::from([("small-a".into(), 1.0), ("small-b".into(), 0.5)]),
            unknown_tier: EconomyTier::Unknown,
        };
        let decision = economy(
            &task,
            &[low, high],
            &costs,
            EconomyOverrides::default(),
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
        );
        assert_eq!(decision.schedule.selected_agent_id.as_deref(), Some("high"));
        assert_eq!(
            decision.schedule.selection_reason,
            SelectionReason::HighestPriority
        );
    }

    #[test]
    fn task_risk_never_promotes_reasoning_effort_or_economy_tier() {
        let mut task = test_task(vec!["code"]);
        task.reasoning_effort = Some(ReasoningEffort::Low);
        task.risk_factors = crate::protocol::TaskRiskFactor::ALL.to_vec();
        let mut agent = test_agent("cheap", 1, vec!["code"]);
        agent.model = Some("small".into());
        let costs = EconomyCostConfiguration {
            model_costs: BTreeMap::from([("small".into(), 1.0)]),
            unknown_tier: EconomyTier::Unknown,
        };
        let resolution = economy(
            &task,
            &[agent],
            &costs,
            EconomyOverrides::default(),
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
        )
        .resolution
        .unwrap();
        assert_eq!(
            resolution.execution.reasoning_effort,
            Some(ReasoningEffort::Low)
        );
        assert_eq!(resolution.record.tier, EconomyTier::Default);
        assert_eq!(resolution.record.escalation_reason, None);
    }

    #[test]
    fn operator_override_preserves_provenance_without_bypassing_eligibility() {
        let task = test_task(vec!["code"]);
        let mut disabled = test_agent("disabled", 100, vec!["code"]);
        disabled.enabled = false;
        let decision = economy(
            &task,
            &[disabled],
            &EconomyCostConfiguration::default(),
            EconomyOverrides {
                agent_id: Some("disabled".into()),
                model: Some("operator-model".into()),
                effort: Some(ReasoningEffort::High),
            },
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
        );
        assert!(decision.resolution.is_none());
        assert!(matches!(
            decision.schedule.candidates[0].status,
            CandidateStatus::Rejected(RejectionReason::Disabled)
        ));

        let eligible = test_agent("eligible", 100, vec!["code"]);
        let decision = economy(
            &task,
            &[eligible],
            &EconomyCostConfiguration::default(),
            EconomyOverrides {
                agent_id: Some("eligible".into()),
                model: Some("operator-model".into()),
                effort: Some(ReasoningEffort::High),
            },
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
        );
        let record = decision.resolution.unwrap().record;
        assert_eq!(record.source, "operator_override");
        assert_eq!(record.selected_model.as_deref(), Some("operator-model"));
        assert_eq!(record.effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn action_profile_precedes_execution_template_inside_resolver() {
        let task = test_task(vec!["code"]);
        let agent = test_agent("agent", 100, vec!["code"]);
        let profiles = BTreeMap::from([(
            "agent".into(),
            AgentActionProfile {
                action: AgentAction::Code,
                model: Some("profile-model".into()),
                reasoning_effort: None,
            },
        )]);
        let template = ExecutionTemplate {
            model: Some("template-model".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
        };
        let resolution = economy(
            &task,
            &[agent],
            &EconomyCostConfiguration::default(),
            EconomyOverrides::default(),
            &profiles,
            &template,
            TransportEligibility::Strict,
        )
        .resolution
        .unwrap();
        assert_eq!(resolution.execution.model.as_deref(), Some("profile-model"));
        assert_eq!(
            resolution.execution.reasoning_effort,
            Some(ReasoningEffort::Medium)
        );
        assert_eq!(resolution.record.source, "action_profile");
    }

    #[test]
    fn injected_transport_ignores_only_unsupported_backend() {
        let task = test_task(vec!["code"]);
        let mut fake = test_agent("fake", 100, vec!["code"]);
        fake.backend = "fake".into();
        let strict = economy(
            &task,
            &[fake.clone()],
            &EconomyCostConfiguration::default(),
            EconomyOverrides::default(),
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
        );
        assert!(strict.resolution.is_none());
        let injected = economy(
            &task,
            &[fake.clone()],
            &EconomyCostConfiguration::default(),
            EconomyOverrides::default(),
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::IgnoreUnsupportedBackend,
        );
        assert!(injected.resolution.is_some());

        fake.quota_remaining_percent = Some(0);
        let exhausted = economy(
            &task,
            &[fake],
            &EconomyCostConfiguration::default(),
            EconomyOverrides::default(),
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::IgnoreUnsupportedBackend,
        );
        assert!(exhausted.resolution.is_none());
        assert!(matches!(
            exhausted.schedule.candidates[0].status,
            CandidateStatus::Rejected(RejectionReason::QuotaExhausted)
        ));
    }

    #[test]
    fn test_highest_priority_wins() {
        let task = test_task(vec!["code", "terminal"]);
        let a1 = test_agent("agent-low", 50, vec!["code", "terminal"]);
        let a2 = test_agent("agent-high", 100, vec!["code", "terminal"]);
        let decision = schedule(&task, &[a1, a2], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("agent-high"));
        assert_eq!(decision.selection_reason, SelectionReason::HighestPriority);
    }

    #[test]
    fn substantially_healthier_capacity_beats_modest_priority() {
        let task = test_task(vec!["code"]);
        let mut priority = test_agent("priority", 105, vec!["code"]);
        priority.quota_remaining_percent = Some(20);
        let mut healthy = test_agent("healthy", 100, vec!["code"]);
        healthy.quota_remaining_percent = Some(80);
        let decision = schedule(&task, &[priority, healthy], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("healthy"));
        assert_eq!(
            decision.selection_reason,
            SelectionReason::HealthierCapacity
        );
    }

    #[test]
    fn nearby_quota_values_have_no_bucket_cliff() {
        let task = test_task(vec!["code"]);
        let mut nine = test_agent("nine", 100, vec!["code"]);
        nine.quota_remaining_percent = Some(9);
        let mut ten = test_agent("ten", 100, vec!["code"]);
        ten.quota_remaining_percent = Some(10);
        let nine_eval = evaluate_candidate(&nine, &task, None);
        let ten_eval = evaluate_candidate(&ten, &task, None);
        assert_eq!(ten_eval.capacity_score, Some(10));
        assert_eq!(nine_eval.capacity_score, Some(9));
        assert!(ranking_score(&ten_eval) > ranking_score(&nine_eval));
    }

    #[test]
    fn similar_capacity_prefers_higher_priority() {
        let task = test_task(vec!["code"]);
        let mut high = test_agent("high", 105, vec!["code"]);
        high.quota_remaining_percent = Some(52);
        let mut low = test_agent("low", 100, vec!["code"]);
        low.quota_remaining_percent = Some(54);
        let decision = schedule(&task, &[low, high], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("high"));
        assert_eq!(decision.selection_reason, SelectionReason::HighestPriority);
    }

    #[test]
    fn reset_horizon_influences_capacity() {
        let task = test_task(vec!["code"]);
        let mut soon = test_agent("soon", 100, vec!["code"]);
        soon.quota_remaining_percent = Some(50);
        soon.quota_reset_at = Some("2026-08-22T00:00:00Z".to_string());
        let mut far = test_agent("far", 100, vec!["code"]);
        far.quota_remaining_percent = Some(50);
        far.quota_reset_at = Some("2999-01-01T00:00:00Z".to_string());
        let soon_eval = evaluate_candidate(&soon, &task, None);
        let far_eval = evaluate_candidate(&far, &task, None);
        assert!(soon_eval.capacity_score > far_eval.capacity_score);
        let decision = schedule(&task, &[far, soon], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("soon"));
    }

    #[test]
    fn reset_horizon_changes_ranking_score() {
        let task = test_task(vec!["code"]);
        let mut soon = test_agent("soon", 100, vec!["code"]);
        soon.quota_remaining_percent = Some(50);
        soon.quota_reset_at = Some("2026-08-22T00:00:00Z".to_string());
        let mut far = soon.clone();
        far.id = "far".to_string();
        far.quota_reset_at = Some("2999-01-01T00:00:00Z".to_string());
        let now = parse_rfc3339_epoch("2026-08-21T00:00:00Z").unwrap();
        let soon_score = capacity_score_at(&soon, now);
        let far_score = capacity_score_at(&far, now);
        let soon_eval = evaluate_candidate(&soon, &task, None);
        let far_eval = evaluate_candidate(&far, &task, None);
        assert!(soon_score > far_score);
        assert!(ranking_score(&soon_eval) > ranking_score(&far_eval));
    }

    #[test]
    fn past_reset_timestamp_has_no_near_reset_bonus() {
        let mut agent = test_agent("past", 100, vec!["code"]);
        agent.quota_remaining_percent = Some(50);
        agent.quota_reset_at = Some("2026-08-20T00:00:00Z".to_string());
        let now = parse_rfc3339_epoch("2026-08-21T00:00:00Z").unwrap();
        assert_eq!(capacity_score_at(&agent, now), Some(50));
    }

    #[test]
    fn test_lexicographic_tie_break() {
        let task = test_task(vec!["code", "terminal"]);
        let a1 = test_agent("codex-b", 100, vec!["code", "terminal"]);
        let a2 = test_agent("codex-a", 100, vec!["code", "terminal"]);
        let decision = schedule(&task, &[a1, a2], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("codex-a"));
        assert_eq!(
            decision.selection_reason,
            SelectionReason::LexicographicTieBreak
        );
    }

    #[test]
    fn test_disabled_rejected() {
        let task = test_task(vec!["code", "terminal"]);
        let mut a = test_agent("agent-1", 100, vec!["code", "terminal"]);
        a.enabled = false;
        let decision = schedule(&task, &[a], None).unwrap();
        assert_eq!(decision.selected_agent_id, None);
        assert_eq!(
            decision.candidates[0].status,
            CandidateStatus::Rejected(RejectionReason::Disabled)
        );
    }

    #[test]
    fn test_unavailable_rejected() {
        let task = test_task(vec!["code", "terminal"]);
        let mut a = test_agent("agent-1", 100, vec!["code", "terminal"]);
        a.status = registry::UNAVAILABLE.to_string();
        a.unavailable_reason = Some("maintenance".to_string());
        let decision = schedule(&task, &[a], None).unwrap();
        assert_eq!(decision.selected_agent_id, None);
        assert_eq!(
            decision.candidates[0].status,
            CandidateStatus::Rejected(RejectionReason::Unavailable {
                reason: Some("maintenance".to_string())
            })
        );
    }

    #[test]
    fn test_known_zero_quota_rejected() {
        let task = test_task(vec!["code", "terminal"]);
        let mut a = test_agent("agent-1", 100, vec!["code", "terminal"]);
        a.quota_remaining_percent = Some(0);
        let decision = schedule(&task, &[a], None).unwrap();
        assert_eq!(decision.selected_agent_id, None);
        assert_eq!(
            decision.candidates[0].status,
            CandidateStatus::Rejected(RejectionReason::QuotaExhausted)
        );
    }

    #[test]
    fn test_unknown_quota_eligible() {
        let task = test_task(vec!["code", "terminal"]);
        let mut a = test_agent("agent-1", 100, vec!["code", "terminal"]);
        a.quota_remaining_percent = None;
        let decision = schedule(&task, &[a], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("agent-1"));
        assert_eq!(decision.candidates[0].status, CandidateStatus::Eligible);
    }

    #[test]
    fn unknown_quota_ordering_is_deterministic() {
        let task = test_task(vec!["code"]);
        let first = test_agent("agent-b", 100, vec!["code"]);
        let second = test_agent("agent-a", 100, vec!["code"]);
        let decision = schedule(&task, &[first, second], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("agent-a"));
    }

    #[test]
    fn test_quota_does_not_override_priority() {
        let task = test_task(vec!["code", "terminal"]);
        let mut a1 = test_agent("agent-high", 100, vec!["code", "terminal"]);
        a1.quota_remaining_percent = Some(2);
        let mut a2 = test_agent("agent-low", 90, vec!["code", "terminal"]);
        a2.quota_remaining_percent = Some(4);
        let decision = schedule(&task, &[a1, a2], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("agent-high"));
    }

    #[test]
    fn test_quota_reserve_rejects_known_low_quota() {
        let task = test_task(vec!["code", "terminal"]);
        let mut agent = test_agent("agent-1", 100, vec!["code", "terminal"]);
        agent.quota_remaining_percent = Some(10);
        let decision = schedule_with_quota_reserve(&task, &[agent], None, 20).unwrap();
        assert_eq!(decision.selected_agent_id, None);
        assert_eq!(
            decision.candidates[0].status,
            CandidateStatus::Rejected(RejectionReason::QuotaReserve {
                remaining: 10,
                reserve: 20,
            })
        );
    }

    #[test]
    fn test_quota_reserve_keeps_unknown_quota_eligible() {
        let task = test_task(vec!["code", "terminal"]);
        let agent = test_agent("agent-1", 100, vec!["code", "terminal"]);
        let decision = schedule_with_quota_reserve(&task, &[agent], None, 20).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("agent-1"));
    }

    #[test]
    fn test_missing_capability_rejected() {
        let task = test_task(vec!["code", "terminal"]);
        let a = test_agent("agent-1", 100, vec!["code"]);
        let decision = schedule(&task, &[a], None).unwrap();
        assert_eq!(decision.selected_agent_id, None);
        assert_eq!(
            decision.candidates[0].status,
            CandidateStatus::Rejected(RejectionReason::MissingCapability {
                capability: "command_execution".to_string()
            })
        );
    }

    #[test]
    fn test_mode_filter() {
        let task = test_task(vec!["code"]);
        let mut a_auto = test_agent("agent-auto", 100, vec!["code", "terminal"]);
        a_auto.execution_mode = "automated".to_string();
        let mut a_man = test_agent("agent-man", 90, vec!["code"]);
        a_man.backend = "chatgpt".to_string();
        a_man.execution_mode = "manual".to_string();

        let dec_manual = schedule(&task, &[a_auto.clone(), a_man.clone()], Some("manual")).unwrap();
        assert_eq!(dec_manual.selected_agent_id.as_deref(), Some("agent-man"));

        let dec_auto = schedule(&task, &[a_auto, a_man], Some("automated")).unwrap();
        assert_eq!(dec_auto.selected_agent_id.as_deref(), Some("agent-auto"));
    }
}
