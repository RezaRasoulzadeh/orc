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
    pub const fn all() -> [Self; 5] {
        [
            Self::Coder,
            Self::Reviewer,
            Self::Architect,
            Self::Researcher,
            Self::General,
        ]
    }

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

impl std::str::FromStr for ExecutionClass {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "coder" => Ok(Self::Coder),
            "reviewer" => Ok(Self::Reviewer),
            "architect" => Ok(Self::Architect),
            "researcher" => Ok(Self::Researcher),
            "general" => Ok(Self::General),
            _ => Err("expected coder, reviewer, architect, researcher, or general".into()),
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecutionTemplate {
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

pub fn resolve_with_template(
    role: &str,
    persisted: &ExecutionTemplate,
    configured_model: Option<&str>,
    configured_effort: Option<ReasoningEffort>,
    override_model: Option<String>,
    override_effort: Option<ReasoningEffort>,
) -> ExecutionResolution {
    let class = class_for_role(role);
    let prefix = match class {
        ExecutionClass::Coder => "CODER",
        ExecutionClass::Reviewer | ExecutionClass::Architect => "REVIEW",
        ExecutionClass::Researcher => "RESEARCH",
        ExecutionClass::General => "GENERAL",
    };
    let env_model = std::env::var(format!("ORC_{prefix}_MODEL")).ok();
    let env_effort = std::env::var(format!("ORC_{prefix}_REASONING_EFFORT"))
        .ok()
        .and_then(|v| ReasoningEffort::parse(&v).ok());
    let builtin_effort = match class {
        ExecutionClass::Coder => Some(ReasoningEffort::Low),
        ExecutionClass::Reviewer | ExecutionClass::Architect => Some(ReasoningEffort::High),
        ExecutionClass::Researcher => Some(ReasoningEffort::Medium),
        ExecutionClass::General => None,
    };
    let model = override_model
        .clone()
        .or_else(|| persisted.model.clone())
        .or(env_model.clone())
        .or_else(|| configured_model.map(str::to_owned));
    let reasoning_effort = override_effort
        .or(persisted.reasoning_effort)
        .or(env_effort)
        .or(builtin_effort)
        .or(configured_effort);
    let source = if override_model.is_some() || override_effort.is_some() {
        "override"
    } else if persisted.model.is_some() || persisted.reasoning_effort.is_some() {
        "persistent-template"
    } else if env_model.is_some() || env_effort.is_some() || builtin_effort.is_some() {
        "template"
    } else if configured_model.is_some() || configured_effort.is_some() {
        "agent"
    } else {
        "provider"
    };
    ExecutionResolution {
        class,
        model,
        reasoning_effort,
        source: source.into(),
    }
}

pub fn resolve(
    role: &str,
    configured_model: Option<&str>,
    configured_effort: Option<ReasoningEffort>,
    override_model: Option<String>,
    override_effort: Option<ReasoningEffort>,
) -> ExecutionResolution {
    resolve_with_template(
        role,
        &ExecutionTemplate::default(),
        configured_model,
        configured_effort,
        override_model,
        override_effort,
    )
}
