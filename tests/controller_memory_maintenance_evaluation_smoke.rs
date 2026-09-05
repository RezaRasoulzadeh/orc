//! Opt-in real-Qwen evaluation for supervised Controller memory maintenance.

#![cfg(feature = "llama-cpp")]

use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_memory_maintenance::{
    ControllerMemoryMaintenanceBuilder, ControllerMemoryMaintenanceInput,
    ControllerMemoryMaintenanceRequest, ControllerMemoryMaintenanceResult,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryRecord,
    MemoryScope,
};
use std::env;
use std::path::PathBuf;

struct JsonRuntime {
    inner: LlamaCppRuntime,
}

impl LocalInferenceRuntime for JsonRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        let mut response = self.inner.infer(request)?;
        response.structured_output =
            Some(serde_json::from_str(response.text.trim()).map_err(|error| {
                LocalInferenceError::InvalidStructuredOutput {
                    raw_output: response.text.clone(),
                    parse_error: error.to_string(),
                }
            })?);
        Ok(response)
    }
}

fn record(
    id: MemoryId,
    kind: MemoryKind,
    scope: MemoryScope,
    subject: &str,
    content: &str,
    provenance: MemoryProvenanceKind,
) -> MemoryRecord {
    MemoryRecord {
        id,
        kind,
        scope,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: provenance,
            source_reference: Some(format!("evaluation:{subject}")),
        },
        confidence: Some(0.8),
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn context(items: Vec<ControllerMemoryItem>) -> ControllerMemoryContext {
    ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items,
    }
}

fn item(record: &MemoryRecord, authority: ControllerMemoryAuthority) -> ControllerMemoryItem {
    ControllerMemoryItem {
        id: record.id.clone(),
        kind: record.kind,
        scope: record.scope.clone(),
        authority,
        subject: record.subject.clone(),
        content: record.content.clone(),
        provenance: record.provenance.clone(),
        confidence: record.confidence,
        lifecycle: record.lifecycle,
        supersedes: record.supersedes.clone(),
    }
}

