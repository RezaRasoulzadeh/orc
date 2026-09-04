//! Opt-in real-Qwen evaluation for supervised Controller memory capture.

#![cfg(feature = "llama-cpp")]

use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_memory_capture::{
    ControllerMemoryCaptureBuilder, ControllerMemoryCaptureCandidate, ControllerMemoryCaptureInput,
    ControllerMemoryCaptureRequest, ControllerMemoryCaptureResult,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
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

fn candidate(
    kind: MemoryKind,
    scope: MemoryScope,
    subject: &str,
    content: &str,
    provenance: MemoryProvenanceKind,
    source: &str,
    source_facts: &[&str],
) -> ControllerMemoryCaptureCandidate {
    ControllerMemoryCaptureCandidate {
        draft: MemoryDraft {
            kind,
            scope,
            subject: subject.into(),
            content: content.into(),
            provenance: MemoryProvenance {
                kind: provenance,
                source_reference: Some(source.into()),
            },
            confidence: Some(0.9),
        },
        source_facts: source_facts.iter().map(|fact| (*fact).into()).collect(),
    }
}

fn memory_item(
    id: MemoryId,
    kind: MemoryKind,
    scope: MemoryScope,
    authority: ControllerMemoryAuthority,
    subject: &str,
    content: &str,
    provenance: MemoryProvenanceKind,
) -> ControllerMemoryItem {
    ControllerMemoryItem {
        id,
        kind,
        scope,
        authority,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: provenance,
            source_reference: Some(format!("evaluation:{subject}")),
        },
        confidence: Some(0.8),
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
    }
}

fn memory(items: Vec<ControllerMemoryItem>) -> ControllerMemoryContext {
    ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items,
    }
}

fn evaluate_case(
    name: &str,
    candidate: ControllerMemoryCaptureCandidate,
    context: ControllerMemoryContext,
    semantic_pass: impl Fn(&ControllerMemoryCaptureResult) -> bool,
    runtime: &mut JsonRuntime,
) -> (bool, bool) {
    let request = ControllerMemoryCaptureRequest::from_candidate(candidate);
    let input = ControllerMemoryCaptureInput::from_request(&request, context);
    match ControllerMemoryCaptureBuilder::new().capture_with_memory(&input, runtime) {
        Ok(result) => {
            let semantic = semantic_pass(&result);
            println!(
                "scenario={name} strict_structured_output=pass semantic_capture_quality={}",
                if semantic { "pass" } else { "fail" }
            );
            (true, semantic)
        }
        Err(error) => {
            println!(
                "scenario={name} strict_structured_output=fail semantic_capture_quality=fail error={error}"
            );
            (false, false)
        }
    }
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF"]
fn qwen3_evaluates_supervised_controller_memory_capture() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let mut runtime = JsonRuntime { inner: runtime };

    let project_decision = candidate(
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "release-gate",
        "Production releases require an operator approval checklist.",
        MemoryProvenanceKind::Operator,
        "operator:release-decision",
        &["The operator explicitly decided this for the current project."],
    );
    let transient = candidate(
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "worker-log",
        "worker stdout: retry 1 completed successfully at 12:04:19",
        MemoryProvenanceKind::Imported,
        "log:run-42",
        &["This is an ephemeral execution detail from one run."],
    );
    let current_over_obsolete = candidate(
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "runtime-language",
        "This project uses Rust 2024 for production services.",
        MemoryProvenanceKind::Operator,
        "operator:current-language",
        &["The current operator/source fact is authoritative for this project."],
    );
    let project_local = candidate(
        MemoryKind::Project,
        MemoryScope::Project { project_id: 1 },
        "local-layout",
        "This repository keeps migrations under db/migrations.",
        MemoryProvenanceKind::ProjectFact,
        "project:layout",
        &["This fact is only about the supplied project."],
    );

    let cases = [
        (
            "durable-project-decision",
            project_decision,
            memory(Vec::new()),
            Box::new(|result: &ControllerMemoryCaptureResult| {
                matches!(
                    result,
                    ControllerMemoryCaptureResult::ProposeMutation { .. }
                )
            }) as Box<dyn Fn(&ControllerMemoryCaptureResult) -> bool>,
        ),
        (
            "transient-execution-log",
            transient,
            memory(Vec::new()),
            Box::new(|result: &ControllerMemoryCaptureResult| {
                matches!(result, ControllerMemoryCaptureResult::Ignore)
            }),
        ),
        (
            "current-candidate-beats-obsolete-history",
            current_over_obsolete,
            memory(vec![
                memory_item(
                    MemoryId::Project {
                        project_id: 1,
                        id: 1,
                    },
                    MemoryKind::Episodic,
                    MemoryScope::Project { project_id: 1 },
                    ControllerMemoryAuthority::ProjectHistory,
                    "runtime-language",
                    "An obsolete historical note said this project used Python 3.",
                    MemoryProvenanceKind::Imported,
                ),
                memory_item(
                    MemoryId::Global(2),
                    MemoryKind::Experience,
                    MemoryScope::Global,
                    ControllerMemoryAuthority::CrossProjectExperience,
                    "runtime-language",
                    "Prefer Python for similar services.",
                    MemoryProvenanceKind::ControllerApproved,
                ),
            ]),
            Box::new(|result: &ControllerMemoryCaptureResult| {
                matches!(
                    result,
                    ControllerMemoryCaptureResult::ProposeMutation { .. }
                )
            }),
        ),
        (
            "project-local-not-global",
            project_local,
            memory(Vec::new()),
            Box::new(|result: &ControllerMemoryCaptureResult| match result {
                ControllerMemoryCaptureResult::Ignore => true,
                ControllerMemoryCaptureResult::ProposeMutation { intent } => {
                    matches!(
                        intent,
                        orc::controller_memory_mutation::ControllerMemoryMutationIntent::Create {
                            draft
                        } if draft.kind == MemoryKind::Project
                            && draft.scope == MemoryScope::Project { project_id: 1 }
                    )
                }
            }),
        ),
    ];

    let mut strict_passes = 0;
    let mut semantic_passes = 0;
    for (name, candidate, context, semantic) in cases {
        let (strict, semantic_pass) =
            evaluate_case(name, candidate, context, semantic, &mut runtime);
        strict_passes += usize::from(strict);
        semantic_passes += usize::from(semantic_pass);
    }
    println!("strict_structured_output={strict_passes}/4");
    println!("semantic_capture_quality={semantic_passes}/4");
    assert_eq!(strict_passes, 4);
    assert_eq!(semantic_passes, 4);
}
