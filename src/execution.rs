use crate::registry::ReasoningEffort;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExecutionClass {
    Coder,
    Reviewer,
    Architect,
    Researcher,
    General,
}

impl ExecutionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
            Self::Architect => "architect",
            Self::Researcher => "researcher",
            Self::General => "general",
        }
    }
}

pub fn normalize_role(role: &str) -> String {
    role.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

pub fn class_for_role(role: &str) -> ExecutionClass {
    match normalize_role(role).as_str() {
        "developer" | "dev" | "coder" | "software-engineer" | "software-developer"
        | "backend-engineer" | "frontend-engineer" => ExecutionClass::Coder,
        "reviewer" | "review" | "code-reviewer" => ExecutionClass::Reviewer,
        "architect" | "architecture" => ExecutionClass::Architect,
        "researcher" | "research" => ExecutionClass::Researcher,
        _ => ExecutionClass::General,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecutionResolution {
    pub class: ExecutionClass,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub source: String,
}

pub fn resolve(
    role: &str,
    configured_model: Option<&str>,
    configured_effort: Option<ReasoningEffort>,
    override_model: Option<String>,
    override_effort: Option<ReasoningEffort>,
) -> ExecutionResolution {
    let class = class_for_role(role);
    let (default_model, default_effort) = match class {
        ExecutionClass::Coder => (
            std::env::var("ORC_CODER_MODEL").ok(),
            Some(ReasoningEffort::Low),
        ),
        ExecutionClass::Reviewer | ExecutionClass::Architect => (
            std::env::var("ORC_REVIEW_MODEL").ok(),
            Some(ReasoningEffort::High),
        ),
        ExecutionClass::Researcher => (
            std::env::var("ORC_RESEARCH_MODEL").ok(),
            Some(ReasoningEffort::Medium),
        ),
        ExecutionClass::General => (None, None),
    };
    let model = override_model
        .clone()
        .or(default_model.clone())
        .or_else(|| configured_model.map(str::to_owned));
    let reasoning_effort = override_effort.or(default_effort).or(configured_effort);
    let source = if override_model.is_some() || override_effort.is_some() {
        "override"
    } else if default_model.is_some() || default_effort.is_some() {
        "template"
    } else if configured_model.is_some() || configured_effort.is_some() {
        "agent"
    } else {
        "template"
    };
    ExecutionResolution {
        class,
        model,
        reasoning_effort,
        source: source.into(),
    }
}
