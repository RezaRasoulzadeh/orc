# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M03 — Typed Controller tools

**Current task:** M03-003 — Connect Controller recommendations to supervised typed actions

**Last completed:** M03-002 — Execute explicitly authorized Controller intents

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- `src/local_runtime.rs` is the replaceable local inference seam; native backend details remain outside Controller/domain types. The optional `llama-cpp` feature implements the first adapter in `src/local_runtime/llama_cpp.rs`.
- Controller recommendations use a model-independent JSON Schema request; the llama.cpp adapter applies native grammar-constrained sampling and strict full-value parsing, retaining raw output on structured parse failure.
- Controller state carries an explicit canonical operational-consistency observation so derived next-step data cannot silently override contradictory lifecycle facts.
- M02 is complete: the final unchanged seven-scenario Qwen3 evaluation achieved `7/7` semantic decisions and `7/7` strict structured-contract compliance.
- M03-001 is complete and source-reviewed `PASS`: typed Controller intents for dispatch/review/revise/accept can be inspected through canonical `ProjectOperations` legality without mutation.
- M03-002 is complete and source-reviewed `PASS`: an opaque one-shot host authorization plus a fresh legality re-check gates canonical dispatch/review/revise/accept execution.
- A legality inspection or Controller recommendation is not authorization and is not a durable grant.
- Model-owned recommendation/intent must never carry or manufacture its own authorization/confirmation.
- M03-003 connects recommendation to typed intent under explicit supervision; it must not introduce an inference/action continuation loop.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement M03-003 against `docs/orc-next/tasks/M03-003.md`.

The task connects the existing bounded read-only Controller recommendation path
to the M03 typed action boundary. Supported recommendations may become proposed
`ControllerActionIntent` values, but proposal generation remains non-mutating
and cannot mint authorization. Unsupported recommendations remain explicitly
non-executable.

A trusted caller may explicitly authorize one proposed action and must reuse the
M03-002 execution boundary, which performs a fresh
`ProjectOperations::inspect_action` check immediately before canonical
mutation. Do not add automatic authorization, inference/action loops, recovery,
planning/Lead migration, memory, scheduler redesign, model changes, Python, GPU
work, or interface confirmation UX.

M03-002 completion evidence:

- Luna + High source review: `PASS`;
- focused Controller action tests: 10 passed;
- `cargo test --lib`: 303 passed;
- `cargo test --features llama-cpp --lib`: 309 passed;
- normal and feature clippy, fmt, and diff check: passed;
- read-only recommendation code unchanged, so real Qwen evaluation was not rerun;
- no M03-003 architectural blocker identified.

See `M00-REPOSITORY-MAP.md` for the repository-grounded migration map.
