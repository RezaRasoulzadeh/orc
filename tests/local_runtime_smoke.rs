//! Opt-in native-model smoke coverage for the M01-002 adapter.
//!
//! This test is ignored and requires both `--features llama-cpp` and a local
//! user-supplied Qwen3 8B GGUF path. It is intentionally absent from normal
//! deterministic test runs.

#![cfg(feature = "llama-cpp")]

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF; see docs/orc-next/tasks/M01-002.md"]
fn qwen3_8b_gguf_returns_bounded_text() {
    use orc::local_runtime::{
        LlamaCppRuntime, LocalInferenceParameters, LocalInferenceRequest, LocalInferenceRuntime,
        LocalRuntimeConfig,
    };
    use std::{env, path::PathBuf};

    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let mut runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let parameters = LocalInferenceParameters {
        max_output_tokens: 64,
        temperature: 0.0,
        top_p: 1.0,
        stop_sequences: vec![],
    };
    let request = LocalInferenceRequest::new(
        "Reply with exactly one short sentence confirming local inference.",
        parameters,
    )
    .expect("smoke request is valid");
    let response = runtime.infer(&request).expect("local inference succeeds");
    assert!(!response.text.trim().is_empty(), "model returned no text");
    assert!(
        response.text.chars().count() <= 4096,
        "response exceeded smoke bound"
    );
}
