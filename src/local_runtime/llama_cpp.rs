//! The optional llama.cpp implementation of [`super::LocalInferenceRuntime`].
//!
//! Everything in this module is an adapter concern: the binding, GGUF model
//! loading, tokenization and sampler are deliberately not part of the public
//! local-runtime request or response types.

use super::{
    LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceResponseFormat, LocalInferenceRuntime, LocalRuntimeConfig,
};
use encoding_rs::UTF_8;
use llama_cpp_2::{
    context::params::LlamaContextParams,
    json_schema_to_grammar,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, params::LlamaModelParams},
    sampling::LlamaSampler,
};
use std::{convert::TryFrom, fmt, num::NonZeroU32};

/// A local inference runtime backed by the pinned llama.cpp Rust bindings.
///
/// The model is loaded once when the adapter is constructed. A context is
/// created per request so the adapter does not expose or require a
/// self-referential native handle in the model-independent boundary.
pub struct LlamaCppRuntime {
    model: LlamaModel,
    backend: LlamaBackend,
    context_tokens: u32,
    threads: Option<u32>,
}

impl fmt::Debug for LlamaCppRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlamaCppRuntime")
            .field("context_tokens", &self.context_tokens)
            .field("threads", &self.threads)
            .finish_non_exhaustive()
    }
}

impl LlamaCppRuntime {
    /// Load the GGUF model at `config.model_path`.
    pub fn from_config(config: &LocalRuntimeConfig) -> Result<Self, LocalInferenceError> {
        config.validate()?;
        // Validate native-representable settings before initializing the
        // process-wide backend or touching a potentially large model file.
        Self::context_params(config.context_tokens(), config.threads())?;
        if !config.model_path().is_file() {
            return Err(LocalInferenceError::ModelUnavailable(format!(
                "model file does not exist: {}",
                config.model_path().display()
            )));
        }

        let backend = LlamaBackend::init().map_err(|error| {
            LocalInferenceError::Backend(format!(
                "llama.cpp backend initialization failed: {error}"
            ))
        })?;
        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, config.model_path(), &model_params)
            .map_err(|error| {
                LocalInferenceError::ModelUnavailable(format!(
                    "llama.cpp could not load model: {error}"
                ))
            })?;

        Ok(Self {
            backend,
            model,
            context_tokens: config.context_tokens(),
            threads: config.threads(),
        })
    }

    fn context_params(
        context_tokens: u32,
        threads: Option<u32>,
    ) -> Result<LlamaContextParams, LocalInferenceError> {
        let context_tokens = NonZeroU32::new(context_tokens).ok_or_else(|| {
            LocalInferenceError::InvalidConfiguration(
                "context_tokens must be greater than 0".into(),
            )
        })?;
        let mut params = LlamaContextParams::default()
            .with_n_ctx(Some(context_tokens))
            .with_n_batch(context_tokens.get())
            .with_n_ubatch(context_tokens.get());
        if let Some(threads) = threads {
            let threads = i32::try_from(threads).map_err(|_| {
                LocalInferenceError::InvalidConfiguration(
                    "threads must fit in a signed 32-bit integer".into(),
                )
            })?;
            params = params.with_n_threads(threads).with_n_threads_batch(threads);
        }
        Ok(params)
    }

    fn sampler(
        &self,
        request: &LocalInferenceRequest,
    ) -> Result<LlamaSampler, LocalInferenceError> {
        let parameters = &request.parameters;
        let mut samplers = Vec::with_capacity(4);
        if let LocalInferenceResponseFormat::JsonSchema { schema } = &parameters.response_format {
            let schema_json = serde_json::to_string(schema).map_err(|error| {
                LocalInferenceError::InvalidRequest(format!(
                    "response schema could not be serialized: {error}"
                ))
            })?;
            let grammar = json_schema_to_grammar(&schema_json).map_err(|error| {
                LocalInferenceError::InvalidRequest(format!(
                    "response schema could not be converted to a grammar: {error}"
                ))
            })?;
            let grammar_sampler =
                LlamaSampler::grammar(&self.model, &grammar, "root").map_err(|error| {
                    LocalInferenceError::InvalidRequest(format!(
                        "response grammar could not be initialized: {error}"
                    ))
                })?;
            // Grammar runs before the final sampling operation so invalid
            // tokens are masked regardless of temperature/top-p settings.
            samplers.push(grammar_sampler);
        }
        if parameters.temperature == 0.0 {
            samplers.push(LlamaSampler::greedy());
        } else {
            samplers.push(LlamaSampler::temp(parameters.temperature));
            samplers.push(LlamaSampler::top_p(parameters.top_p, 1));
            // A fixed seed keeps this boundary deterministic when callers
            // choose the same request and model; seed policy can evolve
            // without changing the model-independent API.
            samplers.push(LlamaSampler::dist(0x0AC0_0001));
        }
        Ok(LlamaSampler::chain_simple(samplers))
    }

    fn parse_structured_output(output: &str) -> Result<serde_json::Value, String> {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return Err("model output was empty".into());
        }
        serde_json::from_str(trimmed).map_err(|error| format!("invalid JSON: {error}"))
    }

    fn truncate_at_stop(output: &mut String, stop_sequences: &[String]) -> bool {
        let stop_at = stop_sequences
            .iter()
            .filter_map(|stop| output.find(stop))
            .min();
        if let Some(stop_at) = stop_at {
            output.truncate(stop_at);
            true
        } else {
            false
        }
    }
}

