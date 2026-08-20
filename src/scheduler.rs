use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::registry::{self, AgentDefinition};
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
    pub status: CandidateStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReason {
    HighestPriority,
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
    let make_eval = |status: CandidateStatus| CandidateEvaluation {
        agent_id: agent.id.clone(),
        backend: agent.backend.clone(),
        execution_mode: agent.execution_mode.clone(),
        priority: agent.priority,
        quota_remaining_percent: agent.quota_remaining_percent,
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
    if registry::validate_backend(&agent.backend).is_err() {
        return make_eval(CandidateStatus::Rejected(
            RejectionReason::UnsupportedBackend {
                backend: agent.backend.clone(),
            },
        ));
    }
    if !is_backend_mode_supported(&agent.backend, &agent.execution_mode) {
        return make_eval(CandidateStatus::Rejected(
            RejectionReason::UnsupportedMode {
                mode: agent.execution_mode.clone(),
            },
        ));
    }

    // 4. required task capabilities must be satisfied
    let required = task.required_capabilities();
    let missing: Vec<String> = required
        .iter()
        .filter(|cap| !agent.capabilities.contains(cap))
        .cloned()
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
    let mut eligible = Vec::new();
    let mut rejected = Vec::new();

    for agent in agents {
        let eval =
            evaluate_candidate_with_quota_reserve(agent, task, requested_mode, quota_reserve);
        match eval.status {
            CandidateStatus::Eligible => eligible.push(eval),
            CandidateStatus::Rejected(_) => rejected.push(eval),
        }
    }

    // Rule 7 (highest priority) & Rule 8 (lexicographic tie-break)
    eligible.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.agent_id.cmp(&b.agent_id))
    });

    rejected.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));

    let selected_agent_id = eligible.first().map(|c| c.agent_id.clone());

    let (selection_reason, explanation) = if let Some(ref winner) = selected_agent_id {
        if eligible.len() == 1 {
            (
                SelectionReason::SingleEligibleCandidate,
                format!("{winner} selected by highest priority."),
            )
        } else if eligible[0].priority > eligible[1].priority {
            (
                SelectionReason::HighestPriority,
                format!("{winner} selected by highest priority."),
            )
        } else {
            (
                SelectionReason::LexicographicTieBreak,
                format!("{winner} selected by lexicographic tie-break."),
            )
        }
    } else {
        (
            SelectionReason::NoEligibleCandidates,
            format!(
                "No eligible agent satisfies requirements for task '{}'.",
                task.id
            ),
        )
    };

    let mut all_candidates = eligible;
    all_candidates.extend(rejected);

    Ok(ScheduleDecision {
        task_id: task.id.clone(),
        selected_agent_id,
        candidates: all_candidates,
        selection_reason,
        explanation,
    })
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
    let mut eligible = Vec::new();
    let mut rejected = Vec::new();
    for agent in agents {
        let evaluation = evaluate_candidate_with_busy_and_quota_reserve(
            agent,
            task,
            requested_mode,
            busy_agents,
            quota_reserve,
        );
        match evaluation.status {
            CandidateStatus::Eligible => eligible.push(evaluation),
            CandidateStatus::Rejected(_) => rejected.push(evaluation),
        }
    }
    eligible.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.agent_id.cmp(&b.agent_id))
    });
    rejected.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    let selected_agent_id = eligible.first().map(|candidate| candidate.agent_id.clone());
    let explanation = selected_agent_id.as_ref().map_or_else(
        || {
            format!(
                "No eligible agent satisfies requirements for task '{}'.",
                task.id
            )
        },
        |id| format!("{id} selected by deterministic priority and lexicographic order."),
    );
    let selection_reason = match selected_agent_id {
        None => SelectionReason::NoEligibleCandidates,
        Some(_) if eligible.len() == 1 => SelectionReason::SingleEligibleCandidate,
        Some(_) if eligible[0].priority > eligible[1].priority => SelectionReason::HighestPriority,
        Some(_) => SelectionReason::LexicographicTieBreak,
    };
    let selected = selected_agent_id.clone();
    let mut candidates = eligible;
    candidates.extend(rejected);
    Ok(ScheduleDecision {
        task_id: task.id.clone(),
        selected_agent_id: selected,
        candidates,
        selection_reason,
        explanation,
    })
}

pub fn validate_override(agent: &AgentDefinition, task: &Task) -> Result<()> {
    if !agent.enabled {
        bail!("agent '{}' is disabled", agent.id);
    }
    if agent.status != registry::AVAILABLE {
        let reason = agent
            .unavailable_reason
            .as_deref()
            .unwrap_or("no reason provided");
        bail!("agent '{}' is unavailable: {}", agent.id, reason);
    }
    if registry::validate_backend(&agent.backend).is_err() {
        bail!(
            "agent '{}' has unsupported backend '{}'",
            agent.id,
            agent.backend
        );
    }
    if !is_backend_mode_supported(&agent.backend, &agent.execution_mode) {
        bail!(
            "agent '{}' has unsupported execution mode '{}' for backend '{}'",
            agent.id,
            agent.execution_mode,
            agent.backend
        );
    }
    let required = task.required_capabilities();
    let missing: Vec<String> = required
        .iter()
        .filter(|cap| !agent.capabilities.contains(cap))
        .cloned()
        .collect();
    if !missing.is_empty() {
        bail!(
            "agent '{}' lacks required capabilities: {}",
            agent.id,
            missing.join(", ")
        );
    }
    if agent.quota_remaining_percent == Some(0) {
        bail!("agent '{}' has exhausted quota (0% remaining)", agent.id);
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
        }
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
                capability: "terminal".to_string()
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
