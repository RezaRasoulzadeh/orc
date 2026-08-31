use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::execution::{ExecutionClass, ExecutionResolution, ExecutionTemplate};
use crate::registry::{
    self, AgentAction, AgentActionProfile, AgentDefinition, EconomyCostConfiguration, EconomyTier,
    EscalationPolicyConfiguration, EscalationRequest, EscalationTrigger, ReasoningEffort,
    ResolutionRecord,
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
    QuotaRefreshFailed { error: String },
    Busy,
    ModeMismatch { requested: String, actual: String },
    UnsupportedAction { action: String },
    AgentConstraint { selected: String },
    BelowEscalationTier { required: EconomyTier },
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
            Self::QuotaRefreshFailed { error } => {
                format!("quota refresh failed: {error}")
            }
            Self::Busy => "busy".to_string(),
            Self::ModeMismatch { requested, actual } => {
                format!("mode mismatch (requested: {requested}, actual: {actual})")
            }
            Self::UnsupportedAction { action } => format!("unsupported action: {action}"),
            Self::AgentConstraint { selected } => {
                format!("agent selection constrained to: {selected}")
            }
            Self::BelowEscalationTier { required } => {
                format!(
                    "economy tier is below required escalation tier: {}",
                    required.as_str()
                )
            }
        }
    }
}

/// One conservative, provider-independent rule for deciding whether persisted
/// quota is current enough to participate in eligibility and capacity ranking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaFreshnessPolicy {
    pub max_age: Duration,
}

