use orc::execution::{ExecutionClass, ExecutionTemplate, resolve, resolve_with_template};
use orc::registry::ReasoningEffort;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_env<T>(coder: Option<&str>, review: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap();
    let old_coder = std::env::var_os("ORC_CODER_MODEL");
    let old_review = std::env::var_os("ORC_REVIEW_MODEL");
    match coder {
        Some(value) => unsafe { std::env::set_var("ORC_CODER_MODEL", value) },
        None => unsafe { std::env::remove_var("ORC_CODER_MODEL") },
    }
    match review {
        Some(value) => unsafe { std::env::set_var("ORC_REVIEW_MODEL", value) },
        None => unsafe { std::env::remove_var("ORC_REVIEW_MODEL") },
    }
    let result = f();
    match old_coder {
        Some(value) => unsafe { std::env::set_var("ORC_CODER_MODEL", value) },
        None => unsafe { std::env::remove_var("ORC_CODER_MODEL") },
    }
    match old_review {
        Some(value) => unsafe { std::env::set_var("ORC_REVIEW_MODEL", value) },
        None => unsafe { std::env::remove_var("ORC_REVIEW_MODEL") },
    }
    result
}

#[test]
fn roles_map_to_execution_classes() {
    for role in ["developer", "dev", "coder", "software-engineer"] {
        assert_eq!(orc::execution::class_for_role(role), ExecutionClass::Coder);
    }
    assert_eq!(
        orc::execution::class_for_role("reviewer"),
        ExecutionClass::Reviewer
    );
    assert_eq!(
        orc::execution::class_for_role("architect"),
        ExecutionClass::Architect
    );
    assert_eq!(
        orc::execution::class_for_role("researcher"),
        ExecutionClass::Researcher
    );
    assert_eq!(
        orc::execution::class_for_role("unknown"),
        ExecutionClass::General
    );
}

#[test]
fn resolution_precedence_and_templates_are_explicit() {
    with_env(Some("coder-model"), Some("review-model"), || {
        let coder = resolve(
            "developer",
            Some("agent-model"),
            Some(ReasoningEffort::High),
            None,
            None,
        );
        assert_eq!(coder.model.as_deref(), Some("coder-model"));
        assert_eq!(coder.reasoning_effort, Some(ReasoningEffort::Low));
        assert_eq!(coder.source, "template");
        let overridden = resolve(
            "developer",
            Some("agent-model"),
            Some(ReasoningEffort::High),
            Some("explicit-model".into()),
            Some(ReasoningEffort::Medium),
        );
        assert_eq!(overridden.model.as_deref(), Some("explicit-model"));
        assert_eq!(overridden.reasoning_effort, Some(ReasoningEffort::Medium));
        let general = resolve(
            "unknown",
            Some("agent-model"),
            Some(ReasoningEffort::High),
            None,
            None,
        );
        assert_eq!(general.model.as_deref(), Some("agent-model"));
        assert_eq!(general.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(general.source, "agent");
    });
}

#[test]
fn persisted_template_precedes_environment_and_agent_defaults() {
    with_env(
        Some("environment-model"),
        Some("environment-review"),
        || {
            let persisted = ExecutionTemplate {
                model: Some("persistent-model".into()),
                reasoning_effort: Some(ReasoningEffort::Medium),
            };
            let resolved = resolve_with_template(
                "developer",
                &persisted,
                Some("agent-model"),
                Some(ReasoningEffort::High),
                None,
                None,
            );
            assert_eq!(resolved.model.as_deref(), Some("persistent-model"));
            assert_eq!(resolved.reasoning_effort, Some(ReasoningEffort::Medium));
            assert_eq!(resolved.source, "persistent-template");

            let overridden = resolve_with_template(
                "developer",
                &persisted,
                Some("agent-model"),
                Some(ReasoningEffort::High),
                Some("override-model".into()),
                Some(ReasoningEffort::Low),
            );
            assert_eq!(overridden.model.as_deref(), Some("override-model"));
            assert_eq!(overridden.reasoning_effort, Some(ReasoningEffort::Low));
            assert_eq!(overridden.source, "override");

            let env_only = resolve_with_template(
                "developer",
                &ExecutionTemplate::default(),
                Some("agent-model"),
                Some(ReasoningEffort::High),
                None,
                None,
            );
            assert_eq!(env_only.model.as_deref(), Some("environment-model"));
            assert_eq!(env_only.reasoning_effort, Some(ReasoningEffort::Low));

            let agent_only = resolve_with_template(
                "general",
                &ExecutionTemplate::default(),
                Some("agent-model"),
                Some(ReasoningEffort::High),
                None,
                None,
            );
            assert_eq!(agent_only.model.as_deref(), Some("agent-model"));
            assert_eq!(agent_only.reasoning_effort, Some(ReasoningEffort::High));
            assert_eq!(agent_only.source, "agent");
        },
    );
}
