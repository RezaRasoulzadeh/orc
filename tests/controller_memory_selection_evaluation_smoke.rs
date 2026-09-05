//! Opt-in real-Qwen evaluation for bounded Controller memory target selection.

#![cfg(feature = "llama-cpp")]

use orc::controller_memory_selection::{
    ControllerMemorySelectionBuilder, ControllerMemorySelectionCandidate,
    ControllerMemorySelectionInput, ControllerMemorySelectionRequest,
    ControllerMemorySelectionResult,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
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
    id: i64,
    kind: MemoryKind,
    subject: &str,
    content: &str,
) -> ControllerMemorySelectionCandidate {
    ControllerMemorySelectionCandidate {
        id: MemoryId::Project { project_id: 1, id },
        kind,
        scope: MemoryScope::Project { project_id: 1 },
        lifecycle: MemoryLifecycle::Active,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Imported,
            source_reference: Some(format!("evaluation:{subject}")),
        },
        confidence: Some(0.8),
    }
}

fn input(
    current_facts: &[&str],
    candidates: Vec<ControllerMemorySelectionCandidate>,
    eligible_candidate_count: usize,
) -> ControllerMemorySelectionInput {
    let selected_candidate_count = candidates.len();
    ControllerMemorySelectionInput {
        current_project_id: 1,
        current_request: ControllerMemorySelectionRequest::new(
            current_facts.iter().map(|fact| (*fact).into()).collect(),
        ),
        candidates,
        eligible_candidate_count,
        selected_candidate_count,
        omitted_candidate_count: eligible_candidate_count - selected_candidate_count,
    }
}

fn evaluate_case(
    name: &str,
    input: ControllerMemorySelectionInput,
    expected: ControllerMemorySelectionResult,
    runtime: &mut JsonRuntime,
) -> (bool, bool) {
    let semantic_expected = expected.clone();
    match ControllerMemorySelectionBuilder::new().select(&input, runtime) {
        Ok(result) => {
            let strict = true;
            let semantic = result == semantic_expected;
            println!(
                "scenario={name} strict_structured_output=pass semantic_target_selection={}",
                if semantic { "pass" } else { "fail" }
            );
            (strict, semantic)
        }
        Err(error) => {
            println!(
                "scenario={name} strict_structured_output=fail semantic_target_selection=fail error={error}"
            );
            (false, false)
        }
    }
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF"]
fn qwen3_evaluates_bounded_controller_memory_target_selection() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let mut runtime = JsonRuntime { inner: runtime };

    let project = candidate(
        1,
        MemoryKind::Project,
        "release-gate",
        "Releases used to require one approver.",
    );
    let episodic = candidate(
        2,
        MemoryKind::Episodic,
        "incident-outcome",
        "The incident was mitigated with a temporary workaround.",
    );
    let unrelated = candidate(
        3,
        MemoryKind::Project,
        "repository-layout",
        "Migrations are stored under db/migrations.",
    );

    let cases = vec![
        (
            "no-candidates",
            input(
                &["There is no current evidence requiring maintenance."],
                Vec::new(),
                0,
            ),
            ControllerMemorySelectionResult::NoTarget,
        ),
        (
            "candidates-no-maintenance-evidence",
            input(
                &["The project is operating normally; no memory is identified for maintenance."],
                vec![project.clone(), episodic.clone()],
                2,
            ),
            ControllerMemorySelectionResult::NoTarget,
        ),
        (
            "explicit-project-target",
            input(
                &[
                    "The operator explicitly says the release-gate memory is obsolete and needs maintenance review.",
                ],
                vec![project.clone(), episodic.clone()],
                2,
            ),
            ControllerMemorySelectionResult::SelectTarget {
                target: project.id.clone(),
            },
        ),
        (
            "explicit-episodic-target",
            input(
                &[
                    "The incident-outcome memory is now inconsistent with the current incident result and needs maintenance review.",
                ],
                vec![project.clone(), episodic.clone()],
                2,
            ),
            ControllerMemorySelectionResult::SelectTarget {
                target: episodic.id.clone(),
            },
        ),
        (
            "multiple-candidates-one-clear-target",
            input(
                &[
                    "The operator explicitly identified the incident-outcome record as obsolete after the workaround was removed.",
                ],
                vec![project.clone(), episodic.clone(), unrelated.clone()],
                3,
            ),
            ControllerMemorySelectionResult::SelectTarget {
                target: episodic.id.clone(),
            },
        ),
        (
            "unrelated-evidence",
            input(
                &[
                    "The operator discussed an unrelated future documentation task, not any supplied memory record.",
                ],
                vec![project.clone(), episodic.clone(), unrelated.clone()],
                3,
            ),
            ControllerMemorySelectionResult::NoTarget,
        ),
        (
            "bounded-omitted-candidates",
            input(
                &[
                    "The release-gate candidate is explicitly identified as needing maintenance review.",
                ],
                vec![project.clone(), episodic.clone()],
                8,
            ),
            ControllerMemorySelectionResult::SelectTarget {
                target: project.id.clone(),
            },
        ),
    ];

    let mut strict_passes = 0;
    let mut semantic_passes = 0;
    for (name, input, expected) in cases {
        let (strict, semantic) = evaluate_case(name, input, expected, &mut runtime);
        strict_passes += usize::from(strict);
        semantic_passes += usize::from(semantic);
    }
    println!(
        "strict_structured_output={strict_passes}/7 semantic_target_selection={semantic_passes}/7"
    );
    assert_eq!(strict_passes, 7);
    assert_eq!(semantic_passes, 7);
}