impl LocalInferenceRuntime for LlamaCppRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        request.validate()?;
        let tokens = self
            .model
            .str_to_token(&request.prompt, AddBos::Always)
            .map_err(|error| {
                LocalInferenceError::InvalidRequest(format!(
                    "prompt could not be tokenized: {error}"
                ))
            })?;
        let output_budget =
            usize::try_from(request.parameters.max_output_tokens).map_err(|_| {
                LocalInferenceError::InvalidRequest("output token budget is too large".into())
            })?;
        if tokens
            .len()
            .checked_add(output_budget)
            .is_none_or(|required| required > self.context_tokens as usize)
        {
            return Err(LocalInferenceError::InvalidRequest(
                "prompt and output budget exceed the configured context".into(),
            ));
        }

        let mut context = self
            .model
            .new_context(
                &self.backend,
                Self::context_params(self.context_tokens, self.threads)?,
            )
            .map_err(|error| {
                LocalInferenceError::Backend(format!("llama.cpp context creation failed: {error}"))
            })?;
        let mut prompt_batch = LlamaBatch::new(tokens.len(), 1);
        prompt_batch
            .add_sequence(&tokens, 0, false)
            .map_err(|error| {
                LocalInferenceError::Backend(format!("llama.cpp prompt batching failed: {error}"))
            })?;
        context.decode(&mut prompt_batch).map_err(|error| {
            LocalInferenceError::Backend(format!("llama.cpp prompt evaluation failed: {error}"))
        })?;

        let mut sampler = self.sampler(request)?;
        let mut decoder = UTF_8.new_decoder();
        let mut output = String::new();
        for index in 0..request.parameters.max_output_tokens {
            let token = sampler.sample(&context, -1);
            if self.model.is_eog_token(token) {
                break;
            }
            let piece = self
                .model
                .token_to_piece(token, &mut decoder, false, None)
                .map_err(|error| {
                    LocalInferenceError::Backend(format!(
                        "llama.cpp token decoding failed: {error}"
                    ))
                })?;
            output.push_str(&piece);
            // `LlamaSampler::sample` already accepts the selected token in
            // llama-cpp-2; accepting it again corrupts stateful samplers such
            // as the native grammar sampler.
            if Self::truncate_at_stop(&mut output, &request.parameters.stop_sequences) {
                break;
            }
            if index + 1 == request.parameters.max_output_tokens {
                break;
            }

            let position = tokens
                .len()
                .checked_add(usize::try_from(index).expect("u32 fits in usize"))
                .ok_or_else(|| {
                    LocalInferenceError::Backend("llama.cpp token position overflow".into())
                })?;
            let position = i32::try_from(position).map_err(|_| {
                LocalInferenceError::Backend("llama.cpp token position exceeds i32".into())
            })?;
            let mut batch = LlamaBatch::new(1, 1);
            batch.add(token, position, &[0], true).map_err(|error| {
                LocalInferenceError::Backend(format!("llama.cpp token batching failed: {error}"))
            })?;
            context.decode(&mut batch).map_err(|error| {
                LocalInferenceError::Backend(format!("llama.cpp inference failed: {error}"))
            })?;
        }

        let structured_output = match &request.parameters.response_format {
            LocalInferenceResponseFormat::Text => None,
            LocalInferenceResponseFormat::JsonSchema { .. } => Some(
                Self::parse_structured_output(&output).map_err(|parse_error| {
                    LocalInferenceError::InvalidStructuredOutput {
                        raw_output: output.clone(),
                        parse_error,
                    }
                })?,
            ),
        };
        Ok(LocalInferenceResponse {
            text: output,
            structured_output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn missing_model_is_reported_without_native_loading() {
        let config = LocalRuntimeConfig::new(Path::new("/definitely/not/a/model.gguf"));
        let error = LlamaCppRuntime::from_config(&config).expect_err("missing model should fail");
        assert!(matches!(error, LocalInferenceError::ModelUnavailable(_)));
    }

    #[test]
    fn context_settings_map_only_generic_runtime_configuration() {
        let params =
            LlamaCppRuntime::context_params(4096, Some(6)).expect("valid context settings");
        assert_eq!(params.n_ctx(), NonZeroU32::new(4096));
        assert_eq!(params.n_batch(), 4096);
        assert_eq!(params.n_ubatch(), 4096);
        assert_eq!(params.n_threads(), 6);
        assert_eq!(params.n_threads_batch(), 6);
    }

    #[test]
    fn native_thread_overflow_is_an_invalid_configuration() {
        let error = LlamaCppRuntime::context_params(4096, Some(u32::MAX))
            .expect_err("thread count must fit native configuration");
        assert!(matches!(
            error,
            LocalInferenceError::InvalidConfiguration(_)
        ));
    }

    #[test]
    fn stop_sequences_are_truncated_at_the_first_match() {
        let mut output = "answer<stop>tail".to_owned();
        assert!(LlamaCppRuntime::truncate_at_stop(
            &mut output,
            &["<stop>".to_owned(), "other".to_owned()]
        ));
        assert_eq!(output, "answer");
    }

    #[test]
    fn strict_structured_parser_rejects_trailing_output() {
        assert_eq!(
            LlamaCppRuntime::parse_structured_output(r#"{"answer":"ok"}"#)
                .expect("one JSON value should parse")["answer"],
            "ok"
        );
        assert!(
            LlamaCppRuntime::parse_structured_output(r#"{"answer":"ok"} trailing prose"#).is_err()
        );
        assert!(
            LlamaCppRuntime::parse_structured_output(r#"{"answer":"ok"}{"repeat":true}"#).is_err()
        );
    }

    #[test]
    fn native_json_schema_conversion_produces_root_grammar() {
        let grammar = json_schema_to_grammar(
            r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}"#,
        )
        .expect("pinned llama.cpp should convert JSON schema");
        assert!(grammar.contains("root ::="));
    }
}