impl Default for QuotaFreshnessPolicy {
    fn default() -> Self {
        Self {
            max_age: Duration::from_secs(15 * 60),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaFreshness {
    Fresh,
    Stale,
    #[default]
    NeverChecked,
    RefreshFailed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaObservation {
    pub remaining_percent: Option<i64>,
    pub reset_at: Option<String>,
    pub checked_at: Option<String>,
    pub source: Option<String>,
    pub freshness: QuotaFreshness,
    pub reserve_percent: i64,
    pub refresh_supported: bool,
    pub refresh_error: Option<String>,
}

impl QuotaObservation {
    pub fn description(&self) -> String {
        let value = self
            .remaining_percent
            .map(|remaining| format!("{remaining}% remaining"))
            .unwrap_or_else(|| "remaining unknown".into());
        match self.freshness {
            QuotaFreshness::Fresh if self.remaining_percent == Some(0) => {
                format!("fresh; {value}; exhausted")
            }
            QuotaFreshness::Fresh
                if self.reserve_percent > 0
                    && self
                        .remaining_percent
                        .is_some_and(|remaining| remaining < self.reserve_percent) =>
            {
                format!("fresh; {value}; below {}% reserve", self.reserve_percent)
            }
            QuotaFreshness::Fresh => format!("fresh; {value}; sufficient"),
            QuotaFreshness::Stale if self.refresh_supported => {
                format!("stale; {value}; refresh required before execution")
            }
            QuotaFreshness::Stale => {
                format!("stale; {value}; refresh unsupported, conservative fallback")
            }
            QuotaFreshness::NeverChecked if self.refresh_supported => {
                "unknown / never checked; refresh required before execution".into()
            }
            QuotaFreshness::NeverChecked => {
                "unknown / never checked; refresh unsupported, conservative fallback".into()
            }
            QuotaFreshness::RefreshFailed => format!(
                "refresh failed: {}",
                self.refresh_error.as_deref().unwrap_or("unknown error")
            ),
        }
    }
}

impl QuotaFreshnessPolicy {
    pub fn classify(self, agent: &AgentDefinition, now_epoch: i64) -> QuotaFreshness {
        let Some(checked_at) = agent.quota_checked_at.as_deref() else {
            return QuotaFreshness::NeverChecked;
        };
        let Some(checked_epoch) = parse_timestamp_epoch(checked_at) else {
            return QuotaFreshness::Stale;
        };
        let age = now_epoch.saturating_sub(checked_epoch);
        if age >= 0 && age <= self.max_age.as_secs() as i64 {
            QuotaFreshness::Fresh
        } else {
            QuotaFreshness::Stale
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
    pub quota_observation: QuotaObservation,
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
    pub quota_refresh_failures: &'a BTreeMap<String, String>,
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
    pub escalation_request: Option<EscalationRequest>,
    pub lineage: String,
}

/// Injectable provider-independent seam used only before an execution-side
/// scheduling decision. Implementations update Orc's persisted normalized
/// observation; the economy resolver itself remains pure.
pub trait QuotaRefresher {
    fn supports(&self, agent: &AgentDefinition) -> bool;
    fn refresh(&self, db: &Database, agent: &AgentDefinition) -> std::result::Result<(), String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProviderQuotaRefresher;

impl QuotaRefresher for ProviderQuotaRefresher {
    fn supports(&self, agent: &AgentDefinition) -> bool {
        crate::backend::provider_supports_quota(&agent.backend)
    }

    fn refresh(&self, db: &Database, agent: &AgentDefinition) -> std::result::Result<(), String> {
        crate::backend::sync_agent_quota(db, agent).map(|_| ())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedQuotaRefresher;

impl QuotaRefresher for UnsupportedQuotaRefresher {
    fn supports(&self, _agent: &AgentDefinition) -> bool {
        false
    }

    fn refresh(&self, _db: &Database, _agent: &AgentDefinition) -> std::result::Result<(), String> {
        Err("quota reconciliation is unsupported by this transport".into())
    }
}

static QUOTA_REFRESH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscalationObservation {
    Retry,
    SingleSemanticRevision,
    SingleValidationFailure,
    InfrastructureValidationFailure,
    TransientProviderFailure,
    RiskMetadataOnly,
    ValidationRepairNonConvergence,
    SemanticRevisionNonConvergence,
    ExplicitPolicyRequest,
}

#[derive(Clone, Debug)]
pub struct EscalationPolicyInput<'a> {
    pub observation: EscalationObservation,
    pub previous_provider_invocation_id: Option<i64>,
    pub previous_resolution: Option<&'a ResolutionRecord>,
    pub previous_attempt: usize,
    pub policy_attempt: usize,
    pub configuration: &'a EscalationPolicyConfiguration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscalationDecision {
    NoEscalation { reason: String },
    Escalate(EscalationRequest),
    Exhausted { reason: String },
}

pub fn evaluate_escalation_policy(input: EscalationPolicyInput<'_>) -> EscalationDecision {
    let trigger = match input.observation {
        EscalationObservation::ValidationRepairNonConvergence => {
            if !input.configuration.validation_repair_non_convergence {
                return EscalationDecision::NoEscalation {
                    reason: "validation-repair escalation policy is disabled".into(),
                };
            }
            EscalationTrigger::ValidationRepairNonConvergence
        }
        EscalationObservation::SemanticRevisionNonConvergence => {
            if !input.configuration.semantic_revision_non_convergence {
                return EscalationDecision::NoEscalation {
                    reason: "semantic-revision escalation policy is disabled".into(),
                };
            }
            EscalationTrigger::SemanticRevisionNonConvergence
        }
        EscalationObservation::ExplicitPolicyRequest => EscalationTrigger::ExplicitPolicyRequest,
        EscalationObservation::Retry => {
            return EscalationDecision::NoEscalation {
                reason: "ordinary retry remains on the current economy tier".into(),
            };
        }
        EscalationObservation::SingleSemanticRevision => {
            return EscalationDecision::NoEscalation {
                reason: "one semantic revision is part of the normal lifecycle".into(),
            };
        }
        EscalationObservation::SingleValidationFailure => {
            return EscalationDecision::NoEscalation {
                reason: "one deterministic validation failure receives same-tier repair".into(),
            };
        }
        EscalationObservation::InfrastructureValidationFailure => {
            return EscalationDecision::NoEscalation {
                reason: "validation infrastructure failure is not model insufficiency".into(),
            };
        }
        EscalationObservation::TransientProviderFailure => {
            return EscalationDecision::NoEscalation {
                reason: "transient provider failure does not imply model insufficiency".into(),
            };
        }
        EscalationObservation::RiskMetadataOnly => {
            return EscalationDecision::NoEscalation {
                reason: "task risk metadata requires guards but never triggers escalation".into(),
            };
        }
    };
    let (Some(previous_invocation), Some(previous)) = (
        input.previous_provider_invocation_id,
        input.previous_resolution,
    ) else {
        return EscalationDecision::Exhausted {
            reason: "escalation requires a persisted previous provider resolution".into(),
        };
    };
    let Some(next_tier) = previous.tier.next() else {
        return EscalationDecision::Exhausted {
            reason: format!(
                "economy escalation exhausted at tier '{}'",
                previous.tier.as_str()
            ),
        };
    };
    if next_tier.rank() > input.configuration.maximum_tier.rank() {
        return EscalationDecision::Exhausted {
            reason: format!(
                "economy escalation is bounded at tier '{}'",
                input.configuration.maximum_tier.as_str()
            ),
        };
    }
    let reason = match trigger {
        EscalationTrigger::ValidationRepairNonConvergence => format!(
            "deterministic validation did not converge after {} same-tier repair attempts",
            input.previous_attempt
        ),
        EscalationTrigger::SemanticRevisionNonConvergence => {
            "the same semantic blocker survived a completed revision".into()
        }
        EscalationTrigger::ExplicitPolicyRequest => {
            "higher-level orchestration explicitly requested bounded escalation".into()
        }
    };
    EscalationDecision::Escalate(EscalationRequest {
        reason,
        lineage: crate::registry::EscalationLineage {
            request_id: None,
            trigger,
            previous_provider_invocation_id: previous_invocation,
            previous_tier: previous.tier,
            previous_model: previous.selected_model.clone(),
            previous_effort: previous.effort,
            previous_attempt: input.previous_attempt,
            requested_minimum_tier: next_tier,
            policy_attempt: input.policy_attempt,
        },
    })
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
                    out.push_str(&format!(
                        "  quota: {}\n",
                        cand.quota_observation.description()
                    ));
                }
                CandidateStatus::Rejected(reason) => {
                    out.push_str("  REJECTED\n");
                    out.push_str(&format!("  {}\n", reason.description()));
                    out.push_str(&format!(
                        "  quota: {}\n",
                        cand.quota_observation.description()
                    ));
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
    let failures = BTreeMap::new();
    evaluate_candidate_for_requirements(
        agent,
        &task.required_capabilities(),
        requested_mode,
        quota_reserve,
        TransportEligibility::Strict,
        current_epoch(),
        &failures,
    )
}

fn evaluate_candidate_for_requirements(
    agent: &AgentDefinition,
    required: &[String],
    requested_mode: Option<&str>,
    quota_reserve: i64,
    transport_eligibility: TransportEligibility,
    now_epoch: i64,
    quota_refresh_failures: &BTreeMap<String, String>,
) -> CandidateEvaluation {
    let quota_observation = quota_observation_for(
        agent,
        quota_reserve,
        now_epoch,
        quota_refresh_failures.get(&agent.id).map(String::as_str),
    );
    let make_eval = |status: CandidateStatus| CandidateEvaluation {
        agent_id: agent.id.clone(),
        backend: agent.backend.clone(),
        execution_mode: agent.execution_mode.clone(),
        priority: agent.priority,
        quota_remaining_percent: agent.quota_remaining_percent,
        quota_reset_at: agent.quota_reset_at.clone(),
        capacity_score: (quota_observation.freshness == QuotaFreshness::Fresh)
            .then(|| capacity_score_at(agent, now_epoch))
            .flatten(),
        quota_observation: quota_observation.clone(),
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

    // 5. Only a fresh observation can prove quota insufficiency. Stale and
    // never-checked values remain provisionally eligible for read-only
    // explanation; execution paths reconcile them before final resolution.
    if let Some(error) = quota_refresh_failures.get(&agent.id) {
        return make_eval(CandidateStatus::Rejected(
            RejectionReason::QuotaRefreshFailed {
                error: error.clone(),
            },
        ));
    }
    if quota_observation.freshness == QuotaFreshness::Fresh
        && agent.quota_remaining_percent == Some(0)
    {
        return make_eval(CandidateStatus::Rejected(RejectionReason::QuotaExhausted));
    }
    if quota_observation.freshness == QuotaFreshness::Fresh
        && quota_reserve > 0
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

fn quota_observation_for(
    agent: &AgentDefinition,
    quota_reserve: i64,
    now_epoch: i64,
    refresh_error: Option<&str>,
) -> QuotaObservation {
    let mut observation = QuotaObservation {
        remaining_percent: agent.quota_remaining_percent,
        reset_at: agent.quota_reset_at.clone(),
        checked_at: agent.quota_checked_at.clone(),
        source: agent.quota_source.clone(),
        freshness: QuotaFreshnessPolicy::default().classify(agent, now_epoch),
        reserve_percent: quota_reserve,
        refresh_supported: crate::backend::provider_supports_quota(&agent.backend),
        refresh_error: None,
    };
    if let Some(error) = refresh_error {
        observation.freshness = QuotaFreshness::RefreshFailed;
        observation.refresh_error = Some(error.to_owned());
    }
    observation
}

fn capacity_score_at(agent: &AgentDefinition, now_epoch: i64) -> Option<i64> {
    let remaining = agent.quota_remaining_percent?;
    let horizon_bonus = agent
        .quota_reset_at
        .as_deref()
        .and_then(parse_timestamp_epoch)
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
    if automatic_escalation(input).is_some() {
        resolution.source = "policy_escalation".into();
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

fn automatic_escalation<'a>(input: &'a EconomyResolverInput<'a>) -> Option<&'a EscalationRequest> {
    (input.overrides.agent_id.is_none()
        && input.overrides.model.is_none()
        && input.overrides.effort.is_none())
    .then_some(input.escalation_request.as_ref())
    .flatten()
}

/// Resolve eligibility, execution identity, and economy ordering in one place.
/// Callers may supply constraints and policy inputs, but this function alone
/// creates the final provider-independent [`ResolutionRecord`].
pub fn resolve_economy(input: EconomyResolverInput<'_>) -> Result<EconomyDecision> {
    let now_epoch = current_epoch();
    let escalation = automatic_escalation(&input).cloned();
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
                now_epoch,
                input.quota_refresh_failures,
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
            now_epoch,
            input.quota_refresh_failures,
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
            if escalation.as_ref().is_some_and(|request| {
                evaluation.economy_tier == EconomyTier::Unknown
                    || evaluation.economy_tier.rank()
                        < request.lineage.requested_minimum_tier.rank()
            }) {
                evaluation.status =
                    CandidateStatus::Rejected(RejectionReason::BelowEscalationTier {
                        required: escalation.as_ref().unwrap().lineage.requested_minimum_tier,
                    });
                rejected.push(evaluation);
            } else {
                eligible.push((evaluation, agent.clone(), execution));
            }
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
            "escalation": escalation,
            "quota": evaluation.quota_observation,
            "selection_reason": selection_reason,
            "selection_explanation": explanation,
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
                escalation_reason: escalation.as_ref().map(|request| request.reason.clone()),
                input_lineage,
                escalation: escalation.clone().map(|request| request.lineage),
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

fn current_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| duration.as_secs() as i64)
}

fn parse_timestamp_epoch(value: &str) -> Option<i64> {
    if let Ok(epoch) = value.parse::<i64>() {
        return Some(epoch);
    }
    if let Ok(timestamp) = OffsetDateTime::parse(value, &Rfc3339) {
        return Some(timestamp.unix_timestamp());
    }
    const SQLITE_TIMESTAMP: &[time::format_description::FormatItem<'_>] =
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    PrimitiveDateTime::parse(value, SQLITE_TIMESTAMP)
        .ok()
        .map(|timestamp| timestamp.assume_utc().unix_timestamp())
}

#[cfg(test)]
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
    let quota_refresh_failures = BTreeMap::new();
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
        quota_refresh_failures: &quota_refresh_failures,
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
        escalation_request: None,
        lineage: "scheduler".into(),
    })?
    .schedule)
}

struct QuotaReconciliation {
    failures: BTreeMap<String, String>,
    attempted: bool,
}

fn reconcile_quota_candidates(
    db: &Database,
    preliminary: &EconomyDecision,
    refresher: &dyn QuotaRefresher,
) -> Result<QuotaReconciliation> {
    let mut failures = BTreeMap::new();
    let mut attempted = false;
    let target_tier = preliminary
        .schedule
        .candidates
        .iter()
        .find(|candidate| matches!(candidate.status, CandidateStatus::Eligible))
        .map(|candidate| candidate.economy_tier);
    for candidate in &preliminary.schedule.candidates {
        if !matches!(candidate.status, CandidateStatus::Eligible)
            || Some(candidate.economy_tier) != target_tier
            || candidate.quota_observation.freshness == QuotaFreshness::Fresh
        {
            continue;
        }
        let Some(agent) = db.get_agent(&candidate.agent_id)? else {
            failures.insert(
                candidate.agent_id.clone(),
                "agent disappeared during quota refresh".into(),
            );
            continue;
        };
        if !refresher.supports(&agent) {
            continue;
        }
        attempted = true;

        // Serialize the cheap check-and-refresh section and re-read after
        // acquiring it. Concurrent selectors then reuse the first fresh
        // observation instead of starting duplicate provider calls.
        let lock = QUOTA_REFRESH_LOCK.get_or_init(|| Mutex::new(()));
        let _guard = lock
            .lock()
            .map_err(|_| anyhow::anyhow!("quota refresh coordination lock is poisoned"))?;
        let current = db.get_agent(&agent.id)?.ok_or_else(|| {
            anyhow::anyhow!("agent '{}' disappeared during quota refresh", agent.id)
        })?;
        if QuotaFreshnessPolicy::default().classify(&current, current_epoch())
            == QuotaFreshness::Fresh
        {
            continue;
        }
        if let Err(error) = refresher.refresh(db, &current) {
            failures.insert(agent.id.clone(), error);
        } else {
            let updated = db.get_agent(&agent.id)?.ok_or_else(|| {
                anyhow::anyhow!("agent '{}' disappeared after quota refresh", agent.id)
            })?;
            if QuotaFreshnessPolicy::default().classify(&updated, current_epoch())
                != QuotaFreshness::Fresh
            {
                failures.insert(
                    agent.id.clone(),
                    "provider refresh did not persist a fresh quota observation".into(),
                );
            }
        }
    }
    Ok(QuotaReconciliation {
        failures,
        attempted,
    })
}

fn ensure_refresh_failures_do_not_promote_tier(
    preliminary: &EconomyDecision,
    final_decision: &EconomyDecision,
    failures: &BTreeMap<String, String>,
) -> Result<()> {
    if failures.is_empty() {
        return Ok(());
    }
    let tiers = preliminary
        .schedule
        .candidates
        .iter()
        .map(|candidate| (candidate.agent_id.as_str(), candidate.economy_tier))
        .collect::<BTreeMap<_, _>>();
    let selected_tier = final_decision
        .resolution
        .as_ref()
        .map(|resolution| resolution.record.tier);
    let blocks_selection = failures.keys().any(|agent_id| {
        let failed_tier = tiers.get(agent_id.as_str()).copied().unwrap_or_default();
        selected_tier.is_none_or(|selected| failed_tier.rank() < selected.rank())
    });
    if blocks_selection {
        let details = failures
            .iter()
            .map(|(agent, error)| format!("{agent}: {error}"))
            .collect::<Vec<_>>()
            .join("; ");
        bail!(
            "quota observation failed for a candidate needed to establish the cheapest eligible economy tier: {details}"
        );
    }
    Ok(())
}

fn apply_refresher_capability_metadata(
    db: &Database,
    decision: &mut EconomyDecision,
    refresher: &dyn QuotaRefresher,
) -> Result<()> {
    for candidate in &mut decision.schedule.candidates {
        if let Some(agent) = db.get_agent(&candidate.agent_id)? {
            candidate.quota_observation.refresh_supported = refresher.supports(&agent);
        }
    }
    if let Some(resolution) = decision.resolution.as_mut()
        && let Some(observation) = decision
            .schedule
            .candidates
            .iter()
            .find(|candidate| candidate.agent_id == resolution.agent.id)
            .map(|candidate| &candidate.quota_observation)
    {
        let mut lineage: serde_json::Value =
            serde_json::from_str(&resolution.record.input_lineage)?;
        lineage["quota"] = serde_json::to_value(observation)?;
        resolution.record.input_lineage = lineage.to_string();
    }
    Ok(())
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
    escalation_request: Option<EscalationRequest>,
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
        escalation_request,
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
    escalation_request: Option<EscalationRequest>,
    lineage: impl Into<String>,
    additional_busy: &HashSet<String>,
) -> Result<EconomyDecision> {
    resolve_task_economy_with_additional_busy_and_failures(
        db,
        task,
        action,
        overrides,
        requested_mode,
        constrained_agent_id,
        task_effort,
        task_source,
        transport_eligibility,
        escalation_request,
        lineage.into(),
        additional_busy,
        &BTreeMap::new(),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the authoritative boundary keeps every policy input explicit"
)]
fn resolve_task_economy_with_additional_busy_and_failures(
    db: &Database,
    task: &Task,
    action: AgentAction,
    overrides: EconomyOverrides,
    requested_mode: Option<&str>,
    constrained_agent_id: Option<String>,
    task_effort: Option<ReasoningEffort>,
    task_source: Option<String>,
    transport_eligibility: TransportEligibility,
    escalation_request: Option<EscalationRequest>,
    lineage: String,
    additional_busy: &HashSet<String>,
    quota_refresh_failures: &BTreeMap<String, String>,
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
        quota_refresh_failures,
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
        escalation_request,
        lineage,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "execution reconciliation keeps every policy input explicit"
)]
pub fn resolve_task_economy_for_execution_with_refresher(
    db: &Database,
    task: &Task,
    action: AgentAction,
    overrides: EconomyOverrides,
    requested_mode: Option<&str>,
    constrained_agent_id: Option<String>,
    task_effort: Option<ReasoningEffort>,
    task_source: Option<String>,
    transport_eligibility: TransportEligibility,
    escalation_request: Option<EscalationRequest>,
    lineage: impl Into<String>,
    additional_busy: &HashSet<String>,
    refresher: &dyn QuotaRefresher,
) -> Result<EconomyDecision> {
    let lineage = lineage.into();
    let mut decision = resolve_task_economy_with_additional_busy_and_failures(
        db,
        task,
        action,
        overrides.clone(),
        requested_mode,
        constrained_agent_id.clone(),
        task_effort,
        task_source.clone(),
        transport_eligibility,
        escalation_request.clone(),
        lineage.clone(),
        additional_busy,
        &BTreeMap::new(),
    )?;
    let mut failures = BTreeMap::new();
    let maximum_passes = decision.schedule.candidates.len().saturating_add(1);
    for _ in 0..maximum_passes {
        let reconciliation = reconcile_quota_candidates(db, &decision, refresher)?;
        if !reconciliation.attempted {
            apply_refresher_capability_metadata(db, &mut decision, refresher)?;
            return Ok(decision);
        }
        failures.extend(reconciliation.failures.clone());
        let next = resolve_task_economy_with_additional_busy_and_failures(
            db,
            task,
            action,
            overrides.clone(),
            requested_mode,
            constrained_agent_id.clone(),
            task_effort,
            task_source.clone(),
            transport_eligibility,
            escalation_request.clone(),
            lineage.clone(),
            additional_busy,
            &failures,
        )?;
        ensure_refresh_failures_do_not_promote_tier(&decision, &next, &reconciliation.failures)?;
        decision = next;
    }
    bail!("quota reconciliation did not converge within the candidate bound")
}

#[expect(
    clippy::too_many_arguments,
    reason = "execution reconciliation keeps every policy input explicit"
)]
pub fn resolve_task_economy_for_execution(
    db: &Database,
    task: &Task,
    action: AgentAction,
    overrides: EconomyOverrides,
    requested_mode: Option<&str>,
    constrained_agent_id: Option<String>,
    task_effort: Option<ReasoningEffort>,
    task_source: Option<String>,
    transport_eligibility: TransportEligibility,
    escalation_request: Option<EscalationRequest>,
    lineage: impl Into<String>,
) -> Result<EconomyDecision> {
    resolve_task_economy_for_execution_with_refresher(
        db,
        task,
        action,
        overrides,
        requested_mode,
        constrained_agent_id,
        task_effort,
        task_source,
        transport_eligibility,
        escalation_request,
        lineage,
        &HashSet::new(),
        &ProviderQuotaRefresher,
    )
}

pub fn resolve_action_economy(
    db: &Database,
    action: AgentAction,
    overrides: EconomyOverrides,
    transport_eligibility: TransportEligibility,
) -> Result<EconomyDecision> {
    resolve_action_economy_with_failures(
        db,
        action,
        overrides,
        transport_eligibility,
        &BTreeMap::new(),
    )
}

fn resolve_action_economy_with_failures(
    db: &Database,
    action: AgentAction,
    overrides: EconomyOverrides,
    transport_eligibility: TransportEligibility,
    quota_refresh_failures: &BTreeMap<String, String>,
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
        quota_refresh_failures,
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
        escalation_request: None,
        lineage: format!("action:{}", action.as_str()),
    })
}

pub fn resolve_action_economy_for_execution_with_refresher(
    db: &Database,
    action: AgentAction,
    overrides: EconomyOverrides,
    transport_eligibility: TransportEligibility,
    refresher: &dyn QuotaRefresher,
) -> Result<EconomyDecision> {
    let mut decision = resolve_action_economy_with_failures(
        db,
        action,
        overrides.clone(),
        transport_eligibility,
        &BTreeMap::new(),
    )?;
    let mut failures = BTreeMap::new();
    let maximum_passes = decision.schedule.candidates.len().saturating_add(1);
    for _ in 0..maximum_passes {
        let reconciliation = reconcile_quota_candidates(db, &decision, refresher)?;
        if !reconciliation.attempted {
            apply_refresher_capability_metadata(db, &mut decision, refresher)?;
            return Ok(decision);
        }
        failures.extend(reconciliation.failures.clone());
        let next = resolve_action_economy_with_failures(
            db,
            action,
            overrides.clone(),
            transport_eligibility,
            &failures,
        )?;
        ensure_refresh_failures_do_not_promote_tier(&decision, &next, &reconciliation.failures)?;
        decision = next;
    }
    bail!("quota reconciliation did not converge within the candidate bound")
}

pub fn resolve_action_economy_for_execution(
    db: &Database,
    action: AgentAction,
    overrides: EconomyOverrides,
    transport_eligibility: TransportEligibility,
) -> Result<EconomyDecision> {
    resolve_action_economy_for_execution_with_refresher(
        db,
        action,
        overrides,
        transport_eligibility,
        &ProviderQuotaRefresher,
    )
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
    resolve_run_invocation_economy_with_failures(
        db,
        task,
        agent_id,
        model,
        effort,
        purpose,
        transport_eligibility,
        &BTreeMap::new(),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "provider invocation resolution keeps quota evidence explicit"
)]
fn resolve_run_invocation_economy_with_failures(
    db: &Database,
    task: &Task,
    agent_id: &str,
    model: Option<String>,
    effort: Option<ReasoningEffort>,
    purpose: &str,
    transport_eligibility: TransportEligibility,
    quota_refresh_failures: &BTreeMap<String, String>,
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
        quota_refresh_failures,
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
        escalation_request: None,
        lineage: format!("{purpose}:task:{}", task.id),
    })?;
    decision.resolution.ok_or_else(|| {
        anyhow::anyhow!(
            "agent '{agent_id}' is not eligible for provider invocation '{purpose}': {}",
            decision.schedule.explanation
        )
    })
}

pub fn resolve_run_invocation_economy_for_execution(
    db: &Database,
    task: &Task,
    agent_id: &str,
    model: Option<String>,
    effort: Option<ReasoningEffort>,
    purpose: &str,
    transport_eligibility: TransportEligibility,
) -> Result<EconomyResolution> {
    let preliminary = resolve_run_invocation_economy_with_failures(
        db,
        task,
        agent_id,
        model.clone(),
        effort,
        purpose,
        transport_eligibility,
        &BTreeMap::new(),
    )?;
    let preliminary_decision = EconomyDecision {
        schedule: ScheduleDecision {
            task_id: task.id.clone(),
            selected_agent_id: Some(preliminary.agent.id.clone()),
            candidates: vec![CandidateEvaluation {
                agent_id: preliminary.agent.id.clone(),
                backend: preliminary.agent.backend.clone(),
                execution_mode: preliminary.agent.execution_mode.clone(),
                priority: preliminary.agent.priority,
                quota_remaining_percent: preliminary.agent.quota_remaining_percent,
                quota_reset_at: preliminary.agent.quota_reset_at.clone(),
                capacity_score: None,
                quota_observation: quota_observation_for(
                    &preliminary.agent,
                    db.quota_reserve()?,
                    current_epoch(),
                    None,
                ),
                resolved_model: preliminary.execution.model.clone(),
                economy_tier: preliminary.record.tier,
                status: CandidateStatus::Eligible,
            }],
            selection_reason: SelectionReason::SingleEligibleCandidate,
            explanation: String::new(),
        },
        resolution: Some(preliminary),
    };
    let reconciliation =
        reconcile_quota_candidates(db, &preliminary_decision, &ProviderQuotaRefresher)?;
    if let Some(error) = reconciliation.failures.get(agent_id) {
        bail!("quota refresh failed for explicitly selected agent '{agent_id}': {error}");
    }
    resolve_run_invocation_economy_with_failures(
        db,
        task,
        agent_id,
        model,
        effort,
        purpose,
        transport_eligibility,
        &reconciliation.failures,
    )
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
    use std::sync::Mutex;
    use tempfile::TempDir;

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

    fn mark_quota_fresh(agent: &mut AgentDefinition) {
        agent.quota_checked_at = Some(current_epoch().to_string());
        agent.quota_source = Some("test".into());
    }

    struct FakeQuotaRefresher {
        outcomes: BTreeMap<String, std::result::Result<i64, String>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeQuotaRefresher {
        fn new(
            outcomes: impl IntoIterator<Item = (&'static str, std::result::Result<i64, String>)>,
        ) -> Self {
            Self {
                outcomes: outcomes
                    .into_iter()
                    .map(|(agent, outcome)| (agent.to_owned(), outcome))
                    .collect(),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl QuotaRefresher for FakeQuotaRefresher {
        fn supports(&self, agent: &AgentDefinition) -> bool {
            agent.backend == "codex"
        }

        fn refresh(
            &self,
            db: &Database,
            agent: &AgentDefinition,
        ) -> std::result::Result<(), String> {
            self.calls.lock().unwrap().push(agent.id.clone());
            match self
                .outcomes
                .get(&agent.id)
                .cloned()
                .unwrap_or_else(|| Err("unexpected refresh".into()))
            {
                Ok(remaining) => db
                    .set_agent_quota(&agent.id, remaining, None)
                    .map_err(|error| error.to_string())
                    .and_then(|updated| {
                        updated
                            .then_some(())
                            .ok_or_else(|| "agent disappeared during fake refresh".into())
                    }),
                Err(error) => Err(error),
            }
        }
    }

    fn scheduling_db(agents: &[AgentDefinition]) -> (TempDir, Database, Task) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::init(directory.path().join("orc.db")).unwrap();
        let project_id = database.create_project("quota scheduling").unwrap();
        for agent in agents {
            database.insert_agent(agent).unwrap();
        }
        let task_id = database
            .insert_task(
                project_id,
                "Quota task",
                "Exercise quota-aware scheduling",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let task = database.get_task(&task_id).unwrap().unwrap();
        (directory, database, task)
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
        economy_with_escalation(
            task, agents, costs, overrides, profiles, template, transport, None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "test fixture exposes policy input"
    )]
    fn economy_with_escalation(
        task: &Task,
        agents: &[AgentDefinition],
        costs: &EconomyCostConfiguration,
        overrides: EconomyOverrides,
        profiles: &BTreeMap<String, AgentActionProfile>,
        template: &ExecutionTemplate,
        transport: TransportEligibility,
        escalation_request: Option<EscalationRequest>,
    ) -> EconomyDecision {
        resolve_economy(EconomyResolverInput {
            action: AgentAction::Code,
            candidates: agents,
            task: Some(task),
            required_capabilities: &task.required_capabilities(),
            requested_mode: Some(registry::AUTOMATED),
            busy_agents: &HashSet::new(),
            quota_reserve: 0,
            quota_refresh_failures: &BTreeMap::new(),
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
            escalation_request,
            lineage: "test".into(),
        })
        .unwrap()
    }

    fn previous_resolution(tier: EconomyTier) -> ResolutionRecord {
        ResolutionRecord {
            selected_agent: "cheap".into(),
            selected_model: Some("small".into()),
            effort: Some(ReasoningEffort::Low),
            tier,
            source: "agent".into(),
            escalation_reason: None,
            input_lineage: "previous".into(),
            escalation: None,
        }
    }

    #[test]
    fn escalation_policy_distinguishes_retry_and_observable_non_convergence() {
        let configuration = EscalationPolicyConfiguration::default();
        let previous = previous_resolution(EconomyTier::Default);
        for observation in [
            EscalationObservation::Retry,
            EscalationObservation::SingleSemanticRevision,
            EscalationObservation::SingleValidationFailure,
            EscalationObservation::InfrastructureValidationFailure,
            EscalationObservation::TransientProviderFailure,
            EscalationObservation::RiskMetadataOnly,
        ] {
            assert!(matches!(
                evaluate_escalation_policy(EscalationPolicyInput {
                    observation,
                    previous_provider_invocation_id: Some(7),
                    previous_resolution: Some(&previous),
                    previous_attempt: 1,
                    policy_attempt: 1,
                    configuration: &configuration,
                }),
                EscalationDecision::NoEscalation { .. }
            ));
        }
        let EscalationDecision::Escalate(request) =
            evaluate_escalation_policy(EscalationPolicyInput {
                observation: EscalationObservation::ValidationRepairNonConvergence,
                previous_provider_invocation_id: Some(7),
                previous_resolution: Some(&previous),
                previous_attempt: MAX_VALIDATION_REPAIRS_FOR_POLICY_TEST,
                policy_attempt: 1,
                configuration: &configuration,
            })
        else {
            panic!("bounded validation non-convergence should request escalation")
        };
        assert_eq!(
            request.lineage.requested_minimum_tier,
            EconomyTier::Escalation
        );
        assert_eq!(
            request.lineage.trigger,
            EscalationTrigger::ValidationRepairNonConvergence
        );
    }

    const MAX_VALIDATION_REPAIRS_FOR_POLICY_TEST: usize = 3;

    #[test]
    fn escalation_is_bounded_at_the_maximum_tier() {
        let previous = previous_resolution(EconomyTier::Exceptional);
        assert!(matches!(
            evaluate_escalation_policy(EscalationPolicyInput {
                observation: EscalationObservation::SemanticRevisionNonConvergence,
                previous_provider_invocation_id: Some(9),
                previous_resolution: Some(&previous),
                previous_attempt: 1,
                policy_attempt: 2,
                configuration: &EscalationPolicyConfiguration::default(),
            }),
            EscalationDecision::Exhausted { .. }
        ));
    }

    #[test]
    fn resolver_applies_policy_request_without_selecting_a_model_in_policy() {
        let task = test_task(vec!["code"]);
        let mut cheap = test_agent("cheap", 1_000, vec!["code"]);
        cheap.model = Some("small".into());
        let mut next = test_agent("next", 1, vec!["code"]);
        next.model = Some("medium".into());
        let costs = EconomyCostConfiguration {
            model_costs: BTreeMap::from([("small".into(), 1.0), ("medium".into(), 2.0)]),
            unknown_tier: EconomyTier::Unknown,
        };
        let previous = previous_resolution(EconomyTier::Default);
        let EscalationDecision::Escalate(request) =
            evaluate_escalation_policy(EscalationPolicyInput {
                observation: EscalationObservation::SemanticRevisionNonConvergence,
                previous_provider_invocation_id: Some(11),
                previous_resolution: Some(&previous),
                previous_attempt: 1,
                policy_attempt: 1,
                configuration: &EscalationPolicyConfiguration::default(),
            })
        else {
            panic!("expected escalation request")
        };
        assert_eq!(request.lineage.previous_model.as_deref(), Some("small"));
        let decision = economy_with_escalation(
            &task,
            &[cheap, next],
            &costs,
            EconomyOverrides::default(),
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
            Some(request.clone()),
        );
        let resolution = decision.resolution.unwrap();
        assert_eq!(resolution.agent.id, "next");
        assert_eq!(resolution.record.source, "policy_escalation");
        assert_eq!(resolution.record.escalation_reason, Some(request.reason));
        assert_eq!(
            resolution.record.escalation.unwrap().trigger,
            EscalationTrigger::SemanticRevisionNonConvergence
        );
        assert!(
            decision
                .schedule
                .candidates
                .iter()
                .any(|candidate| matches!(
                    candidate.status,
                    CandidateStatus::Rejected(RejectionReason::BelowEscalationTier { .. })
                ))
        );
    }

    #[test]
    fn escalation_never_bypasses_eligibility_and_operator_override_stays_distinct() {
        let task = test_task(vec!["code"]);
        let mut cheap = test_agent("cheap", 100, vec!["code"]);
        cheap.model = Some("small".into());
        let mut unclassified = test_agent("unclassified", 200, vec!["code"]);
        unclassified.model = Some("unpriced".into());
        let costs = EconomyCostConfiguration {
            model_costs: BTreeMap::from([("small".into(), 1.0)]),
            unknown_tier: EconomyTier::Unknown,
        };
        let previous = previous_resolution(EconomyTier::Default);
        let EscalationDecision::Escalate(request) =
            evaluate_escalation_policy(EscalationPolicyInput {
                observation: EscalationObservation::ValidationRepairNonConvergence,
                previous_provider_invocation_id: Some(12),
                previous_resolution: Some(&previous),
                previous_attempt: 3,
                policy_attempt: 1,
                configuration: &EscalationPolicyConfiguration::default(),
            })
        else {
            panic!("expected escalation request")
        };
        let unavailable = economy_with_escalation(
            &task,
            &[cheap.clone(), unclassified],
            &costs,
            EconomyOverrides::default(),
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
            Some(request.clone()),
        );
        assert!(unavailable.resolution.is_none());
        assert!(
            unavailable
                .schedule
                .candidates
                .iter()
                .all(|candidate| matches!(
                    candidate.status,
                    CandidateStatus::Rejected(RejectionReason::BelowEscalationTier { .. })
                ))
        );

        let operator = economy_with_escalation(
            &task,
            &[cheap],
            &costs,
            EconomyOverrides {
                agent_id: Some("cheap".into()),
                model: Some("small".into()),
                effort: Some(ReasoningEffort::Low),
            },
            &BTreeMap::new(),
            &ExecutionTemplate::default(),
            TransportEligibility::Strict,
            Some(request),
        )
        .resolution
        .unwrap();
        assert_eq!(operator.record.source, "operator_override");
        assert_eq!(operator.record.escalation_reason, None);
        assert_eq!(operator.record.escalation, None);
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
        mark_quota_fresh(&mut fake);
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
        mark_quota_fresh(&mut priority);
        let mut healthy = test_agent("healthy", 100, vec!["code"]);
        healthy.quota_remaining_percent = Some(80);
        mark_quota_fresh(&mut healthy);
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
        mark_quota_fresh(&mut nine);
        let mut ten = test_agent("ten", 100, vec!["code"]);
        ten.quota_remaining_percent = Some(10);
        mark_quota_fresh(&mut ten);
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
        mark_quota_fresh(&mut soon);
        soon.quota_reset_at = Some((current_epoch() + 86_400).to_string());
        let mut far = test_agent("far", 100, vec!["code"]);
        far.quota_remaining_percent = Some(50);
        mark_quota_fresh(&mut far);
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
        mark_quota_fresh(&mut soon);
        soon.quota_reset_at = Some((current_epoch() + 86_400).to_string());
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
        mark_quota_fresh(&mut a);
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
        mark_quota_fresh(&mut agent);
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
    fn freshness_policy_classifies_never_checked_fresh_and_stale() {
        let now = 1_800_000_000;
        let policy = QuotaFreshnessPolicy::default();
        let mut agent = test_agent("quota", 100, vec!["code"]);
        assert_eq!(policy.classify(&agent, now), QuotaFreshness::NeverChecked);
        agent.quota_checked_at = Some((now - 60).to_string());
        assert_eq!(policy.classify(&agent, now), QuotaFreshness::Fresh);
        agent.quota_checked_at = Some((now - policy.max_age.as_secs() as i64 - 1).to_string());
        assert_eq!(policy.classify(&agent, now), QuotaFreshness::Stale);
    }

    #[test]
    fn stale_low_quota_is_not_rejected_by_read_only_resolution() {
        let task = test_task(vec!["code"]);
        let mut agent = test_agent("stale", 100, vec!["code"]);
        agent.quota_remaining_percent = Some(2);
        agent.quota_checked_at = Some("2000-01-01 00:00:00".into());
        agent.quota_source = Some("provider".into());
        let decision = schedule_with_quota_reserve(&task, &[agent], None, 20).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("stale"));
        assert_eq!(
            decision.candidates[0].quota_observation.freshness,
            QuotaFreshness::Stale
        );
        assert!(
            decision
                .format_explanation()
                .contains("refresh required before execution")
        );
    }

    #[test]
    fn read_only_database_resolution_reports_stale_without_mutating_it() {
        let mut agent = test_agent("read-only", 100, vec!["code"]);
        agent.quota_remaining_percent = Some(2);
        agent.quota_checked_at = Some("2000-01-01 00:00:00".into());
        agent.quota_source = Some("provider".into());
        let (_directory, database, _task) = scheduling_db(&[agent]);
        database.set_quota_reserve(20).unwrap();
        let decision = resolve_action_economy(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
        )
        .unwrap();
        assert_eq!(
            decision.schedule.candidates[0].quota_observation.freshness,
            QuotaFreshness::Stale
        );
        let persisted = database.get_agent("read-only").unwrap().unwrap();
        assert_eq!(
            persisted.quota_checked_at.as_deref(),
            Some("2000-01-01 00:00:00")
        );
        assert_eq!(persisted.quota_remaining_percent, Some(2));
    }

    #[test]
    fn stale_and_unknown_quota_refresh_before_execution_while_fresh_does_not() {
        let mut stale = test_agent("stale", 100, vec!["code"]);
        stale.quota_remaining_percent = Some(2);
        stale.quota_checked_at = Some("2000-01-01 00:00:00".into());
        stale.quota_source = Some("provider".into());
        let (_directory, database, _task) = scheduling_db(&[stale]);
        database.set_quota_reserve(20).unwrap();
        let refresher = FakeQuotaRefresher::new([("stale", Ok(80))]);
        let decision = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &refresher,
        )
        .unwrap();
        assert_eq!(
            decision.schedule.selected_agent_id.as_deref(),
            Some("stale")
        );
        assert_eq!(refresher.calls(), vec!["stale"]);
        assert_eq!(
            decision.schedule.candidates[0].quota_observation.freshness,
            QuotaFreshness::Fresh
        );

        let no_refresh = FakeQuotaRefresher::new([]);
        let repeated = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &no_refresh,
        )
        .unwrap();
        assert_eq!(
            repeated.schedule.selected_agent_id.as_deref(),
            Some("stale")
        );
        assert!(no_refresh.calls().is_empty());

        database.clear_agent_quota("stale").unwrap();
        let unknown = FakeQuotaRefresher::new([("stale", Ok(75))]);
        resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &unknown,
        )
        .unwrap();
        assert_eq!(unknown.calls(), vec!["stale"]);
    }

    #[test]
    fn refreshed_below_reserve_remains_ineligible() {
        let mut agent = test_agent("low", 100, vec!["code"]);
        agent.quota_remaining_percent = Some(1);
        agent.quota_checked_at = Some("2000-01-01 00:00:00".into());
        let (_directory, database, _task) = scheduling_db(&[agent]);
        database.set_quota_reserve(20).unwrap();
        let refresher = FakeQuotaRefresher::new([("low", Ok(10))]);
        let decision = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &refresher,
        )
        .unwrap();
        assert!(decision.resolution.is_none());
        assert!(matches!(
            decision.schedule.candidates[0].status,
            CandidateStatus::Rejected(RejectionReason::QuotaReserve {
                remaining: 10,
                reserve: 20
            })
        ));
    }

    #[test]
    fn refresh_failure_cannot_silently_promote_to_expensive_tier() {
        let mut cheap = test_agent("cheap", 10, vec!["code"]);
        cheap.model = Some("cheap-model".into());
        cheap.quota_remaining_percent = Some(1);
        cheap.quota_checked_at = Some("2000-01-01 00:00:00".into());
        let mut expensive = test_agent("expensive", 1000, vec!["code"]);
        expensive.model = Some("expensive-model".into());
        expensive.quota_remaining_percent = Some(90);
        mark_quota_fresh(&mut expensive);
        let (_directory, database, _task) = scheduling_db(&[cheap, expensive]);
        database
            .set_economy_cost_configuration(&EconomyCostConfiguration {
                model_costs: BTreeMap::from([
                    ("cheap-model".into(), 1.0),
                    ("expensive-model".into(), 3.0),
                ]),
                unknown_tier: EconomyTier::Unknown,
            })
            .unwrap();
        let refresher = FakeQuotaRefresher::new([("cheap", Err("timeout".into()))]);
        let error = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &refresher,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cheapest eligible economy tier"));
        assert!(error.to_string().contains("cheap: timeout"));
        assert_eq!(refresher.calls(), vec!["cheap"]);
    }

    #[test]
    fn same_tier_refresh_failure_can_use_confirmed_alternative() {
        let mut failed = test_agent("failed", 200, vec!["code"]);
        failed.quota_checked_at = Some("2000-01-01 00:00:00".into());
        let mut alternative = test_agent("alternative", 100, vec!["code"]);
        alternative.quota_remaining_percent = Some(70);
        mark_quota_fresh(&mut alternative);
        let (_directory, database, _task) = scheduling_db(&[failed, alternative]);
        let refresher = FakeQuotaRefresher::new([("failed", Err("timeout".into()))]);
        let decision = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &refresher,
        )
        .unwrap();
        assert_eq!(
            decision.schedule.selected_agent_id.as_deref(),
            Some("alternative")
        );
        let failed = decision
            .schedule
            .candidates
            .iter()
            .find(|candidate| candidate.agent_id == "failed")
            .unwrap();
        assert!(matches!(
            failed.status,
            CandidateStatus::Rejected(RejectionReason::QuotaRefreshFailed { .. })
        ));
    }

    #[test]
    fn explicit_override_refreshes_and_cannot_bypass_confirmed_reserve() {
        let mut selected = test_agent("selected", 1, vec!["code"]);
        selected.quota_remaining_percent = Some(2);
        selected.quota_checked_at = Some("2000-01-01 00:00:00".into());
        let other = test_agent("other", 100, vec!["code"]);
        let (_directory, database, _task) = scheduling_db(&[selected, other]);
        database.set_quota_reserve(20).unwrap();
        let refresher = FakeQuotaRefresher::new([("selected", Ok(10))]);
        let decision = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides {
                agent_id: Some("selected".into()),
                ..EconomyOverrides::default()
            },
            TransportEligibility::Strict,
            &refresher,
        )
        .unwrap();
        assert!(decision.resolution.is_none());
        assert_eq!(refresher.calls(), vec!["selected"]);
        assert!(matches!(
            decision
                .schedule
                .candidates
                .iter()
                .find(|candidate| candidate.agent_id == "selected")
                .unwrap()
                .status,
            CandidateStatus::Rejected(RejectionReason::QuotaReserve { .. })
        ));
    }

