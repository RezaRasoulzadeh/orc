//! Model-independent boundary for local Controller inference.
//!
//! This module intentionally contains no Controller state, lifecycle, storage,
//! provider, model-family, tokenizer, or native-backend types. The optional
//! native adapter (initially implemented with llama.cpp/GGUF) accepts
//! [`LocalRuntimeConfig`] and implements [`LocalInferenceRuntime`]. It
//! translates its native request,
//! output, and failure details at that boundary, leaving Controller-facing
//! types replaceable when the model or runtime changes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Maximum prompt size accepted by the local runtime boundary.
pub const MAX_PROMPT_BYTES: usize = 256 * 1024;
/// Maximum output budget accepted by one local inference request.
pub const MAX_OUTPUT_TOKENS: u32 = 16 * 1024;
const MAX_STOP_SEQUENCES: usize = 32;
const MAX_STOP_SEQUENCE_BYTES: usize = 1024;

/// Runtime-owned sampling and output controls.
///
/// These settings are deliberately separate from Controller/domain types. A
/// native adapter may map them to its own sampler configuration without
/// exposing backend-specific structures to the rest of Orc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalInferenceParameters {
    pub max_output_tokens: u32,
    pub temperature: f32,
    pub top_p: f32,
    #[serde(default)]
    pub stop_sequences: Vec<String>,
}

impl Default for LocalInferenceParameters {
    fn default() -> Self {
        Self {
            max_output_tokens: 1024,
            temperature: 0.2,
            top_p: 0.95,
            stop_sequences: Vec::new(),
        }
    }
}

impl LocalInferenceParameters {
    fn validate(&self) -> Result<(), LocalInferenceError> {
        if self.max_output_tokens == 0 || self.max_output_tokens > MAX_OUTPUT_TOKENS {
            return Err(LocalInferenceError::InvalidRequest(format!(
                "max_output_tokens must be between 1 and {MAX_OUTPUT_TOKENS}"
            )));
        }
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err(LocalInferenceError::InvalidRequest(
                "temperature must be finite and between 0 and 2".into(),
            ));
        }
        if !self.top_p.is_finite() || !(0.0..=1.0).contains(&self.top_p) || self.top_p == 0.0 {
            return Err(LocalInferenceError::InvalidRequest(
                "top_p must be finite and greater than 0 and at most 1".into(),
            ));
        }
        if self.stop_sequences.len() > MAX_STOP_SEQUENCES {
            return Err(LocalInferenceError::InvalidRequest(format!(
                "at most {MAX_STOP_SEQUENCES} stop sequences are supported"
            )));
        }
        if self
            .stop_sequences
            .iter()
            .any(|sequence| sequence.is_empty() || sequence.len() > MAX_STOP_SEQUENCE_BYTES)
        {
            return Err(LocalInferenceError::InvalidRequest(format!(
                "stop sequences must be non-empty and at most {MAX_STOP_SEQUENCE_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// One bounded, model-independent local inference request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalInferenceRequest {
    pub prompt: String,
    pub parameters: LocalInferenceParameters,
}

impl LocalInferenceRequest {
    /// Construct a request after enforcing the runtime boundary's bounds.
    pub fn new(
        prompt: impl Into<String>,
        parameters: LocalInferenceParameters,
    ) -> Result<Self, LocalInferenceError> {
        let request = Self {
            prompt: prompt.into(),
            parameters,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), LocalInferenceError> {
        if self.prompt.trim().is_empty() {
            return Err(LocalInferenceError::InvalidRequest(
                "prompt must not be empty".into(),
            ));
        }
        if self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(LocalInferenceError::InvalidRequest(format!(
                "prompt exceeds the {MAX_PROMPT_BYTES}-byte limit"
            )));
        }
        self.parameters.validate()
    }
}

/// Text and optional structured output returned by a local runtime.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalInferenceResponse {
    pub text: String,
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,
}

impl LocalInferenceResponse {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            structured_output: None,
        }
    }

    pub fn structured(text: impl Into<String>, value: serde_json::Value) -> Self {
        Self {
            text: text.into(),
            structured_output: Some(value),
        }
    }
}

/// Runtime and request failures exposed to the future Controller boundary.
#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum LocalInferenceError {
    #[error("invalid local inference request: {0}")]
    InvalidRequest(String),
    #[error("invalid local runtime configuration: {0}")]
    InvalidConfiguration(String),
    #[error("local model is unavailable: {0}")]
    ModelUnavailable(String),
    #[error("local inference backend failed: {0}")]
    Backend(String),
    #[error("local inference was cancelled")]
    Cancelled,
}

/// Runtime concerns needed to construct a native local-model adapter.
///
/// The model path and execution settings are kept here instead of in
/// Controller/domain state. Validation checks shape only; the native adapter
/// owns model-file loading and backend-specific availability checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalRuntimeConfig {
    model_path: PathBuf,
    context_tokens: u32,
    threads: Option<u32>,
}

