//! Ignored full-surface Qwen3 baseline evidence for M09-003.
//!
//! The target is feature-gated and requires the established
//! `ORC_QWEN3_GGUF` local model path. It evaluates the exact M09-002 suite and
//! prints a typed report; it never executes Controller actions or writes Orc
//! state.

#![cfg(feature = "llama-cpp")]

use orc::controller_specialization_baseline::{
    ControllerSpecializationBaselineModelIdentity, ControllerSpecializationBaselineReport,
    ControllerSpecializationBaselineRuntime, ControllerSpecializationBaselineRuntimeRequest,
};
use orc::controller_specialization_evaluation::{
    ControllerSpecializationSuite, evaluate_controller_specialization,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceRequest, LocalInferenceResponse, LocalInferenceRuntime,
    LocalRuntimeConfig,
};
use std::env;
use std::path::PathBuf;

struct RecordingRuntime {
    inner: LlamaCppRuntime,
    requests: Vec<LocalInferenceRequest>,
}

impl LocalInferenceRuntime for RecordingRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, orc::local_runtime::LocalInferenceError> {
        self.requests.push(request.clone());
        self.inner.infer(request)
    }
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF via ORC_QWEN3_GGUF"]
fn qwen3_runs_the_canonical_m09_002_full_surface_baseline() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let suite = ControllerSpecializationSuite::representative_suite()
        .expect("canonical M09-002 representative suite is valid");
    let mut runtime = RecordingRuntime {
        inner: runtime,
        requests: Vec::new(),
    };
    let evaluation = evaluate_controller_specialization(&suite, &mut runtime)
        .expect("canonical M09-002 evaluation report is valid");
    let runtime_requests = suite
        .scenarios
        .iter()
        .filter(|scenario| scenario.requires_runtime())
        .zip(runtime.requests)
        .map(
            |(scenario, request)| ControllerSpecializationBaselineRuntimeRequest {
                scenario_id: scenario.id.clone(),
                capability: scenario.capability.clone(),
                parameters: request.parameters,
            },
        )
        .collect();
    let runtime_metadata =
        ControllerSpecializationBaselineRuntime::from_llama_cpp_config(&config, runtime_requests)
            .expect("recorded native runtime metadata is valid");
    let report = ControllerSpecializationBaselineReport::from_evaluation_report(
        &suite,
        &evaluation,
        ControllerSpecializationBaselineModelIdentity::from_runtime_config(&config)
            .expect("model identity is valid"),
        runtime_metadata,
    )
    .expect("baseline report is valid against the canonical suite");

    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("baseline report serializes")
    );
}