    #[test]
    fn selected_resolution_lineage_contains_fresh_quota_observation() {
        let mut agent = test_agent("lineage", 100, vec!["code"]);
        agent.quota_remaining_percent = Some(66);
        mark_quota_fresh(&mut agent);
        let (_directory, database, _task) = scheduling_db(&[agent]);
        database.set_quota_reserve(15).unwrap();
        let refresher = FakeQuotaRefresher::new([]);
        let decision = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &refresher,
        )
        .unwrap();
        let lineage: serde_json::Value =
            serde_json::from_str(&decision.resolution.unwrap().record.input_lineage).unwrap();
        assert_eq!(lineage["quota"]["remaining_percent"], 66);
        assert_eq!(lineage["quota"]["freshness"], "fresh");
        assert_eq!(lineage["quota"]["reserve_percent"], 15);
        assert_eq!(lineage["quota"]["source"], "test");
        assert_eq!(lineage["selection_reason"], "single_eligible_candidate");
        assert!(
            lineage["selection_explanation"]
                .as_str()
                .is_some_and(|value| value.contains("lineage"))
        );
        assert!(refresher.calls().is_empty());
    }

    #[test]
    fn refreshed_quota_preserves_cheapest_tier_before_priority() {
        let mut cheap = test_agent("cheap", 1, vec!["code"]);
        cheap.model = Some("cheap-model".into());
        cheap.quota_remaining_percent = Some(1);
        cheap.quota_checked_at = Some("2000-01-01 00:00:00".into());
        let mut expensive = test_agent("expensive", 10_000, vec!["code"]);
        expensive.model = Some("expensive-model".into());
        expensive.quota_remaining_percent = Some(90);
        mark_quota_fresh(&mut expensive);
        let (_directory, database, _task) = scheduling_db(&[cheap, expensive]);
        database
            .set_economy_cost_configuration(&EconomyCostConfiguration {
                model_costs: BTreeMap::from([
                    ("cheap-model".into(), 1.0),
                    ("expensive-model".into(), 3.0),
                ]),
                unknown_tier: EconomyTier::Unknown,
            })
            .unwrap();
        let refresher = FakeQuotaRefresher::new([("cheap", Ok(75))]);
        let decision = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &refresher,
        )
        .unwrap();
        assert_eq!(
            decision.schedule.selected_agent_id.as_deref(),
            Some("cheap")
        );
        assert_eq!(
            decision.schedule.selection_reason,
            SelectionReason::CheapestEconomyTier
        );
        assert_eq!(refresher.calls(), vec!["cheap"]);
    }

    #[test]
    fn next_tier_is_refreshed_only_after_cheaper_tier_is_confirmed_insufficient() {
        let mut cheap = test_agent("cheap", 100, vec!["code"]);
        cheap.model = Some("cheap-model".into());
        cheap.quota_remaining_percent = Some(1);
        cheap.quota_checked_at = Some("2000-01-01 00:00:00".into());
        let mut expensive = test_agent("expensive", 100, vec!["code"]);
        expensive.model = Some("expensive-model".into());
        expensive.quota_remaining_percent = Some(1);
        expensive.quota_checked_at = Some("2000-01-01 00:00:00".into());
        let (_directory, database, _task) = scheduling_db(&[cheap, expensive]);
        database.set_quota_reserve(20).unwrap();
        database
            .set_economy_cost_configuration(&EconomyCostConfiguration {
                model_costs: BTreeMap::from([
                    ("cheap-model".into(), 1.0),
                    ("expensive-model".into(), 3.0),
                ]),
                unknown_tier: EconomyTier::Unknown,
            })
            .unwrap();
        let refresher = FakeQuotaRefresher::new([("cheap", Ok(10)), ("expensive", Ok(80))]);
        let decision = resolve_action_economy_for_execution_with_refresher(
            &database,
            AgentAction::Code,
            EconomyOverrides::default(),
            TransportEligibility::Strict,
            &refresher,
        )
        .unwrap();
        assert_eq!(
            decision.schedule.selected_agent_id.as_deref(),
            Some("expensive")
        );
        assert_eq!(refresher.calls(), vec!["cheap", "expensive"]);
    }

    #[test]
    fn escalation_reconciles_required_tier_before_declaring_it_unavailable() {
        let mut baseline = test_agent("baseline", 100, vec!["code", "terminal"]);
        baseline.model = Some("cheap-model".into());
        mark_quota_fresh(&mut baseline);
        baseline.quota_remaining_percent = Some(100);
        let mut escalated = test_agent("escalated", 10, vec!["code", "terminal"]);
        escalated.model = Some("strong-model".into());
        escalated.quota_remaining_percent = Some(1);
        escalated.quota_checked_at = Some("2000-01-01 00:00:00".into());
        let (_directory, database, task) = scheduling_db(&[baseline, escalated]);
        database.set_quota_reserve(20).unwrap();
        database
            .set_economy_cost_configuration(&EconomyCostConfiguration {
                model_costs: BTreeMap::from([
                    ("cheap-model".into(), 1.0),
                    ("strong-model".into(), 3.0),
                ]),
                unknown_tier: EconomyTier::Unknown,
            })
            .unwrap();
        let escalation = EscalationRequest {
            reason: "semantic non-convergence".into(),
            lineage: crate::registry::EscalationLineage {
                request_id: Some(1),
                trigger: EscalationTrigger::SemanticRevisionNonConvergence,
                previous_provider_invocation_id: 9,
                previous_tier: EconomyTier::Default,
                previous_model: Some("cheap-model".into()),
                previous_effort: None,
                previous_attempt: 1,
                requested_minimum_tier: EconomyTier::Escalation,
                policy_attempt: 1,
            },
        };
        let refresher = FakeQuotaRefresher::new([("escalated", Ok(70))]);
        let decision = resolve_task_economy_for_execution_with_refresher(
            &database,
            &task,
            AgentAction::Code,
            EconomyOverrides::default(),
            Some(registry::AUTOMATED),
            None,
            task.reasoning_effort,
            Some("task_contract".into()),
            TransportEligibility::Strict,
            Some(escalation),
            "quota_escalation_test",
            &HashSet::new(),
            &refresher,
        )
        .unwrap();
        assert_eq!(
            decision.schedule.selected_agent_id.as_deref(),
            Some("escalated"),
            "{decision:#?}"
        );
        assert_eq!(refresher.calls(), vec!["escalated"]);
    }

    #[test]
    fn unsupported_unknown_quota_uses_conservative_no_capacity_fallback() {
        let task = test_task(vec!["code"]);
        let mut agent = test_agent("copilot", 100, vec!["code"]);
        agent.backend = "copilot".into();
        let decision = schedule_with_quota_reserve(&task, &[agent], None, 20).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("copilot"));
        assert_eq!(decision.candidates[0].capacity_score, None);
        assert_eq!(
            decision.candidates[0].quota_observation.freshness,
            QuotaFreshness::NeverChecked
        );
        assert!(!decision.candidates[0].quota_observation.refresh_supported);
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