fn evaluate_case(
    name: &str,
    target: MemoryRecord,
    current_facts: Vec<&str>,
    memory: ControllerMemoryContext,
    semantic: impl Fn(&ControllerMemoryMaintenanceResult) -> bool,
    runtime: &mut JsonRuntime,
) -> (bool, bool) {
    let request = ControllerMemoryMaintenanceRequest::new(
        target.id.clone(),
        current_facts.into_iter().map(str::to_owned).collect(),
    );
    let input = ControllerMemoryMaintenanceInput::from_resolved_target(&request, target, memory);
    match ControllerMemoryMaintenanceBuilder::new().maintain(&input, runtime) {
        Ok(result) => {
            let semantic_pass = semantic(&result);
            println!(
                "scenario={name} strict_structured_output=pass semantic_maintenance_quality={}",
                if semantic_pass { "pass" } else { "fail" }
            );
            (true, semantic_pass)
        }
        Err(error) => {
            println!(
                "scenario={name} strict_structured_output=fail semantic_maintenance_quality=fail error={error}"
            );
            (false, false)
        }
    }
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF"]
fn qwen3_evaluates_supervised_controller_memory_maintenance() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let mut runtime = JsonRuntime { inner: runtime };

    let obsolete_decision = record(
        MemoryId::Project {
            project_id: 1,
            id: 1,
        },
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "release-gate",
        "Releases used to require one approver.",
        MemoryProvenanceKind::Imported,
    );
    let correction = record(
        MemoryId::Project {
            project_id: 1,
            id: 2,
        },
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "runtime-language",
        "This project uses Python 3.8.",
        MemoryProvenanceKind::ProjectFact,
    );
    let obsolete = record(
        MemoryId::Project {
            project_id: 1,
            id: 3,
        },
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "temporary-workaround",
        "A one-time migration workaround was needed.",
        MemoryProvenanceKind::Imported,
    );
    let ambiguous = record(
        MemoryId::Project {
            project_id: 1,
            id: 4,
        },
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "deployment-mode",
        "Deployments used a rolling strategy.",
        MemoryProvenanceKind::Imported,
    );
    let ambiguous_history = record(
        MemoryId::Project {
            project_id: 1,
            id: 6,
        },
        MemoryKind::Episodic,
        MemoryScope::Project { project_id: 1 },
        "deployment-mode",
        "An old historical note described a rolling strategy.",
        MemoryProvenanceKind::Imported,
    );
    let local_fact = record(
        MemoryId::Project {
            project_id: 1,
            id: 5,
        },
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "repository-layout",
        "This project stores migrations under db/migrations.",
        MemoryProvenanceKind::ProjectFact,
    );

    let cases = [
        (
            "obsolete-project-decision-new-current-decision",
            obsolete_decision.clone(),
            vec!["The operator explicitly decided that releases now require two approvers."],
            context(Vec::new()),
            Box::new(|result: &ControllerMemoryMaintenanceResult| {
                matches!(
                    result,
                    ControllerMemoryMaintenanceResult::ProposeMutation {
                        intent: orc::controller_memory_mutation::ControllerMemoryMutationIntent::Supersede { .. }
                    }
                )
            }) as Box<dyn Fn(&ControllerMemoryMaintenanceResult) -> bool>,
        ),
        (
            "materially-incorrect-current-record",
            correction.clone(),
            vec![
                "The selected current record is factually wrong: production services use Rust 2024; correct the runtime language.",
            ],
            context(Vec::new()),
            Box::new(|result: &ControllerMemoryMaintenanceResult| {
                matches!(
                    result,
                    ControllerMemoryMaintenanceResult::ProposeMutation {
                        intent: orc::controller_memory_mutation::ControllerMemoryMutationIntent::Correct { .. }
                    }
                )
            }),
        ),
        (
            "obsolete-no-replacement-value",
            obsolete.clone(),
            vec![
                "This temporary workaround has no continuing durable value and has no replacement.",
            ],
            context(Vec::new()),
            Box::new(|result: &ControllerMemoryMaintenanceResult| {
                matches!(
                    result,
                    ControllerMemoryMaintenanceResult::ProposeMutation {
                        intent: orc::controller_memory_mutation::ControllerMemoryMutationIntent::Remove { .. }
                    }
                )
            }),
        ),
        (
            "ambiguous-historical-contradiction",
            ambiguous.clone(),
            vec![
                "There is no authoritative current operator or project fact about deployment strategy.",
            ],
            context(vec![item(
                &ambiguous_history,
                ControllerMemoryAuthority::ProjectHistory,
            )]),
            Box::new(|result: &ControllerMemoryMaintenanceResult| {
                matches!(result, ControllerMemoryMaintenanceResult::Keep)
            }),
        ),
        (
            "project-local-fact-does-not-rewrite-global-memory",
            local_fact.clone(),
            vec!["This evidence applies only to the supplied project and not to other projects."],
            context(vec![
                item(
                    &record(
                        MemoryId::Global(1),
                        MemoryKind::User,
                        MemoryScope::Global,
                        "repository-layout",
                        "Other projects use a different layout.",
                        MemoryProvenanceKind::Operator,
                    ),
                    ControllerMemoryAuthority::DurableUser,
                ),
                item(
                    &record(
                        MemoryId::Global(2),
                        MemoryKind::Experience,
                        MemoryScope::Global,
                        "repository-layout",
                        "Prefer a single universal layout everywhere.",
                        MemoryProvenanceKind::ControllerApproved,
                    ),
                    ControllerMemoryAuthority::CrossProjectExperience,
                ),
            ]),
            Box::new(|result: &ControllerMemoryMaintenanceResult| {
                match result {
                ControllerMemoryMaintenanceResult::Keep => true,
                ControllerMemoryMaintenanceResult::ProposeMutation { intent } => match intent {
                    orc::controller_memory_mutation::ControllerMemoryMutationIntent::Correct {
                        replacement,
                        ..
                    }
                    | orc::controller_memory_mutation::ControllerMemoryMutationIntent::Supersede {
                        replacement,
                        ..
                    } => {
                        replacement.kind == MemoryKind::Project
                            && replacement.scope == (MemoryScope::Project { project_id: 1 })
                    }
                    orc::controller_memory_mutation::ControllerMemoryMutationIntent::Remove {
                        ..
                    } => true,
                    orc::controller_memory_mutation::ControllerMemoryMutationIntent::Create {
                        ..
                    } => false,
                },
            }
            }),
        ),
    ];

    let mut strict_passes = 0;
    let mut semantic_passes = 0;
    for (name, target, current_facts, memory, semantic) in cases {
        let (strict, semantic_pass) =
            evaluate_case(name, target, current_facts, memory, semantic, &mut runtime);
        strict_passes += usize::from(strict);
        semantic_passes += usize::from(semantic_pass);
    }
    println!("strict_structured_output={strict_passes}/5");
    println!("semantic_maintenance_quality={semantic_passes}/5");
    assert_eq!(strict_passes, 5);
    assert_eq!(semantic_passes, 5);
}