impl LocalRuntimeConfig {
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            context_tokens: 8192,
            threads: None,
        }
    }

    pub fn with_context_tokens(mut self, context_tokens: u32) -> Self {
        self.context_tokens = context_tokens;
        self
    }

    pub fn with_threads(mut self, threads: Option<u32>) -> Self {
        self.threads = threads;
        self
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    pub fn context_tokens(&self) -> u32 {
        self.context_tokens
    }

    pub fn threads(&self) -> Option<u32> {
        self.threads
    }

    pub fn validate(&self) -> Result<(), LocalInferenceError> {
        if self.model_path.as_os_str().is_empty() {
            return Err(LocalInferenceError::InvalidConfiguration(
                "model_path must not be empty".into(),
            ));
        }
        if self.context_tokens == 0 {
            return Err(LocalInferenceError::InvalidConfiguration(
                "context_tokens must be greater than 0".into(),
            ));
        }
        if self.threads.is_some_and(|threads| threads == 0) {
            return Err(LocalInferenceError::InvalidConfiguration(
                "threads must be greater than 0 when specified".into(),
            ));
        }
        Ok(())
    }
}

/// Replaceable native local inference boundary.
///
/// M01-002's llama.cpp adapter can implement this trait by translating
/// [`LocalRuntimeConfig`] into native context/model settings and translating
/// native output/errors into [`LocalInferenceResponse`] and
/// [`LocalInferenceError`]. No llama.cpp, GGUF, Qwen, tokenizer, or model
/// handle crosses this trait.
pub trait LocalInferenceRuntime: Send {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError>;
}

#[cfg(feature = "llama-cpp")]
mod llama_cpp;

/// Native llama.cpp implementation of the model-independent local runtime.
#[cfg(feature = "llama-cpp")]
pub use llama_cpp::LlamaCppRuntime;

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRuntime {
        result: Result<LocalInferenceResponse, LocalInferenceError>,
        requests: Vec<LocalInferenceRequest>,
    }

    impl FakeRuntime {
        fn responding(response: LocalInferenceResponse) -> Self {
            Self {
                result: Ok(response),
                requests: Vec::new(),
            }
        }

        fn failing(error: LocalInferenceError) -> Self {
            Self {
                result: Err(error),
                requests: Vec::new(),
            }
        }
    }

    impl LocalInferenceRuntime for FakeRuntime {
        fn infer(
            &mut self,
            request: &LocalInferenceRequest,
        ) -> Result<LocalInferenceResponse, LocalInferenceError> {
            request.validate()?;
            self.requests.push(request.clone());
            self.result.clone()
        }
    }

    #[test]
    fn fake_runtime_propagates_text_and_structured_response() {
        let response = LocalInferenceResponse::structured(
            "recommend",
            serde_json::json!({"action": "inspect"}),
        );
        let mut runtime = FakeRuntime::responding(response.clone());
        let request = LocalInferenceRequest::new("bounded prompt", Default::default()).unwrap();

        assert_eq!(runtime.infer(&request).unwrap(), response);
        assert_eq!(runtime.requests, vec![request]);
    }

    #[test]
    fn fake_runtime_propagates_typed_backend_failure() {
        let error = LocalInferenceError::Backend("fake backend stopped".into());
        let mut runtime = FakeRuntime::failing(error.clone());
        let request = LocalInferenceRequest::new("bounded prompt", Default::default()).unwrap();

        assert_eq!(runtime.infer(&request), Err(error));
    }

    #[test]
    fn request_bounds_are_checked_without_backend_access() {
        let empty = LocalInferenceRequest::new(" ", Default::default()).unwrap_err();
        assert!(matches!(empty, LocalInferenceError::InvalidRequest(_)));

        let oversized = "x".repeat(MAX_PROMPT_BYTES + 1);
        let error = LocalInferenceRequest::new(oversized, Default::default()).unwrap_err();
        assert!(matches!(error, LocalInferenceError::InvalidRequest(_)));
    }

    #[test]
    fn runtime_configuration_is_separate_from_controller_request() {
        let config = LocalRuntimeConfig::new("models/controller.gguf")
            .with_context_tokens(4096)
            .with_threads(Some(4));
        assert!(config.validate().is_ok());

        let request = LocalInferenceRequest::new("prompt", Default::default()).unwrap();
        let serialized = serde_json::to_value(request).unwrap();
        assert!(serialized.get("model_path").is_none());
        assert!(serialized.get("context_tokens").is_none());
        assert!(serialized.get("threads").is_none());
    }

    #[test]
    fn configuration_shape_errors_are_typed_and_do_not_touch_the_filesystem() {
        let empty = LocalRuntimeConfig::new("").validate().unwrap_err();
        assert!(matches!(
            empty,
            LocalInferenceError::InvalidConfiguration(_)
        ));

        let missing_file = LocalRuntimeConfig::new("does-not-exist/controller.gguf");
        assert!(missing_file.validate().is_ok());
    }
}
